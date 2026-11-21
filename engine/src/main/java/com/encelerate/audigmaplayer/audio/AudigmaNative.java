/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */

package com.encelerate.audigmaplayer.audio;

import java.nio.ByteBuffer;

/**
 * JNI entry points of libaudigma_glue.so (engine/native/audigma-glue). Raw DSD is DSF block pairs: 4096 B
 * left then 4096 B right, LSB-first. PCM is interleaved, little-endian, Media3 encodings. Buffers
 * must be direct.
 */
public final class AudigmaNative {
  static { System.loadLibrary("audigma_glue"); }

  private AudigmaNative() {}

  /**
   * Packs srcLen bytes of raw DSF block pairs from a direct buffer into M7 at dstOff of another
   * direct buffer. Returns bytes written; -1 invalid buffer or range, -2 unaligned length or short
   * output.
   */
  public static native int packDsfBlocks(ByteBuffer src, int srcOff, int srcLen, ByteBuffer dst, int dstOff);

  /**
   * Starts a stopped writer thread that writes M7 from its ring with
   * track.write(scratch, n, WRITE_BLOCKING). scratch: direct buffer, multiple of 8192 bytes.
   * pcmRate 0: input is raw DSF via nativePush; otherwise audigma converts PCM at pcmRate Hz
   * (nativePushPcm) to dsdRate Hz stereo DSD on its own threads. Returns a handle, 0 on failure
   * (IllegalStateException with the reason when the converter could not be set up).
   */
  public static native long nativeCreate(
      Object track, ByteBuffer scratch, int ringBytes, int dsdRate, int pcmRate);

  /** Copies raw DSF block pairs into the ring, never blocks; returns bytes accepted, -1 bad range. */
  public static native int nativePush(long handle, ByteBuffer src, int off, int len);

  /**
   * Queues interleaved PCM (encoding: C.ENCODING_PCM_16BIT, _24BIT, _32BIT or _FLOAT; 1 or 2
   * channels) scaled by gain for conversion, as many whole frames as fit; never blocks. Returns
   * bytes accepted, -1 for a bad range, unsupported format or an output without a converter.
   */
  public static native int nativePushPcm(
      long handle, ByteBuffer src, int off, int len, int encoding, int channels, float gain);

  /** No more input for now: the converter converts what it holds plus its filters' tail. */
  public static native void nativeEndOfStream(long handle);

  /** false: no new writes start (a blocked write needs the track paused or stopped too). */
  public static native void nativeSetRunning(long handle, boolean running);

  /** Waits until the writer is parked and not running; false on timeout. */
  public static native boolean nativeAwaitIdle(long handle, int timeoutMs);

  /** Drops all queued audio (converter input too), the written counter and the error. */
  public static native void nativeReset(long handle);

  /** M7 bytes queued, not yet accepted by the track (PCM not yet converted is estimated). */
  public static native long nativeBuffered(long handle);

  /** Free ring space in bytes. */
  public static native long nativeFree(long handle);

  /** M7 bytes accepted by the track since the last reset. */
  public static native long nativeWritten(long handle);

  /** 0, -1000 (the track call raised), -2000 (conversion failed) or a negative AudioTrack error. */
  public static native int nativeError(long handle);

  /** Converter filter delay in DSD samples per channel; 0 for raw DSD. */
  public static native long nativeDelaySamples(long handle);

  /** Converter configuration and thread placement for logs; empty for raw DSD. */
  public static native String nativeDescribe(long handle);

  /**
   * {clipped input samples, modulator overloads, peak since the last call in 1/1000 of full scale};
   * zeros for raw DSD. Reading resets the peak.
   */
  public static native long[] nativeStats(long handle);

  /** Stops and joins the converter and the writer and frees the handle; pause/stop the track first. */
  public static native boolean nativeDestroy(long handle, int timeoutMs);
}
