// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! JNI entry points for the writer engine and the PCM to DSD converter. Java side: `AudigmaNative`.
//!
//! The handle is the address of a boxed [`Native`]; only `AudigmaNative`'s Kotlin owner calls these,
//! on one object, and `nativeDestroy` is the last call (it frees the handle).

use crate::convert::Converter;
use crate::writer::{ERR_EXCEPTION, Engine, Output, writer_loop};
use jni::objects::{GlobalRef, JByteBuffer, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use std::sync::Arc;
use std::time::Duration;

/// `AudioTrack.WRITE_BLOCKING`.
const WRITE_BLOCKING: jint = 0;
const SIG_WRITE: &str = "(Ljava/nio/ByteBuffer;II)I";

/// One output: the writer, plus the converter in front of it for PCM input.
struct Native {
    engine: Arc<Engine>,
    conv: Option<Converter>,
}

struct TrackOutput<'a, 'e> {
    env: &'a mut JNIEnv<'e>,
    track: GlobalRef,
    java_buf: GlobalRef,
    ptr: *mut u8,
    cap: usize,
}

impl Output for TrackOutput<'_, '_> {
    fn buf(&mut self) -> &mut [u8] {
        // SAFETY: the direct buffer is kept alive by `java_buf` and only this thread touches it.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.cap) }
    }

    fn write(&mut self, len: usize) -> i32 {
        let env = &mut *self.env;
        // The blocking write advances the buffer position; always start from 0.
        let ok = env
            .call_method(self.java_buf.as_obj(), "clear", "()Ljava/nio/Buffer;", &[])
            .is_ok();
        let r = ok
            .then(|| {
                env.call_method(
                    self.track.as_obj(),
                    "write",
                    SIG_WRITE,
                    &[
                        JValue::Object(self.java_buf.as_obj()),
                        JValue::Int(len as jint),
                        JValue::Int(WRITE_BLOCKING),
                    ],
                )
            })
            .and_then(|r| r.ok())
            .and_then(|v| v.i().ok());
        if env.exception_check().unwrap_or(true) {
            let _ = env.exception_clear();
            return ERR_EXCEPTION;
        }
        r.unwrap_or(ERR_EXCEPTION)
    }
}

fn native<'a>(handle: jlong) -> &'a Native {
    // SAFETY: see the module comment.
    unsafe { &*(handle as *const Native) }
}

/// The direct buffer range `off..off + len` of `src`, `None` if invalid.
fn direct<'a>(env: &JNIEnv, src: &JByteBuffer, off: jint, len: jint) -> Option<&'a [u8]> {
    let (Ok(p), Ok(cap), Ok(off), Ok(len)) = (
        env.get_direct_buffer_address(src),
        env.get_direct_buffer_capacity(src),
        usize::try_from(off),
        usize::try_from(len),
    ) else {
        return None;
    };
    if p.is_null() || off.checked_add(len).is_none_or(|e| e > cap) {
        return None;
    }
    // SAFETY: the range lies inside a direct buffer the caller keeps alive for this call.
    Some(unsafe { std::slice::from_raw_parts(p.add(off), len) })
}

/// `static native long nativeCreate(AudioTrack track, ByteBuffer scratch, int ringBytes,
/// int dsdRate, int pcmRate, String options)`. `scratch` is a direct buffer, a multiple of 8192 bytes, used as
/// the track's write buffer. `pcmRate` 0: the input is raw DSF ([`nativePush`]); otherwise PCM
/// at that rate is converted to `dsdRate` ([`nativePushPcm`]) under `options` (see
/// [`crate::options`]; ignored for raw DSD). Returns 0 on failure, with an
/// `IllegalStateException` pending when the converter could not be set up.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    track: JObject,
    scratch: JByteBuffer,
    ring_bytes: jint,
    dsd_rate: jint,
    pcm_rate: jint,
    options: JString,
) -> jlong {
    let (Ok(vm), Ok(track), Ok(buf), Ok(ptr), Ok(cap)) = (
        env.get_java_vm(),
        env.new_global_ref(&track),
        env.new_global_ref(&scratch),
        env.get_direct_buffer_address(&scratch),
        env.get_direct_buffer_capacity(&scratch),
    ) else {
        return 0;
    };
    if ptr.is_null() || cap < crate::m7::DSF_BLOCK_PAIR || ring_bytes <= 0 {
        return 0;
    }
    let ptr = ptr as usize; // raw pointers are not Send
    let run = move |sh: &crate::writer::Shared| {
        let vm: JavaVM = vm;
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        let mut out = TrackOutput {
            env: &mut env,
            track,
            java_buf: buf,
            ptr: ptr as *mut u8,
            cap,
        };
        writer_loop(sh, &mut out);
    };
    let Ok(engine) = Engine::spawn(ring_bytes as usize, run).map(Arc::new) else {
        return 0;
    };
    let conv = if pcm_rate > 0 {
        let options = match env.get_string(&options) {
            Ok(o) => String::from(o),
            Err(_) => {
                engine.destroy(Duration::from_secs(1));
                return 0;
            }
        };
        match Converter::new(engine.clone(), dsd_rate as u32, pcm_rate as u32, &options) {
            Ok(c) => Some(c),
            Err(e) => {
                // The writer never ran, so nothing can block it.
                engine.destroy(Duration::from_secs(1));
                let _ = env.throw_new("java/lang/IllegalStateException", e);
                return 0;
            }
        }
    } else {
        None
    };
    Box::into_raw(Box::new(Native { engine, conv })) as jlong
}

/// `static native int nativeResolveDsdRate(String options, int pcmRate)`: the DSD bit rate in Hz
/// audigma plays `pcmRate` Hz PCM at under `options` (see [`crate::options`]). Throws
/// `IllegalStateException` with audigma's reason when the options or the rate are rejected.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeResolveDsdRate(
    mut env: JNIEnv,
    _class: JClass,
    options: JString,
    pcm_rate: jint,
) -> jint {
    let Ok(options) = env.get_string(&options).map(String::from) else {
        return 0;
    };
    match u32::try_from(pcm_rate)
        .map_err(|e| e.to_string())
        .and_then(|r| crate::options::dsd_rate(&options, r))
    {
        Ok(rate) => rate as jint,
        Err(e) => {
            let _ = env.throw_new("java/lang/IllegalStateException", e);
            0
        }
    }
}

/// `static native int nativePush(long h, ByteBuffer src, int off, int len)`: packs raw DSF block
/// pairs (optionally ending in one short pair, see `DsfPacker`) into the ring without blocking,
/// all or nothing. Returns `len` when taken, `0` when the ring lacks room, -1 for a bad range or
/// a malformed length.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativePush(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    src: JByteBuffer,
    off: jint,
    len: jint,
) -> jint {
    let Some(s) = direct(&env, &src, off, len) else {
        return -1;
    };
    native(handle).engine.push(s).map_or(-1, |n| n as jint)
}

/// `static native int nativePushPcm(long h, ByteBuffer src, int off, int len, int encoding,
/// int channels, float gain)`: queues interleaved PCM (Media3 `encoding`, 1 or 2 channels)
/// scaled by `gain` for conversion, as many whole frames as fit, without blocking. Returns the
/// bytes taken, -1 for a bad range, an unsupported format, or an output without a converter.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativePushPcm(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    src: JByteBuffer,
    off: jint,
    len: jint,
    encoding: jint,
    channels: jint,
    gain: jfloat,
) -> jint {
    let (Some(s), Some(conv)) = (direct(&env, &src, off, len), native(handle).conv.as_ref())
    else {
        return -1;
    };
    conv.push(s, encoding, channels.max(0) as usize, gain)
        .map_or(-1, |n| n as jint)
}

/// End of the input: the converter plays out what it holds plus its filters' tail. No-op for
/// raw DSD.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeEndOfStream(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if let Some(c) = &native(handle).conv {
        c.end_of_stream();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeSetRunning(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    running: jboolean,
) {
    native(handle).engine.set_running(running != 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeAwaitIdle(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    timeout_ms: jint,
) -> jboolean {
    native(handle)
        .engine
        .await_idle(Duration::from_millis(timeout_ms.max(0) as u64)) as jboolean
}

/// Drops all queued audio (converter first, see `convert.rs`), the written counter and errors.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeReset(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    let n = native(handle);
    if let Some(c) = &n.conv {
        c.reset();
        c.clear_error();
    }
    n.engine.reset();
    n.engine.clear_error();
}

/// M7 bytes queued and not yet accepted by the track: the ring, unaccepted packed bytes, and an
/// estimate for PCM the converter has not converted yet.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeBuffered(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    let n = native(handle);
    let conv = n.conv.as_ref().map_or(0, Converter::pending_bytes);
    (n.engine.buffered() + conv) as jlong
}

/// Free space in the ring.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    native(handle).engine.free() as jlong
}

/// M7 bytes accepted by the track since the last reset.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeWritten(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    native(handle).engine.written() as jlong
}

/// Filter group delay of the converter in DSD samples per channel (0 for raw DSD): the output
/// lags the input by this much.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeDelaySamples(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    native(handle)
        .conv
        .as_ref()
        .map_or(0, |c| c.group_delay_samples() as jlong)
}

/// Last error: `0` none, `-1000` the track call raised, `-2000` the converter's DSP failed, other
/// negatives are the track's.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeError(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    let n = native(handle);
    match n.engine.error() {
        0 => n.conv.as_ref().map_or(0, Converter::error),
        e => e,
    }
}

/// `[clipped samples, modulator overloads, peak since the last call in 1/1000 of full scale]`
/// of the converter; zeros for raw DSD. Reading resets the peak.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeStats<'a>(
    env: JNIEnv<'a>,
    _class: JClass<'a>,
    handle: jlong,
) -> jni::sys::jlongArray {
    let (clipped, overloads, peak) = native(handle).conv.as_ref().map_or((0, 0, 0.0), Converter::stats);
    let v = [clipped as jlong, overloads as jlong, (peak * 1000.0).round() as jlong];
    let Ok(a) = env.new_long_array(3) else {
        return std::ptr::null_mut();
    };
    if env.set_long_array_region(&a, 0, &v).is_err() {
        return std::ptr::null_mut();
    }
    a.into_raw()
}

/// Converter configuration and thread placement for logs; empty for raw DSD.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeDescribe(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let s = native(handle)
        .conv
        .as_ref()
        .map_or_else(String::new, Converter::describe);
    env.new_string(s)
        .map_or(std::ptr::null_mut(), |s| s.into_raw())
}

/// Stops and joins the converter and the writer, then frees the handle. Pause or stop the track
/// first so a blocked write returns. Returns `false` if a thread did not exit in `timeout_ms`;
/// it is then left detached.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    timeout_ms: jint,
) -> jboolean {
    // SAFETY: `handle` came from `nativeCreate` and is not used after this call.
    let mut n = unsafe { Box::from_raw(handle as *mut Native) };
    let timeout = Duration::from_millis(timeout_ms.max(0) as u64);
    // The converter first: it may wait for room in the writer's ring. A detached thread keeps
    // its own `Arc`s of the shared state.
    let conv_ok = n.conv.as_mut().is_none_or(|c| c.destroy(timeout));
    (n.engine.destroy(timeout) && conv_ok) as jboolean
}
