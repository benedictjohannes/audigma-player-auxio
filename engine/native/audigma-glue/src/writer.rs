// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! Single writer thread between the JVM sink and the `AudioTrack` (`docs/playback-engine-design.md`,
//! "Threading and control").
//!
//! The sink pushes raw DSF block pairs; the push packs them to M7 into a byte ring without
//! blocking ([`DsfPacker`] carries partial frames between pushes). For PCM input the converter
//! (`convert.rs`) is the producer instead: it pushes packed M7 ([`Engine::push_m7`]) and waits for
//! room ([`Engine::wait_free`]). The writer thread hands whole M7 frames to an [`Output`] (the JNI
//! `AudioTrack.write`). Control calls come from the JVM playback thread:
//!
//! - [`Engine::set_running`]`(false)` stops the writer from starting new writes. A write that is
//!   already blocked inside the track returns only when the JVM pauses, stops or flushes the track,
//!   so the JVM must do that next, then call [`Engine::await_idle`].
//! - [`Engine::reset`] drops the ring, any packed bytes not yet accepted by the track, and the
//!   position counter. It is safe at any time; call it between pausing and flushing the track's
//!   consumer, and only then resume.
//! - Packed bytes the track did not accept (a short write) stay pending and are written first on
//!   the next run. A write returning `0` means the track is paused/stopped or full: the writer
//!   waits briefly and retries while it is running.

use crate::m7::{DsfPacker, M7_FRAME};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Error code reported when the output call raised an exception (track released or invalid).
pub const ERR_EXCEPTION: i32 = -1000;

/// Wait between retries when the track accepts nothing (`write` returned `0`).
const RETRY_WAIT: Duration = Duration::from_millis(2);

/// Destination of packed M7 bytes. `write` is called only from the writer thread.
pub trait Output {
    /// Scratch buffer the packed bytes are placed in; its length is the largest write.
    fn buf(&mut self) -> &mut [u8];
    /// Writes `buf()[..len]` (blocking). Returns the bytes accepted (`0..=len`), `0` when the
    /// track accepts nothing right now, or a negative error code.
    fn write(&mut self, len: usize) -> i32;
}

struct Ring {
    data: Vec<u8>,
    head: usize,
    len: usize,
}

impl Ring {
    fn new(cap: usize) -> Ring {
        Ring {
            data: vec![0; cap],
            head: 0,
            len: 0,
        }
    }

    fn free(&self) -> usize {
        self.data.len() - self.len
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn push(&mut self, src: &[u8]) -> usize {
        let n = src.len().min(self.free());
        let cap = self.data.len();
        let tail = (self.head + self.len) % cap;
        let first = n.min(cap - tail);
        self.data[tail..tail + first].copy_from_slice(&src[..first]);
        self.data[..n - first].copy_from_slice(&src[first..n]);
        self.len += n;
        n
    }

    fn pop_into(&mut self, dst: &mut [u8]) {
        let n = dst.len();
        debug_assert!(n <= self.len);
        let cap = self.data.len();
        let first = n.min(cap - self.head);
        dst[..first].copy_from_slice(&self.data[self.head..self.head + first]);
        dst[first..].copy_from_slice(&self.data[..n - first]);
        self.head = (self.head + n) % cap;
        self.len -= n;
    }
}

struct State {
    ring: Ring,
    /// Packed bytes at the start of the output buffer not yet accepted by the track.
    pending_off: usize,
    pending_len: usize,
    running: bool,
    quit: bool,
    /// The writer is parked (not running, nothing to do, or quitting) and holds no write.
    idle: bool,
    /// Bumped by [`Engine::reset`]; a write that started before it must not update the state.
    generation: u64,
    /// M7 bytes accepted by the track since the last reset.
    written: u64,
}

pub struct Shared {
    state: Mutex<State>,
    /// Wakes the writer: data, run state, reset, quit.
    wake: Condvar,
    /// Wakes `await_idle`.
    idle_cv: Condvar,
    /// Wakes [`Engine::wait_free`]: the track accepted bytes, reset, quit.
    space_cv: Condvar,
    error: AtomicI32,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The writer loop. Runs on the writer thread until [`Engine::destroy`].
pub fn writer_loop(sh: &Shared, out: &mut dyn Output) {
    let cap = out.buf().len() / M7_FRAME * M7_FRAME;
    assert!(cap > 0, "output buffer smaller than one M7 frame");
    loop {
        let (len, off, generation);
        {
            let mut st = sh.lock();
            loop {
                if st.quit {
                    st.idle = true;
                    sh.idle_cv.notify_all();
                    return;
                }
                let ready = st.pending_len > 0 || st.ring.len >= M7_FRAME;
                if st.running && ready {
                    st.idle = false;
                    break;
                }
                st.idle = true;
                sh.idle_cv.notify_all();
                st = sh.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            generation = st.generation;
            if st.pending_len == 0 {
                // The ring holds whole M7 frames only.
                let n = st.ring.len.min(cap) / M7_FRAME * M7_FRAME;
                st.ring.pop_into(&mut out.buf()[..n]);
                st.pending_off = 0;
                st.pending_len = n;
            }
            len = st.pending_len;
            off = st.pending_off;
            st.pending_off = 0; // the data is moved to the start of `buf` just below
        }
        // The output takes the data from the start of `buf`; move an earlier remainder there.
        if off > 0 {
            out.buf().copy_within(off..off + len, 0);
        }
        let r = out.write(len);
        let mut st = sh.lock();
        if st.generation != generation {
            continue;
        }
        if r > 0 {
            let r = (r as usize).min(len);
            // The data was moved to the start of `buf`; the unaccepted rest sits at `r..`.
            st.pending_off = r;
            st.pending_len -= r;
            st.written += r as u64;
            sh.space_cv.notify_all();
        } else if r < 0 {
            sh.error.store(r, Ordering::Relaxed);
            st.running = false;
        } else if st.running && !st.quit {
            let _ = sh.wake.wait_timeout(st, RETRY_WAIT);
        }
    }
}

/// Handle to the writer thread and its shared state.
pub struct Engine {
    shared: Arc<Shared>,
    /// Push side, used only from the JVM playback thread. Lock order: `producer`, then `state`.
    producer: Mutex<Producer>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct Producer {
    packer: DsfPacker,
    /// Packed bytes of the current push; only grows, so steady state does not allocate.
    scratch: Vec<u8>,
}

impl Engine {
    /// Starts a stopped writer with a ring of `ring_bytes`. `run` executes on the writer thread
    /// and must call [`writer_loop`] with its output.
    pub fn spawn<F>(ring_bytes: usize, run: F) -> std::io::Result<Engine>
    where
        F: FnOnce(&Shared) + Send + 'static,
    {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ring: Ring::new(ring_bytes),
                pending_off: 0,
                pending_len: 0,
                running: false,
                quit: false,
                idle: false,
                generation: 0,
                written: 0,
            }),
            wake: Condvar::new(),
            idle_cv: Condvar::new(),
            space_cv: Condvar::new(),
            error: AtomicI32::new(0),
        });
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("audigma-writer".into())
            .spawn(move || run(&sh))?;
        Ok(Engine {
            shared,
            producer: Mutex::new(Producer::default()),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Packs `src` (DSF block pairs, see [`DsfPacker`]) to M7 into the ring without blocking, all
    /// or nothing: `Some(src.len())` when taken, `Some(0)` when the ring lacks room, `None` for a
    /// malformed length or a push that can never fit.
    pub fn push(&self, src: &[u8]) -> Option<usize> {
        let mut p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
        let need = p.packer.packed_len(src.len())?;
        {
            let st = self.shared.lock();
            if need > st.ring.data.len() {
                return None;
            }
            // Only this thread adds to the ring, so the room checked here stays available.
            if need > st.ring.free() {
                return Some(0);
            }
        }
        let Producer { packer, scratch } = &mut *p;
        if scratch.len() < need {
            scratch.resize(need, 0);
        }
        let n = packer.pack(src, &mut scratch[..need]).ok()?;
        if n > 0 {
            let mut st = self.shared.lock();
            let pushed = st.ring.push(&scratch[..n]);
            debug_assert_eq!(pushed, n);
            self.shared.wake.notify_all();
        }
        Some(src.len())
    }

    /// Copies already packed M7 bytes (whole frames) into the ring without blocking, all or
    /// nothing: `false` when the ring lacks room. For the converter, which packs on its own thread.
    pub fn push_m7(&self, m7: &[u8]) -> bool {
        debug_assert!(m7.len().is_multiple_of(M7_FRAME));
        let _p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.shared.lock();
        if m7.len() > st.ring.free() {
            return false;
        }
        st.ring.push(m7);
        self.shared.wake.notify_all();
        true
    }

    /// Waits until the ring has `n` bytes free; `false` on timeout or once the writer quits.
    pub fn wait_free(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.lock();
        loop {
            if st.ring.free() >= n {
                return true;
            }
            let left = deadline.checked_duration_since(Instant::now());
            let (Some(left), false) = (left, st.quit) else {
                return false;
            };
            st = self
                .shared
                .space_cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Ring capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.shared.lock().ring.data.len()
    }

    pub fn set_running(&self, running: bool) {
        let mut st = self.shared.lock();
        st.running = running;
        self.shared.wake.notify_all();
    }

    /// Waits until the writer is parked and not running; `false` on timeout (a write is still
    /// blocked in the track).
    pub fn await_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.lock();
        loop {
            if st.idle && !st.running {
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            st = self
                .shared
                .idle_cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub fn reset(&self) {
        let mut p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
        p.packer.clear();
        let mut st = self.shared.lock();
        st.ring.clear();
        st.pending_len = 0;
        st.pending_off = 0;
        st.written = 0;
        st.generation += 1;
        self.shared.wake.notify_all();
        self.shared.space_cv.notify_all();
    }

    /// Packed bytes in the ring plus packed bytes not yet accepted by the track.
    pub fn buffered(&self) -> u64 {
        let st = self.shared.lock();
        (st.ring.len + st.pending_len) as u64
    }

    /// Free space in the ring.
    pub fn free(&self) -> u64 {
        self.shared.lock().ring.free() as u64
    }

    /// M7 bytes accepted by the track since the last reset.
    pub fn written(&self) -> u64 {
        self.shared.lock().written
    }

    /// Last output error code (`0` none); cleared by [`Engine::clear_error`].
    pub fn error(&self) -> i32 {
        self.shared.error.load(Ordering::Relaxed)
    }

    pub fn clear_error(&self) {
        self.shared.error.store(0, Ordering::Relaxed);
    }

    /// Stops and joins the writer. The caller must first have unblocked any write (pause/stop the
    /// track). Returns `false`, leaving the thread running, if it did not exit within `timeout`.
    pub fn destroy(&self, timeout: Duration) -> bool {
        {
            let mut st = self.shared.lock();
            st.quit = true;
            st.running = false;
            self.shared.wake.notify_all();
            self.shared.space_cv.notify_all();
        }
        let deadline = Instant::now() + timeout;
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = thread.as_ref() else {
            return true;
        };
        while !t.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = thread.take().map(|t| t.join());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m7::{DSF_BLOCK_PAIR, pack_m7_dsf_blocks};

    /// Scripted output: records every accepted byte; `limit` caps each write (0 = refuse).
    struct Mock {
        buf: Vec<u8>,
        got: Arc<Mutex<Vec<u8>>>,
        limit: Arc<Mutex<usize>>,
    }

    impl Output for Mock {
        fn buf(&mut self) -> &mut [u8] {
            &mut self.buf
        }
        fn write(&mut self, len: usize) -> i32 {
            let n = len.min(*self.limit.lock().unwrap());
            self.got.lock().unwrap().extend_from_slice(&self.buf[..n]);
            if n == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            n as i32
        }
    }

    struct Rig {
        engine: Engine,
        got: Arc<Mutex<Vec<u8>>>,
        limit: Arc<Mutex<usize>>,
    }

    fn rig(ring: usize, buf: usize) -> Rig {
        let got = Arc::new(Mutex::new(Vec::new()));
        let limit = Arc::new(Mutex::new(usize::MAX));
        let (g, l) = (got.clone(), limit.clone());
        let engine = Engine::spawn(ring, move |sh| {
            writer_loop(
                sh,
                &mut Mock {
                    buf: vec![0; buf],
                    got: g,
                    limit: l,
                },
            )
        })
        .unwrap();
        Rig { engine, got, limit }
    }

    fn input(blocks: usize, seed: u32) -> Vec<u8> {
        (0..(blocks * DSF_BLOCK_PAIR) as u32)
            .map(|i| (i.wrapping_add(seed).wrapping_mul(2654435761) >> 11) as u8)
            .collect()
    }

    fn m7(src: &[u8]) -> Vec<u8> {
        let mut d = vec![0; src.len()];
        pack_m7_dsf_blocks(src, &mut d).unwrap();
        d
    }

    fn wait_for(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timeout");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn streams_in_order_through_short_and_refused_writes() {
        let r = rig(8 * DSF_BLOCK_PAIR, 2 * DSF_BLOCK_PAIR);
        let src = input(20, 1);
        r.engine.set_running(true);
        let mut off = 0;
        for i in 0.. {
            if off == src.len() {
                break;
            }
            // Alternate refused writes and short writes that split a block pair.
            *r.limit.lock().unwrap() = [0, 4097, 3000][i % 3];
            let end = (off + 2 * DSF_BLOCK_PAIR).min(src.len());
            off += r.engine.push(&src[off..end]).unwrap();
            std::thread::sleep(Duration::from_micros(300));
        }
        *r.limit.lock().unwrap() = 4097;
        wait_for(|| r.engine.written() == src.len() as u64);
        assert_eq!(*r.got.lock().unwrap(), m7(&src));
        assert!(r.engine.destroy(Duration::from_secs(2)));
    }

    #[test]
    fn pause_stops_writes_and_resume_loses_nothing() {
        let r = rig(64 * DSF_BLOCK_PAIR, 2 * DSF_BLOCK_PAIR);
        let src = input(40, 2);
        assert_eq!(r.engine.push(&src), Some(src.len()));
        r.engine.set_running(true);
        wait_for(|| r.engine.written() > 0);
        r.engine.set_running(false);
        assert!(r.engine.await_idle(Duration::from_secs(2)));
        let n = r.got.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(r.got.lock().unwrap().len(), n, "write after pause");
        r.engine.set_running(true);
        wait_for(|| r.engine.written() == src.len() as u64);
        assert_eq!(*r.got.lock().unwrap(), m7(&src));
        assert!(r.engine.destroy(Duration::from_secs(2)));
    }

    #[test]
    fn reset_drops_old_audio_and_counter() {
        let r = rig(64 * DSF_BLOCK_PAIR, 2 * DSF_BLOCK_PAIR);
        let old = input(40, 3);
        let new = input(6, 4);
        *r.limit.lock().unwrap() = 0; // track full: a packed chunk stays pending
        assert_eq!(r.engine.push(&old), Some(old.len()));
        r.engine.set_running(true);
        std::thread::sleep(Duration::from_millis(20)); // writer has packed a chunk and is retrying
        assert_eq!(r.engine.written(), 0);
        r.engine.set_running(false);
        assert!(r.engine.await_idle(Duration::from_secs(2)));
        r.engine.reset();
        assert_eq!((r.engine.buffered(), r.engine.written()), (0, 0));
        r.got.lock().unwrap().clear();
        *r.limit.lock().unwrap() = usize::MAX;
        assert_eq!(r.engine.push(&new), Some(new.len()));
        r.engine.set_running(true);
        wait_for(|| r.engine.written() == new.len() as u64);
        assert_eq!(*r.got.lock().unwrap(), m7(&new));
        assert!(r.engine.destroy(Duration::from_secs(2)));
    }

    #[test]
    fn push_is_all_or_nothing_and_never_blocks() {
        let r = rig(4 * DSF_BLOCK_PAIR, DSF_BLOCK_PAIR);
        let src = input(10, 5);
        // Not running: the ring fills up, then refuses without taking a part.
        assert_eq!(
            r.engine.push(&src[..4 * DSF_BLOCK_PAIR]),
            Some(4 * DSF_BLOCK_PAIR)
        );
        assert_eq!(r.engine.push(&src[..DSF_BLOCK_PAIR]), Some(0));
        assert_eq!(r.engine.buffered(), 4 * DSF_BLOCK_PAIR as u64);
        assert_eq!(r.engine.push(&src), None, "larger than the ring");
        assert!(r.engine.destroy(Duration::from_secs(2)));
    }

    #[test]
    fn output_error_stops_the_writer() {
        struct Bad(Vec<u8>);
        impl Output for Bad {
            fn buf(&mut self) -> &mut [u8] {
                &mut self.0
            }
            fn write(&mut self, _: usize) -> i32 {
                ERR_EXCEPTION
            }
        }
        let e = Engine::spawn(4 * DSF_BLOCK_PAIR, |sh| {
            writer_loop(sh, &mut Bad(vec![0; DSF_BLOCK_PAIR]))
        })
        .unwrap();
        e.push(&input(1, 6));
        e.set_running(true);
        wait_for(|| e.error() == ERR_EXCEPTION);
        assert!(e.await_idle(Duration::from_secs(2)));
        assert!(e.destroy(Duration::from_secs(2)));
    }
}
