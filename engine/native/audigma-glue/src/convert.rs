// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! PCM to DSD conversion with audigma (`docs/playback-engine-design.md`, data path 1).
//!
//! The JVM sink pushes interleaved PCM ([`Converter::push`], never blocks) into a planar input
//! ring. The converter thread is the coordinator of audigma's channel worker pool: it runs the
//! input stage (rational SRC for the 48 kHz family), the half-band cascade and the modulators,
//! packs the MSB-first DSD bytes to M7 ([`M7Stream`]) and hands them to the writer's ring
//! ([`Engine::push_m7`]), waiting while that ring is full. Consecutive pushes form one stream:
//! the filters and modulators run on across gapless track boundaries.
//!
//! Control, from the JVM playback thread:
//! - [`Converter::reset`] drops queued PCM; the thread resets filters, modulators and the packer
//!   before its next block. Call it before the writer's reset: a block converted from old input
//!   that finishes after `reset` is discarded (generation check under the state lock, held while
//!   pushing), and one pushed before it is dropped by the writer's reset.
//! - [`Converter::end_of_stream`] converts what is queued, then feeds digital silence for the
//!   filters' group delay so the end of the input reaches the output.

use crate::m7::{M7_FRAME, M7Stream};
use crate::writer::Engine;
use audigma_dsp::config::DspConfiguration;
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::rates::PcmRateHz;
use audigma_dsp::config::TargetRateSpec;
use audigma_engine::{
    FrameGain, InputDspConfig, InputResampling, InputTileControls, PipelineTopology,
    ThreadPlacement, TileProcessor, Waiter, WorkerSetup,
};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Error code reported when the DSP failed; the converter thread has stopped.
pub const ERR_DSP: i32 = -2000;

/// Media3 `C.ENCODING_PCM_16BIT`, `C.ENCODING_PCM_FLOAT`, `C.ENCODING_PCM_24BIT` (packed),
/// `C.ENCODING_PCM_32BIT`. Samples are little-endian.
pub const ENCODING_PCM_16BIT: i32 = 2;
pub const ENCODING_PCM_FLOAT: i32 = 4;
pub const ENCODING_PCM_24BIT: i32 = 21;
pub const ENCODING_PCM_32BIT: i32 = 22;

/// Baseband frames per worker dispatch (audigma's offline tile; the live daemon uses 256).
const TILE_FRAMES: usize = 512;
/// Input ring capacity in frames (about 1.5 s at 44.1 kHz).
const INPUT_FRAMES: usize = 65536;
/// Recheck interval while waiting for room in the writer's ring.
const ROOM_WAIT: Duration = Duration::from_millis(50);

struct PcmRing {
    l: Vec<f64>,
    r: Vec<f64>,
    head: usize,
    len: usize,
}

impl PcmRing {
    fn free(&self) -> usize {
        self.l.len() - self.len
    }

    fn push(&mut self, a: f64, b: f64) {
        let i = (self.head + self.len) % self.l.len();
        self.l[i] = a;
        self.r[i] = b;
        self.len += 1;
    }

    fn pop_into(&mut self, l: &mut [f64], r: &mut [f64]) {
        let n = l.len();
        debug_assert!(n <= self.len && r.len() == n);
        let cap = self.l.len();
        let first = n.min(cap - self.head);
        l[..first].copy_from_slice(&self.l[self.head..self.head + first]);
        r[..first].copy_from_slice(&self.r[self.head..self.head + first]);
        l[first..].copy_from_slice(&self.l[..n - first]);
        r[first..].copy_from_slice(&self.r[..n - first]);
        self.head = (self.head + n) % cap;
        self.len -= n;
    }
}

struct State {
    ring: PcmRing,
    /// Frames taken from the ring for the block being converted.
    in_flight: usize,
    /// Frames of silence still to feed after the input ([`Converter::end_of_stream`]).
    tail_left: usize,
    /// `end_of_stream` was called and no input arrived since.
    ended: bool,
    /// Bumped by [`Converter::reset`].
    generation: u64,
    quit: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the converter thread: input, end of stream, reset, quit.
    wake: Condvar,
    error: AtomicI32,
    /// Worker and converter threads whose pinning to the fast cores failed / succeeded.
    pin_failed: AtomicUsize,
    pin_ok: AtomicUsize,
    /// Diagnostics for loud material: input samples clamped to full scale after the gain, the
    /// largest |sample| after the gain since the last [`Converter::stats`] (f32 bits; for
    /// non-negative floats the bit order is the value order) and the modulators' overloads.
    clipped: AtomicU64,
    peak_bits: AtomicU32,
    overloads: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A running PCM to DSD conversion at one input rate and one DSD rate.
pub struct Converter {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    input_rate: u32,
    dsd_rate: u32,
    tail_frames: usize,
    group_delay_samples: usize,
    description: String,
}

impl Converter {
    /// Prepares audigma for `input_rate` Hz PCM to `dsd_rate` Hz stereo DSD under `options` (see
    /// [`crate::options`]; `dsd_rate` is what [`crate::options::dsd_rate`] resolved) and starts
    /// the converter thread, which writes to `engine`.
    pub fn new(
        engine: Arc<Engine>,
        dsd_rate: u32,
        input_rate: u32,
        options: &str,
    ) -> Result<Converter, String> {
        let pcm_rate =
            PcmRateHz::new(input_rate).ok_or(format!("unsupported PCM rate {input_rate}"))?;
        let plan = crate::options::plan(options, input_rate)?;
        let target = plan.stream_spec.target;
        if !matches!(target, TargetRateSpec::Dsd { bit_rate } if bit_rate.get() == dsd_rate) {
            return Err(format!("audigma resolved {target:?}, expected DSD {dsd_rate}"));
        }
        let topology = PipelineTopology::resolve(pcm_rate, target).map_err(|e| e.to_string())?;
        let input_config = InputDspConfig {
            pcm_decimation_stages: topology.decimation_stages,
            dsd: None,
            resampling: if topology.requires_rational_src {
                InputResampling::Rational {
                    input_rate: pcm_rate,
                    output_rate: topology.intermediate_rate,
                }
            } else {
                InputResampling::None
            },
        };
        let gain_db = plan.dsp_config.total_configured_gain_db();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ring: PcmRing {
                    l: vec![0.0; INPUT_FRAMES],
                    r: vec![0.0; INPUT_FRAMES],
                    head: 0,
                    len: 0,
                },
                in_flight: 0,
                tail_left: 0,
                ended: false,
                generation: 0,
                quit: false,
            }),
            wake: Condvar::new(),
            error: AtomicI32::new(0),
            pin_failed: AtomicUsize::new(0),
            pin_ok: AtomicUsize::new(0),
            clipped: AtomicU64::new(0),
            peak_bits: AtomicU32::new(0),
            overloads: AtomicU64::new(0),
        });
        let fast = fast_cpus();
        let processor = TileProcessor::new_with_input(
            2,
            plan.dsp_config.phase,
            plan.dsp_config.apodization,
            topology,
            TILE_FRAMES,
            topology.target_dsd_config(&plan.dsp_config),
            false,
            input_config,
            Some(worker_setup(&fast, &shared)),
        )
        .map_err(|e| e.to_string())?;
        let mult = processor.rate_multiplier();
        let group_delay_samples = processor.total_group_delay_output_samples();
        // As audigma's offline tail: the group delay in baseband frames plus a margin, here in
        // input frames (the SRC changes the rate before the cascade).
        let baseband = group_delay_samples / mult + 48;
        let tail_frames = (baseband as u64 * input_rate as u64)
            .div_ceil(topology.intermediate_rate.get() as u64) as usize;
        let description = format!(
            "PCM {input_rate} Hz -> DSD {dsd_rate} Hz: {}, src {}, x{mult}, group delay {} us, \
             gain {gain_db:.2} dB, fast cpus {fast:?}",
            plan.dsp_config.filter_description(),
            topology.requires_rational_src,
            group_delay_samples as u64 * 1_000_000 / dsd_rate as u64,
        );
        let linear_gain = DspConfiguration::compute_linear_gain(gain_db, false);
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("audigma-convert".into())
            .spawn(move || {
                pin(&fast, &sh);
                convert_loop(&sh, &engine, processor, linear_gain);
            })
            .map_err(|e| e.to_string())?;
        Ok(Converter {
            shared,
            thread: Some(thread),
            input_rate,
            dsd_rate,
            tail_frames,
            group_delay_samples,
            description,
        })
    }

    /// Converts interleaved PCM (`channels` 1 or 2, Media3 `encoding`) scaled by `gain` into the
    /// input ring, as many whole frames as fit. Returns the bytes taken, `None` for an
    /// unsupported encoding or channel count.
    pub fn push(&self, src: &[u8], encoding: i32, channels: usize, gain: f32) -> Option<usize> {
        let bytes = match encoding {
            ENCODING_PCM_16BIT => 2,
            ENCODING_PCM_24BIT => 3,
            ENCODING_PCM_32BIT | ENCODING_PCM_FLOAT => 4,
            _ => return None,
        };
        if !(1..=2).contains(&channels) {
            return None;
        }
        let frame = bytes * channels;
        let g = gain as f64;
        let mut st = self.shared.lock();
        let n = (src.len() / frame).min(st.ring.free());
        let (mut peak, mut clipped) = (0.0f64, 0u64);
        let mut limit = |v: f64| {
            let m = v.abs();
            peak = peak.max(m);
            clipped += u64::from(m > 1.0);
            v.clamp(-1.0, 1.0)
        };
        for f in src[..n * frame].chunks_exact(frame) {
            let a = limit(sample(&f[..bytes], encoding) * g);
            let b = if channels == 2 {
                limit(sample(&f[bytes..], encoding) * g)
            } else {
                a
            };
            st.ring.push(a, b);
        }
        self.shared.clipped.fetch_add(clipped, Ordering::Relaxed);
        self.shared.peak_bits.fetch_max((peak as f32).to_bits(), Ordering::Relaxed);
        if n > 0 {
            // More input after the end: no more silence, the stream goes on.
            st.ended = false;
            st.tail_left = 0;
            self.shared.wake.notify_all();
        }
        Some(n * frame)
    }

    /// Converts everything queued, then the filters' tail. Repeated calls add nothing.
    pub fn end_of_stream(&self) {
        let mut st = self.shared.lock();
        if !st.ended {
            st.ended = true;
            st.tail_left = self.tail_frames;
            self.shared.wake.notify_all();
        }
    }

    /// Drops queued input and the tail; the DSP state is reset before the next block.
    pub fn reset(&self) {
        let mut st = self.shared.lock();
        st.ring.len = 0;
        st.ring.head = 0;
        st.tail_left = 0;
        st.ended = false;
        st.generation += 1;
        self.shared.wake.notify_all();
    }

    /// M7 bytes still to come out of the converter (queued input, the block in flight, the
    /// pending tail), estimated from frame counts.
    pub fn pending_bytes(&self) -> u64 {
        let st = self.shared.lock();
        let frames = (st.ring.len + st.in_flight + st.tail_left) as u64;
        // 2 channels x (dsd_rate / input_rate) bits per frame.
        (frames * self.dsd_rate as u64).div_ceil(self.input_rate as u64 * 4)
    }

    /// Group delay of the filters in output samples (DSD bits per channel).
    pub fn group_delay_samples(&self) -> usize {
        self.group_delay_samples
    }

    pub fn error(&self) -> i32 {
        self.shared.error.load(Ordering::Relaxed)
    }

    pub fn clear_error(&self) {
        self.shared.error.store(0, Ordering::Relaxed);
    }

    /// `(clipped samples, overloads, peak since the last call)` for logs; the peak is the
    /// largest |sample| after gain, 1.0 = full scale.
    pub fn stats(&self) -> (u64, u64, f32) {
        let peak = f32::from_bits(self.shared.peak_bits.swap(0, Ordering::Relaxed));
        (
            self.shared.clipped.load(Ordering::Relaxed),
            self.shared.overloads.load(Ordering::Relaxed),
            peak,
        )
    }

    /// Configuration and thread placement, for logs.
    pub fn describe(&self) -> String {
        format!(
            "{}, pinned {} failed {}",
            self.description,
            self.shared.pin_ok.load(Ordering::Relaxed),
            self.shared.pin_failed.load(Ordering::Relaxed)
        )
    }

    /// Stops and joins the converter thread (its worker pool exits with it). `false` if it did
    /// not exit within `timeout`; it is then left detached.
    pub fn destroy(&mut self, timeout: Duration) -> bool {
        {
            let mut st = self.shared.lock();
            st.quit = true;
            self.shared.wake.notify_all();
        }
        let deadline = Instant::now() + timeout;
        let Some(t) = self.thread.as_ref() else {
            return true;
        };
        while !t.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.thread.take().map(|t| t.join());
        true
    }
}

fn sample(b: &[u8], encoding: i32) -> f64 {
    match encoding {
        ENCODING_PCM_16BIT => i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0,
        ENCODING_PCM_24BIT => {
            (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f64 / 8_388_608.0
        }
        ENCODING_PCM_32BIT => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / 2_147_483_648.0,
        _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
    }
}

fn convert_loop(sh: &Shared, engine: &Engine, mut proc: TileProcessor, linear_gain: f64) {
    let _ftz = ScopedFtzDaz::new();
    let cap = proc.input_frame_capacity();
    let mult = proc.rate_multiplier();
    // One dispatch yields at most `output_frame_capacity` baseband frames, `mult / 8` bytes per
    // channel each; the packer adds at most one carried frame.
    let max_out = 2 * proc.output_frame_capacity() * mult / 8 + M7_FRAME;
    let room = max_out.min(engine.capacity());
    let (mut l, mut r) = (vec![0.0; cap], vec![0.0; cap]);
    let gains = vec![FrameGain::Preserve; cap];
    let mut out = vec![0u8; max_out];
    let mut packer = M7Stream::new(false);
    let mut seen = 0;
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        sh.error.store(ERR_DSP, Ordering::Relaxed);
        eprintln!("audigma-convert: {what}: {e}");
    };
    loop {
        let (n, silence, generation);
        {
            let mut st = sh.lock();
            while !st.quit && st.generation == seen && st.ring.len == 0 && st.tail_left == 0 {
                st = sh.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.quit {
                return;
            }
            if st.generation != seen {
                seen = st.generation;
                drop(st);
                if let Err(e) = proc.reset() {
                    return fail("reset", &e);
                }
                packer.clear();
                continue;
            }
            generation = st.generation;
            silence = st.ring.len == 0;
            if silence {
                n = st.tail_left.min(cap);
                l[..n].fill(0.0);
                r[..n].fill(0.0);
            } else {
                n = st.ring.len.min(cap);
                st.ring.pop_into(&mut l[..n], &mut r[..n]);
            }
            st.in_flight = n;
        }
        // Wait for room in the writer's ring (it drains at the DSD rate; paused, it does not).
        let stale = || {
            let st = sh.lock();
            st.quit || st.generation != generation
        };
        let mut abandoned = false;
        while !engine.wait_free(room, ROOM_WAIT) {
            if stale() {
                abandoned = true;
                break;
            }
        }
        let mut k = 0;
        if !abandoned {
            let baseband = match proc.pcm_baseband_frames(n) {
                Ok(b) => b,
                Err(e) => return fail("input", &e),
            };
            let controls = InputTileControls {
                frame_gains: &gains[..baseband],
                linear_gain,
                drift_ppm: 0.0,
            };
            if let Err(e) = proc.process_input_pcm(&[&l[..n], &r[..n]], controls) {
                return fail("process", &e);
            }
            sh.overloads.store(proc.total_overload_count(), Ordering::Relaxed);
            match packer.pack(proc.channel_dsd(0), proc.channel_dsd(1), &mut out) {
                Ok(m) => k = m,
                Err(e) => return fail("pack", &e),
            }
        }
        let mut st = sh.lock();
        st.in_flight = 0;
        if st.generation == generation && !abandoned {
            if k > 0 && !engine.push_m7(&out[..k]) {
                // Cannot happen: only this thread adds to the ring and room was checked.
                return fail("push", &"writer ring full");
            }
            if silence {
                st.tail_left -= n;
            }
        }
    }
}

/// The CPUs with the highest maximum frequency (the big cluster), empty if all are equal or
/// unreadable.
fn fast_cpus() -> Vec<usize> {
    let freqs: Vec<(usize, u64)> = (0..64)
        .filter_map(|c| {
            let p = format!("/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq");
            let f = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
            Some((c, f))
        })
        .collect();
    let max = freqs.iter().map(|&(_, f)| f).max().unwrap_or(0);
    if freqs.iter().all(|&(_, f)| f == max) {
        return Vec::new();
    }
    freqs.into_iter().filter(|&(_, f)| f == max).map(|(c, _)| c).collect()
}

/// Restricts the calling thread to `cpus` (no-op when empty); the result is counted in `sh`.
fn pin(cpus: &[usize], sh: &Shared) {
    if cpus.is_empty() {
        return;
    }
    // SAFETY: a zeroed cpu_set_t is a valid empty set; the call only reads it.
    let ok = unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cpus {
            libc::CPU_SET(c, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0
    };
    let counter = if ok { &sh.pin_ok } else { &sh.pin_failed };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Two workers (one per channel) on the fast cores, blocking on a futex instead of spinning.
fn worker_setup(fast: &[usize], sh: &Arc<Shared>) -> WorkerSetup {
    let placement = ThreadPlacement {
        cpus: (!fast.is_empty()).then(|| fast.to_vec()),
        rt_priority: None,
        spin: false,
    };
    let (cpus, sh) = (fast.to_vec(), Arc::clone(sh));
    WorkerSetup {
        workers: None,
        placements: vec![placement; 2],
        hook: Some(Arc::new(move |_, _| {
            pin(&cpus, &sh);
            Ok(())
        })),
        require_hook_ok: false,
        waiter: Waiter::new(futex::wait, futex::wake_one),
    }
}

/// Process-private futex, as `audigma_io::wait` does on Linux.
mod futex {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    const FUTEX_WAIT_PRIVATE: libc::c_int = libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG;
    const FUTEX_WAKE_PRIVATE: libc::c_int = libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG;

    pub(super) fn wait(atomic: &AtomicU32, expected: u32, timeout: Duration) {
        let ts = libc::timespec {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_nsec: timeout.subsec_nanos() as libc::c_long,
        };
        // EAGAIN, EINTR and ETIMEDOUT all mean "re-check", which every caller does.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                atomic.as_ptr(),
                FUTEX_WAIT_PRIVATE,
                expected,
                &ts as *const libc::timespec,
            );
        }
    }

    pub(super) fn wake_one(atomic: &AtomicU32) {
        unsafe {
            libc::syscall(libc::SYS_futex, atomic.as_ptr(), FUTEX_WAKE_PRIVATE, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{Output, writer_loop};
    use std::f64::consts::PI;

    /// Collects every byte the writer hands to the track.
    struct Sink(Vec<u8>, Arc<Mutex<Vec<u8>>>);

    impl Output for Sink {
        fn buf(&mut self) -> &mut [u8] {
            &mut self.0
        }
        fn write(&mut self, len: usize) -> i32 {
            self.1.lock().unwrap().extend_from_slice(&self.0[..len]);
            len as i32
        }
    }

    fn rig(dsd_rate: u32, input_rate: u32) -> (Arc<Engine>, Converter, Arc<Mutex<Vec<u8>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let engine = Arc::new(
            Engine::spawn(1 << 20, move |sh| writer_loop(sh, &mut Sink(vec![0; 65536], g)))
                .unwrap(),
        );
        let conv = Converter::new(
            engine.clone(),
            dsd_rate,
            input_rate,
            &format!("multiplier=dsd{}\nfamily=44100", dsd_rate / 2_822_400 * 64),
        )
        .unwrap();
        (engine, conv, got)
    }

    /// Interleaved float stereo: `fl` Hz on the left, `fr` Hz on the right, -6 dBFS.
    fn sine_f32(frames: usize, rate: u32, fl: f64, fr: f64) -> Vec<u8> {
        let s = |f: f64, i: usize| (0.5 * (2.0 * PI * f * i as f64 / rate as f64).sin()) as f32;
        (0..frames)
            .flat_map(|i| [s(fl, i).to_le_bytes(), s(fr, i).to_le_bytes()].concat())
            .collect()
    }

    /// Splits M7 (`L1 L0 R1 R0 L3 L2 R3 R2`) back into chronological MSB-first channel bytes.
    fn unpack_m7(m7: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for f in m7.chunks_exact(M7_FRAME) {
            l.extend([f[1], f[0], f[5], f[4]]);
            r.extend([f[3], f[2], f[7], f[6]]);
        }
        (l, r)
    }

    /// 1-bit stream to PCM by summing `bits` bits per sample (a box filter).
    fn decimate(dsd: &[u8], bits: usize) -> Vec<f64> {
        let ones: Vec<f64> = dsd
            .iter()
            .flat_map(|b| (0..8).rev().map(move |i| ((b >> i) & 1) as f64 * 2.0 - 1.0))
            .collect();
        ones.chunks_exact(bits).map(|c| c.iter().sum::<f64>() / bits as f64).collect()
    }

    /// Power at `f` Hz (Goertzel).
    fn power(x: &[f64], f: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * f / rate;
        let (mut s1, mut s2) = (0.0, 0.0);
        for &v in x {
            let s = v + 2.0 * w.cos() * s1 - s2;
            (s2, s1) = (s1, s);
        }
        s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2
    }

    fn wait_for(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(20), "timeout");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn push_all(conv: &Converter, mut src: &[u8]) {
        while !src.is_empty() {
            let n = conv.push(src, ENCODING_PCM_FLOAT, 2, 1.0).unwrap();
            src = &src[n..];
            if n == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Every input frame reaches the output, plus the tail: an album's last track must not lose
    /// its end. 1 s at 44.1 kHz to DSD64 is 64 bits per channel per frame.
    #[test]
    fn end_of_stream_converts_all_input_and_the_tail() {
        let (engine, mut conv, got) = rig(2_822_400, 44_100);
        engine.set_running(true);
        push_all(&conv, &sine_f32(44_100, 44_100, 1000.0, 1000.0));
        conv.end_of_stream();
        wait_for(|| conv.pending_bytes() == 0 && engine.buffered() == 0);
        let out = got.lock().unwrap().len();
        let input_bytes = 44_100 * 64 * 2 / 8;
        let tail = conv.tail_frames * 64 * 2 / 8;
        assert!(out % M7_FRAME == 0);
        assert!(out >= input_bytes + tail - M7_FRAME && out <= input_bytes + tail, "{out}");
        assert_eq!(conv.error(), 0);
        assert!(conv.destroy(Duration::from_secs(2)));
        assert!(engine.destroy(Duration::from_secs(2)));
    }

    /// Through the 48 -> 44.1 kHz SRC to DSD256: only what was pushed after a reset comes out,
    /// and each channel carries its own tone (channel order, bit order and M7 layout).
    #[test]
    fn reset_drops_queued_input_and_channels_keep_their_tones() {
        let (engine, mut conv, got) = rig(11_289_600, 48_000);
        // Writer stopped: queue input, then reset it away before anything plays.
        push_all(&conv, &sine_f32(4_800, 48_000, 3000.0, 3000.0));
        conv.reset();
        wait_for(|| conv.pending_bytes() == 0);
        engine.reset();
        engine.set_running(true);
        push_all(&conv, &sine_f32(48_000, 48_000, 1000.0, 2000.0));
        conv.end_of_stream();
        wait_for(|| conv.pending_bytes() == 0 && engine.buffered() == 0);
        let out = got.lock().unwrap();
        let one_second = 11_289_600 * 2 / 8;
        assert!(
            out.len() > one_second && out.len() < one_second * 102 / 100,
            "{}",
            out.len()
        );
        let (l, r) = unpack_m7(&out);
        // 256 bits per sample: 44.1 kHz. Skip the start and the tail.
        let (l, r) = (decimate(&l, 256), decimate(&r, 256));
        let (l, r) = (&l[4410..39690], &r[4410..39690]);
        let db = |a: f64, b: f64| 10.0 * (a / b).log10();
        let rate = 44_100.0;
        assert!(db(power(l, 1000.0, rate), power(l, 2000.0, rate)) > 40.0);
        assert!(db(power(r, 2000.0, rate), power(r, 1000.0, rate)) > 40.0);
        assert!(db(power(l, 1000.0, rate), power(l, 3000.0, rate)) > 40.0, "reset leaked");
        drop(out);
        assert!(conv.destroy(Duration::from_secs(2)));
        assert!(engine.destroy(Duration::from_secs(2)));
    }
}
