/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import java.nio.ByteBuffer
import java.nio.ByteOrder
import timber.log.Timber as L

/**
 * Native DSD output on the FiiO JM21 (`docs/dsd-transport-spec.md`): an `AudioTrack` with encoding
 * 254 at DSD rate / 32, fed by the Rust writer thread (`native/audigma-glue`) with M7-packed stereo
 * DSD.
 *
 * Input is either raw DSF block pairs (4096 B left, 4096 B right, LSB-first) via [push], or, when
 * opened with a [pcmRateHz], PCM via [pushPcm] that audigma converts to DSD at [dsdRateHz] on its
 * own threads. The JVM owns the track (creation, `setCodecParam`, play, pause, flush, release); the
 * writer thread only calls `write`. All methods are called from one thread (Media3's playback
 * thread).
 *
 * Control ordering (design doc, "Threading and control"): stop the writer from starting writes,
 * unblock a write already inside the track (pause), wait for the writer to park, and only then
 * flush the track and reset the writer and the converter.
 */
class DsdOutput
private constructor(
    val dsdRateHz: Int,
    /** PCM input rate converted by audigma; 0 for raw DSD input. */
    val pcmRateHz: Int,
    private val track: AudioTrack,
    private val handle: Long,
) {
    private var released = false
    /** `AudioTrack.play()` was called and the track was not paused or flushed since. */
    private var trackPlaying = false
    /** [play] was requested; the track starts after the writer has primed it. */
    private var wantPlay = false
    private var emptiedAtNanos = 0L
    /** Last `AudioTrack.underrunCount` seen by [checkUnderruns], and when it was read. */
    private var underruns = 0
    private var underrunCheckNanos = 0L
    /** [endOfStream] was called and no input arrived since: an underrun now is the drain's. */
    private var ending = false

    /** Bytes per second of M7 data: 2 channels x DSD rate / 8. */
    private val bytesPerSecond = dsdRateHz / 4L

    /** The converter's filter delay: the output lags the input by this much. */
    private val delayUs = AudigmaNative.nativeDelaySamples(handle) * 1_000_000L / dsdRateHz

    /**
     * Packs DSF block pairs from [src] (see [AudigmaAudioSink.MIME_DSD_DSF]) into the ring, all or
     * nothing: returns [length] when taken, 0 when the ring lacks room.
     */
    fun push(src: ByteBuffer, offset: Int, length: Int): Int {
        require(src.isDirect)
        val n = AudigmaNative.nativePush(handle, src, offset, length)
        check(n >= 0) { "bad push range" }
        if (n > 0) ending = false
        poll()
        return n
    }

    /**
     * Queues interleaved PCM ([encoding] a Media3 `C.ENCODING_PCM_*` of [isConvertible], 1 or 2
     * [channels]) for conversion, scaled by [gain]. Takes as many whole frames as fit; returns the
     * bytes taken.
     */
    fun pushPcm(
        src: ByteBuffer,
        offset: Int,
        length: Int,
        encoding: Int,
        channels: Int,
        gain: Float,
    ): Int {
        require(src.isDirect)
        val n = AudigmaNative.nativePushPcm(handle, src, offset, length, encoding, channels, gain)
        check(n >= 0) { "bad PCM push: encoding=$encoding channels=$channels" }
        if (n > 0) ending = false
        poll()
        return n
    }

    /** M7 bytes the track has accepted since the last [flush]. */
    val bytesWritten: Long
        get() = AudigmaNative.nativeWritten(handle)

    /** Bytes pushed but not yet accepted by the track (PCM not yet converted is estimated). */
    val bytesBuffered: Long
        get() = AudigmaNative.nativeBuffered(handle)

    val ringFree: Long
        get() = AudigmaNative.nativeFree(handle)

    /** Last writer error: 0 none, otherwise see [AudigmaNative.nativeError]. */
    val error: Int
        get() = AudigmaNative.nativeError(handle)

    /**
     * Playback position in microseconds of data accepted by the track (excludes queued data and the
     * converter's filter delay).
     */
    val positionUs: Long
        get() = (bytesWritten * 1_000_000L / bytesPerSecond - delayUs).coerceAtLeast(0)

    val trackBufferUs: Long
        get() = TRACK_BYTES * 1_000_000L / bytesPerSecond

    /**
     * Starts the writer. The track itself starts once it holds a full buffer ([poll]), as the spec
     * requires `play()` after the first write; [endOfStream] forces it for short streams.
     */
    fun play() {
        wantPlay = true
        AudigmaNative.nativeSetRunning(handle, true)
        if (trackPlaying) return
        if (primed()) startTrack()
    }

    /** Pauses: no new writes, the blocked write is released, the writer is parked on return. */
    fun pause() {
        wantPlay = false
        checkUnderruns()
        park()
        if (trackPlaying) {
            runCatching { track.pause() }
            trackPlaying = false
        }
    }

    /**
     * Drops everything queued: ring, unaccepted bytes, the track's buffer. The play intent survives
     * (as in `DefaultAudioSink`): if playing, the writer resumes and the track restarts once the
     * new data has primed it.
     */
    fun flush() {
        park()
        runCatching {
            track.pause()
            track.flush()
        }
        trackPlaying = false
        AudigmaNative.nativeReset(handle)
        emptiedAtNanos = 0
        ending = false
        if (wantPlay) AudigmaNative.nativeSetRunning(handle, true)
    }

    /**
     * No more input for now (end of the stream, or before a switch): the converter plays out what
     * it holds plus its filters' tail, and a track not filled to [TRACK_BYTES] starts anyway.
     */
    fun endOfStream() {
        ending = true
        AudigmaNative.nativeEndOfStream(handle)
        if (wantPlay && !trackPlaying) startTrack()
    }

    /** True once everything pushed has been accepted by the track and the track has played out. */
    fun isDrained(): Boolean {
        if (bytesBuffered > 0) {
            emptiedAtNanos = 0
            return false
        }
        val now = System.nanoTime()
        if (emptiedAtNanos == 0L) emptiedAtNanos = now
        return now - emptiedAtNanos >= (trackBufferUs + DRAIN_MARGIN_US) * 1000
    }

    /** Called from every sink entry point; starts the track once the first buffer is filled. */
    fun poll() {
        if (wantPlay && !trackPlaying && primed()) startTrack()
        val now = System.nanoTime()
        if (trackPlaying && now - underrunCheckNanos >= UNDERRUN_CHECK_NANOS) {
            underrunCheckNanos = now
            checkUnderruns()
        }
    }

    /** Logs underruns since the last check, so each one is placed in time. */
    private fun checkUnderruns() {
        val n = runCatching { track.underrunCount }.getOrDefault(underruns)
        if (n <= underruns) return
        L.w(
            "DSD underrun: +${n - underruns} (total $n) at ${positionUs / 1000} ms, " +
                "queued $bytesBuffered B" +
                if (ending) ", draining" else ""
        )
        underruns = n
    }

    private fun primed() = bytesWritten >= TRACK_BYTES

    private fun startTrack() {
        runCatching { track.play() }.onFailure { L.e(it, "AudioTrack.play failed") }
        trackPlaying = true
    }

    private fun park() {
        AudigmaNative.nativeSetRunning(handle, false)
        // A write blocked inside the track returns when the track is paused or stopped.
        runCatching { track.pause() }
        if (!AudigmaNative.nativeAwaitIdle(handle, PARK_TIMEOUT_MS)) {
            L.w("DSD writer still blocked after pause, stopping the track")
            runCatching { track.stop() }
            if (!AudigmaNative.nativeAwaitIdle(handle, PARK_TIMEOUT_MS)) L.e("DSD writer stuck")
        }
    }

    /** Stops the writer and the converter, joins them, releases the track. Idempotent. */
    fun release() {
        if (released) return
        released = true
        checkUnderruns()
        park()
        val underruns = runCatching { track.underrunCount }.getOrDefault(-1)
        val converter = AudigmaNative.nativeDescribe(handle)
        if (!AudigmaNative.nativeDestroy(handle, DESTROY_TIMEOUT_MS)) {
            L.e("DSD writer did not exit")
        }
        runCatching { track.release() }
        L.d("DSD output released: rate=$dsdRateHz pcm=$pcmRateHz underruns=$underruns $converter")
    }

    companion object {
        /** `AudioFormat.ENCODING_DSD_NATIVE` on FiiO firmware; absent from public SDKs. */
        const val ENCODING_DSD_NATIVE = 254

        /** The track buffer and the size of every write (spec: 64 KiB). */
        const val TRACK_BYTES = 65536

        const val RING_BYTES = 1 shl 20

        private const val PARK_TIMEOUT_MS = 500
        private const val DESTROY_TIMEOUT_MS = 1000
        private const val DRAIN_MARGIN_US = 20_000L
        private const val UNDERRUN_CHECK_NANOS = 1_000_000_000L

        private val RATES_HZ = setOf(2_822_400, 5_644_800, 11_289_600)

        /** PCM rates audigma converts here: 44.1 and 48 kHz families up to 384 kHz. */
        private val PCM_RATES_HZ =
            setOf(44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000)

        private val PCM_ENCODINGS =
            setOf(
                C.ENCODING_PCM_16BIT,
                C.ENCODING_PCM_24BIT,
                C.ENCODING_PCM_32BIT,
                C.ENCODING_PCM_FLOAT,
            )

        private val libraryLoaded: Boolean by lazy {
            runCatching { Class.forName(AudigmaNative::class.java.name) }
                .onFailure { L.w(it, "libaudigma_glue unavailable; DSD output disabled") }
                .isSuccess
        }

        /** Whether DSD at [dsdRateHz] can be attempted here: library present, a supported rate. */
        fun supports(dsdRateHz: Int) = dsdRateHz in RATES_HZ && libraryLoaded

        /** Whether audigma can convert PCM in [format] (rate, 1 or 2 channels, encoding). */
        fun isConvertible(format: Format) =
            MimeTypes.AUDIO_RAW == format.sampleMimeType &&
                format.sampleRate in PCM_RATES_HZ &&
                format.channelCount in 1..2 &&
                format.pcmEncoding in PCM_ENCODINGS

        /**
         * Opens a stereo DSD output at [dsdRateHz] (2822400, 5644800 or 11289600). With
         * [pcmRateHz] > 0 the input is PCM at that rate ([isConvertible]), converted by audigma.
         *
         * @throws IllegalStateException when the device refuses (not a JM21, library missing) or
         *   audigma rejects the conversion.
         */
        fun open(dsdRateHz: Int, pcmRateHz: Int = 0): DsdOutput {
            check(supports(dsdRateHz)) { "DSD rate $dsdRateHz unsupported" }
            val trackRate = dsdRateHz / 32
            // Hidden, static, FiiO-only: mode 1 = native DSD.
            val result =
                AudioManager::class
                    .java
                    .getDeclaredMethod(
                        "setCodecParam",
                        Int::class.java,
                        Int::class.java,
                        Int::class.java,
                        Int::class.java,
                    )
                    .apply { isAccessible = true }
                    .invoke(null, 100, 1, trackRate, 0)
            L.d("setCodecParam(100, 1, $trackRate, 0) -> $result")
            val track =
                AudioTrack.Builder()
                    .setAudioAttributes(
                        AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
                    )
                    .setAudioFormat(
                        AudioFormat.Builder()
                            .setSampleRate(trackRate)
                            .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                            .setEncoding(ENCODING_DSD_NATIVE)
                            .build()
                    )
                    .setBufferSizeInBytes(TRACK_BYTES)
                    .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_POWER_SAVING)
                    .build()
            check(track.state == AudioTrack.STATE_INITIALIZED) { "AudioTrack not initialised" }
            // The writer thread keeps the scratch buffer alive through a JNI global reference.
            val scratch = ByteBuffer.allocateDirect(TRACK_BYTES).order(ByteOrder.nativeOrder())
            val handle =
                try {
                    AudigmaNative.nativeCreate(track, scratch, RING_BYTES, dsdRateHz, pcmRateHz)
                } catch (e: IllegalStateException) {
                    track.release()
                    throw e
                }
            if (handle == 0L) {
                track.release()
                error("native writer failed to start")
            }
            val output = DsdOutput(dsdRateHz, pcmRateHz, track, handle)
            L.d(
                "DSD output opened: rate=$dsdRateHz trackRate=$trackRate " +
                    AudigmaNative.nativeDescribe(handle)
            )
            return output
        }
    }
}
