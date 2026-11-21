// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! JNI glue for the Android player. Class: `com.encelerate.audigmaplayer.audio.AudigmaNative`.

pub mod convert;
pub mod jni_api;
pub mod m7;
pub mod options;
pub mod writer;

use jni::JNIEnv;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::jint;

const ERR_BUFFER: jint = -1;
const ERR_PACK: jint = -2;

/// `static native int packDsfBlocks(ByteBuffer src, int srcOff, int srcLen, ByteBuffer dst, int dstOff)`
///
/// Packs `srcLen` bytes of raw DSF block pairs (direct buffers only) into M7 at `dstOff`.
/// Returns the bytes written, `-1` for an invalid buffer or range, `-2` when packing is rejected
/// (unaligned length, short output).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_encelerate_audigmaplayer_audio_AudigmaNative_packDsfBlocks(
    env: JNIEnv,
    _class: JClass,
    src: JByteBuffer,
    src_off: jint,
    src_len: jint,
    dst: JByteBuffer,
    dst_off: jint,
) -> jint {
    let (Ok(sp), Ok(sc), Ok(dp), Ok(dc)) = (
        env.get_direct_buffer_address(&src),
        env.get_direct_buffer_capacity(&src),
        env.get_direct_buffer_address(&dst),
        env.get_direct_buffer_capacity(&dst),
    ) else {
        return ERR_BUFFER;
    };
    let (Ok(so), Ok(sl), Ok(d_o)) = (
        usize::try_from(src_off),
        usize::try_from(src_len),
        usize::try_from(dst_off),
    ) else {
        return ERR_BUFFER;
    };
    if sp.is_null() || dp.is_null() || so.checked_add(sl).is_none_or(|e| e > sc) || d_o > dc {
        return ERR_BUFFER;
    }
    // SAFETY: both ranges lie inside direct buffers kept alive by the caller for this call. The
    // two buffers must not overlap (the caller passes distinct buffers).
    let (s, d) = unsafe {
        (
            std::slice::from_raw_parts(sp.add(so), sl),
            std::slice::from_raw_parts_mut(dp.add(d_o), dc - d_o),
        )
    };
    match m7::pack_m7_dsf_blocks(s, d) {
        Ok(n) => n as jint,
        Err(_) => ERR_PACK,
    }
}
