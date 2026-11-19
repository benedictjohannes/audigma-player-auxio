/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.audio

import android.content.Context
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer
import java.nio.ByteOrder
import timber.log.Timber as L

/**
 * The seam between Media3 renderers and the audio output. Three outputs:
 * - PCM to [delegate] (a `DefaultAudioSink`), with a software gain (ReplayGain). At unity gain
 *   buffers are passed through untouched, so playback stays bit-perfect. Gain is applied here
 *   rather than as a Media3 `AudioProcessor`, because `DefaultAudioSink` skips custom processors
 *   when it outputs float.
 * - Raw DSD ([MIME_DSD_DSF]) to a [DsdOutput] at the file's rate, untouched (passthrough).
 * - PCM converted to DSD by audigma when [pcmToDsdRateHz] is set and the format is convertible
 *   ([DsdOutput.isConvertible]); the gain is applied before the modulator.
 *
 * @param name Identifies the owning renderer in logs.
 * @param gain Linear gain to apply, read once per buffer on the playback thread.
 */
class AudigmaAudioSink(
    private val name: String,
    delegate: AudioSink,
    private val gain: () -> Float,
) : ForwardingAudioSink(delegate) {
    /** PCM encoding of incoming buffers, or [C.ENCODING_INVALID] if not linear PCM. */
    private var encoding = C.ENCODING_INVALID
    /** Channel count of incoming PCM, for conversion to DSD. */
    private var channels = 0
    /** Scaled copy of the current input buffer, until the delegate has consumed all of it. */
    private var scaled: ByteBuffer? = null
    /** The delegate partially consumed an unscaled buffer; it must get the same buffer again. */
    private var passingThrough = false
    private var scratch: ByteBuffer = ByteBuffer.allocateDirect(0)

    /** Active DSD output (raw DSD or converted PCM), or null while the delegate plays PCM. */
    private var dsd: DsdOutput? = null
    /** Direct copy of an input buffer that is not direct, until it is consumed. */
    private var dsdStage: ByteBuffer? = null
    /** Presentation time of the first buffer since the last flush, `C.TIME_UNSET` before it. */
    private var dsdStartUs = C.TIME_UNSET
    private var dsdInputEnded = false

    /** Last [play] or [pause]; survives output switches, as `DefaultAudioSink`'s does. */
    private var playing = false
    /**
     * A configuration that needs a different output (DSD rate change, PCM <-> DSD). Applied once
     * the current output has played out what it holds (gapless contract), or at once on [flush].
     */
    private var pending: PendingConfig? = null

    override fun supportsFormat(format: Format) =
        getFormatSupport(format) != AudioSink.SINK_FORMAT_UNSUPPORTED

    override fun getFormatSupport(format: Format): Int =
        if (isDsd(format)) {
            if (DsdOutput.supports(format.sampleRate) && format.channelCount == 2) {
                AudioSink.SINK_FORMAT_SUPPORTED_DIRECTLY
            } else AudioSink.SINK_FORMAT_UNSUPPORTED
        } else super.getFormatSupport(format)

    override fun configure(
        inputFormat: Format,
        specifiedBufferSize: Int,
        outputChannels: IntArray?,
    ) {
        L.d(
            "$name configure: mime=${inputFormat.sampleMimeType} " +
                "encoding=${inputFormat.pcmEncoding} rate=${inputFormat.sampleRate} " +
                "channels=${inputFormat.channelCount}"
        )
        scaled = null
        passingThrough = false
        val target = dsdTargetOf(inputFormat)
        val current = dsd
        if (target != null && current != null && target.isServedBy(current)) {
            // Same output: the new stream joins the old one, sample-exactly for raw DSD, through
            // the running filters and modulators for converted PCM.
            pending = null
            dsdInputEnded = false
            setDsdInputFormat(inputFormat)
            return
        }
        if (target == null && current == null) {
            // PCM to PCM: the delegate drains its old track itself.
            pending = null
            configureDelegate(inputFormat, specifiedBufferSize, outputChannels)
            return
        }
        pending = PendingConfig(inputFormat, specifiedBufferSize, outputChannels, target)
    }

    /** The DSD output [format] needs, or null for the delegate (PCM, conversion off or n/a). */
    private fun dsdTargetOf(format: Format): DsdTarget? {
        if (isDsd(format)) return DsdTarget(format.sampleRate, 0)
        val rate = pcmToDsdRateHz
        return if (DsdOutput.supports(rate) && DsdOutput.isConvertible(format)) {
            DsdTarget(rate, format.sampleRate)
        } else null
    }

    private fun setDsdInputFormat(format: Format) {
        encoding = if (isDsd(format)) C.ENCODING_INVALID else format.pcmEncoding
        channels = format.channelCount
    }

    private fun configureDelegate(format: Format, bufferSize: Int, outputChannels: IntArray?) {
        super.configure(format, bufferSize, outputChannels)
        encoding =
            if (MimeTypes.AUDIO_RAW == format.sampleMimeType) format.pcmEncoding
            else C.ENCODING_INVALID
    }

    /** Plays out the current output before a switch; true once nothing is left to play. */
    private fun drainCurrent(): Boolean {
        val out = dsd
        if (out == null) {
            super.playToEndOfStream()
            return super.isEnded()
        }
        if (out.bytesWritten == 0L && out.bytesBuffered == 0L) return true
        out.endOfStream()
        return out.isDrained()
    }

    /** Replaces the current output by the one [config] needs, keeping the play state. */
    private fun applyPending(config: PendingConfig) {
        pending = null
        encoding = C.ENCODING_INVALID
        releaseDsd()
        // The PCM track must be gone before the DSD track opens. Resets the delegate's play state.
        super.reset()
        val format = config.format
        val target = config.dsdTarget
        if (target != null) {
            val out = DsdOutput.open(target.dsdRateHz, target.pcmRateHz)
            dsd = out
            setDsdInputFormat(format)
            if (playing) out.play()
        } else {
            configureDelegate(format, config.bufferSize, config.outputChannels)
            if (playing) super.play()
        }
    }

    override fun handleBuffer(
        buffer: ByteBuffer,
        presentationTimeUs: Long,
        encodedAccessUnitCount: Int,
    ): Boolean {
        pending?.let {
            if (!drainCurrent()) return false
            applyPending(it)
        }
        dsd?.let {
            return handleDsdBuffer(it, buffer, presentationTimeUs)
        }
        var out = scaled
        if (out == null) {
            val g = gain()
            if (passingThrough || g == 1f || !isScalable(encoding)) {
                val consumed =
                    super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
                passingThrough = !consumed
                return consumed
            }
            out = scale(buffer, g)
            scaled = out
        }
        // On a retry the renderer passes the same, already read, input buffer; the delegate
        // gets the same scaled buffer, as it requires.
        val consumed = super.handleBuffer(out, presentationTimeUs, encodedAccessUnitCount)
        if (consumed) scaled = null
        return consumed
    }

    /**
     * Raw DSF block pairs go straight to the Rust ring; nothing here touches the bits, so DSD is
     * passthrough (no gain, no processing). PCM goes to audigma's converter, with the gain. Never
     * blocks: a full ring returns false and Media3 retries with the rest of the buffer.
     */
    private fun handleDsdBuffer(
        out: DsdOutput,
        buffer: ByteBuffer,
        presentationTimeUs: Long,
    ): Boolean {
        if (dsdStartUs == C.TIME_UNSET) dsdStartUs = presentationTimeUs
        check(out.error == 0) { "DSD output failed: ${out.error}" }
        val direct =
            if (buffer.isDirect) buffer
            else (dsdStage ?: copyToDirect(buffer).also { dsdStage = it })
        val n =
            if (out.pcmRateHz == 0) out.push(direct, direct.position(), direct.remaining())
            else
                out.pushPcm(
                    direct,
                    direct.position(),
                    direct.remaining(),
                    encoding,
                    channels,
                    gain(),
                )
        direct.position(direct.position() + n)
        if (direct !== buffer) buffer.position(buffer.limit() - direct.remaining())
        if (!direct.hasRemaining()) {
            dsdStage = null
            return true
        }
        return false
    }

    private fun copyToDirect(src: ByteBuffer): ByteBuffer {
        val copy = ByteBuffer.allocateDirect(src.remaining())
        copy.put(src.duplicate()).flip()
        return copy
    }

    override fun getCurrentPositionUs(sourceEnded: Boolean): Long {
        val out = dsd ?: return super.getCurrentPositionUs(sourceEnded)
        out.poll()
        if (dsdStartUs == C.TIME_UNSET) return AudioSink.CURRENT_POSITION_NOT_SET
        return dsdStartUs + out.positionUs
    }

    override fun play() {
        playing = true
        dsd?.play() ?: super.play()
    }

    override fun pause() {
        playing = false
        dsd?.pause() ?: super.pause()
    }

    override fun playToEndOfStream() {
        val out = dsd ?: return super.playToEndOfStream()
        dsdInputEnded = true
        out.endOfStream()
    }

    override fun isEnded() = dsd?.let { dsdInputEnded && it.isDrained() } ?: super.isEnded()

    override fun hasPendingData() =
        dsd?.let {
            it.poll()
            it.bytesBuffered > 0
        } ?: super.hasPendingData()

    override fun getAudioTrackBufferSizeUs() =
        dsd?.trackBufferUs ?: super.getAudioTrackBufferSizeUs()

    override fun flush() {
        scaled = null
        passingThrough = false
        // Nothing queued survives a flush, so a pending switch needs no playout.
        pending?.let {
            applyPending(it)
            return
        }
        dsd?.let {
            it.flush()
            dsdStage = null
            dsdStartUs = C.TIME_UNSET
            dsdInputEnded = false
            return
        }
        super.flush()
    }

    override fun reset() {
        scaled = null
        passingThrough = false
        pending = null
        playing = false
        releaseDsd()
        super.reset()
    }

    override fun release() {
        releaseDsd()
        super.release()
    }

    private fun releaseDsd() {
        dsd?.release()
        dsd = null
        dsdStage = null
        dsdStartUs = C.TIME_UNSET
        dsdInputEnded = false
    }

    /** Copies the remainder of [input] into [scratch] multiplied by [g]; consumes [input]. */
    private fun scale(input: ByteBuffer, g: Float): ByteBuffer {
        val size = input.remaining()
        if (scratch.capacity() < size) {
            scratch = ByteBuffer.allocateDirect(size).order(ByteOrder.nativeOrder())
        }
        val out = scratch
        out.clear()
        // Samples are little-endian. Clamp to the encoding's range so that samples amplified
        // beyond it do not wrap around, which would cause popping.
        val src = input.duplicate().order(ByteOrder.LITTLE_ENDIAN)
        val dst = out.order(ByteOrder.LITTLE_ENDIAN)
        when (encoding) {
            C.ENCODING_PCM_16BIT ->
                while (src.remaining() >= 2) {
                    val s = (src.short * g).toInt()
                    dst.putShort(
                        s.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()).toShort()
                    )
                }
            C.ENCODING_PCM_24BIT ->
                while (src.remaining() >= 3) {
                    val b0 = src.get().toInt() and 0xFF
                    val b1 = src.get().toInt() and 0xFF
                    val b2 = src.get().toInt() // sign-extends
                    val s = ((b2 shl 16) or (b1 shl 8) or b0) * g.toDouble()
                    val v = s.toInt().coerceIn(-0x800000, 0x7FFFFF)
                    dst.put(v.toByte())
                    dst.put((v shr 8).toByte())
                    dst.put((v shr 16).toByte())
                }
            C.ENCODING_PCM_32BIT ->
                while (src.remaining() >= 4) {
                    // Double to Int conversion saturates, which is the clamp.
                    dst.putInt((src.int * g.toDouble()).toInt())
                }
            C.ENCODING_PCM_FLOAT ->
                while (src.remaining() >= 4) {
                    dst.putFloat((src.float * g).coerceIn(-1f, 1f))
                }
        }
        input.position(input.limit())
        out.flip()
        return out
    }

    private class PendingConfig(
        val format: Format,
        val bufferSize: Int,
        val outputChannels: IntArray?,
        /** The DSD output to open, or null for the delegate. */
        val dsdTarget: DsdTarget?,
    )

    /** A DSD output at [dsdRateHz]; [pcmRateHz] is the converted PCM rate, 0 for raw DSD. */
    private data class DsdTarget(val dsdRateHz: Int, val pcmRateHz: Int) {
        fun isServedBy(out: DsdOutput) = out.dsdRateHz == dsdRateHz && out.pcmRateHz == pcmRateHz
    }

    companion object {
        /**
         * `Format.sampleMimeType` of raw DSD input: DSF block pairs (4096 B left, 4096 B right,
         * LSB-first) with `sampleRate` the DSD rate in Hz (2822400, 5644800, 11289600) and two
         * channels. A file's last buffer may end in one short pair (`v` bytes left, then `v` bytes
         * right): the last block truncated to the sample count. Produced by `DsfExtractor`,
         * delivered by `MediaCodecAudioRenderer` in codec bypass.
         */
        const val MIME_DSD_DSF = "audio/x-audigma-dsd-dsf"

        /**
         * DSD rate in Hz (2822400, 5644800 or 11289600) that PCM is converted to by audigma; any
         * other value (0) plays PCM bit-perfect. Read at every [configure], so a change applies
         * from the next track on.
         */
        // Step I: DSD256 until the settings UI (Step J) owns this and defaults it to off.
        @Volatile @JvmStatic var pcmToDsdRateHz = 11_289_600

        private fun isDsd(format: Format) = MIME_DSD_DSF == format.sampleMimeType

        private fun isScalable(encoding: Int) =
            encoding == C.ENCODING_PCM_16BIT ||
                encoding == C.ENCODING_PCM_24BIT ||
                encoding == C.ENCODING_PCM_32BIT ||
                encoding == C.ENCODING_PCM_FLOAT

        /**
         * Bit-perfect PCM: a [DefaultAudioSink] with float output enabled, so 24-bit sources reach
         * the `AudioTrack` as float (exact for 24-bit) instead of being truncated to 16 bits.
         * Decoders that can output float (FFmpeg) then do so for every source. The JM21 opens its
         * output at the track's rate and format and converts to 32-bit in the HAL; packed 24-bit
         * tracks are broken there, so they must not be used.
         *
         * @param gain See [AudigmaAudioSink].
         */
        fun create(name: String, context: Context, gain: () -> Float) =
            AudigmaAudioSink(
                name,
                DefaultAudioSink.Builder(context).setEnableFloatOutput(true).build(),
                gain,
            )
    }
}
