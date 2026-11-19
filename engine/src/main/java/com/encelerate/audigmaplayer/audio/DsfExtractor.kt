/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.audio

import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.ParserException
import androidx.media3.common.util.ParsableByteArray
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.extractor.Extractor
import androidx.media3.extractor.ExtractorInput
import androidx.media3.extractor.ExtractorOutput
import androidx.media3.extractor.PositionHolder
import androidx.media3.extractor.SeekMap
import androidx.media3.extractor.SeekPoint
import androidx.media3.extractor.TrackOutput

/**
 * Media3 [Extractor] for stereo DSF files. Emits the data chunk as it is in the file (raw block
 * pairs, LSB-first) with MIME type [AudigmaAudioSink.MIME_DSD_DSF]; no renderer decodes it, the
 * stock `MediaCodecAudioRenderer` passes it to [AudigmaAudioSink] in codec bypass.
 *
 * Samples are up to [MAX_PAIRS] whole block pairs, all key frames. The last block is truncated to
 * the header's sample count: the final sample is a short pair (`v` bytes left, `v` bytes right), so
 * the zero padding in the file (full-scale DC in DSD) is never played. Seeking is to block-pair
 * boundaries (1.45 ms at DSD64).
 */
@UnstableApi
class DsfExtractor : Extractor {
    private lateinit var output: TrackOutput
    private lateinit var extractorOutput: ExtractorOutput

    /** File offset of the first block pair; -1 until the header has been read. */
    private var dataStart = -1L
    private var rateHz = 0
    /** Block pairs whose blocks are entirely valid audio. */
    private var fullPairs = 0L
    /** Valid bytes per channel in the block pair after [fullPairs]; 0 if there is none. */
    private var tailBytes = 0

    private var sampleSize = 0
    /** Bytes of the current sample not yet read; 0 between samples. */
    private var samplePending = 0
    private var sampleTimeUs = 0L

    override fun sniff(input: ExtractorInput): Boolean {
        val b = ParsableByteArray(DSD_CHUNK_BYTES + 4)
        if (!input.peekFully(b.data, 0, b.limit(), true)) return false
        if (b.readString(4) != "DSD ") return false
        if (b.readLittleEndianLong() != DSD_CHUNK_BYTES.toLong()) return false
        b.position = DSD_CHUNK_BYTES
        return b.readString(4) == "fmt "
    }

    override fun init(output: ExtractorOutput) {
        extractorOutput = output
        this.output = output.track(0, C.TRACK_TYPE_AUDIO)
    }

    override fun read(input: ExtractorInput, seekPosition: PositionHolder): Int {
        if (dataStart < 0) {
            readHeader(input)
            return Extractor.RESULT_CONTINUE
        }
        if (samplePending == 0) {
            if (input.position < dataStart) input.skipFully((dataStart - input.position).toInt())
            val rel = input.position - dataStart
            if (rel % PAIR != 0L) {
                throw ParserException.createForMalformedContainer("DSF: read at $rel", null)
            }
            val pair = rel / PAIR
            when {
                pair < fullPairs -> {
                    sampleSize = (minOf(MAX_PAIRS.toLong(), fullPairs - pair) * PAIR).toInt()
                    samplePending = sampleSize
                    sampleTimeUs = timeOfPair(pair)
                }
                pair == fullPairs && tailBytes > 0 -> {
                    readTail(input)
                    return Extractor.RESULT_END_OF_INPUT
                }
                else -> return Extractor.RESULT_END_OF_INPUT
            }
        }
        val n = output.sampleData(input, samplePending, true)
        if (n == C.RESULT_END_OF_INPUT) {
            // The data chunk is shorter than its header says: keep the whole pairs read.
            val read = sampleSize - samplePending
            val whole = read / PAIR * PAIR
            if (whole > 0) {
                output.sampleMetadata(
                    sampleTimeUs,
                    C.BUFFER_FLAG_KEY_FRAME,
                    whole,
                    read - whole,
                    null,
                )
            }
            samplePending = 0
            return Extractor.RESULT_END_OF_INPUT
        }
        samplePending -= n
        if (samplePending == 0) {
            output.sampleMetadata(sampleTimeUs, C.BUFFER_FLAG_KEY_FRAME, sampleSize, 0, null)
        }
        return Extractor.RESULT_CONTINUE
    }

    /** Emits the last, partly valid block pair as a short pair of [tailBytes] per channel. */
    private fun readTail(input: ExtractorInput) {
        val buf = ByteArray(BLOCK + tailBytes)
        input.readFully(buf, 0, buf.size)
        // Left valid bytes stay at 0; move the right valid bytes right behind them.
        System.arraycopy(buf, BLOCK, buf, tailBytes, tailBytes)
        val size = 2 * tailBytes
        output.sampleData(ParsableByteArray(buf, size), size)
        output.sampleMetadata(timeOfPair(fullPairs), C.BUFFER_FLAG_KEY_FRAME, size, 0, null)
    }

    private fun readHeader(input: ExtractorInput) {
        val dsd = ParsableByteArray(DSD_CHUNK_BYTES)
        input.readFully(dsd.data, 0, DSD_CHUNK_BYTES)
        dsd.skipBytes(4)
        input.skipFully((dsd.readLittleEndianLong() - DSD_CHUNK_BYTES).toInt())

        val fmtHead = readChunkHeader(input, "fmt ")
        val fmt = ParsableByteArray((fmtHead - CHUNK_HEADER_BYTES).toInt())
        input.readFully(fmt.data, 0, fmt.limit())
        fmt.skipBytes(4) // format version
        val formatId = fmt.readLittleEndianInt()
        fmt.skipBytes(4) // channel type
        val channels = fmt.readLittleEndianInt()
        val rate = fmt.readLittleEndianInt()
        val bitsPerSample = fmt.readLittleEndianInt()
        val sampleCount = fmt.readLittleEndianLong()
        val blockSize = fmt.readLittleEndianInt()
        if (formatId != 0 || channels != 2 || bitsPerSample != 1 || blockSize != BLOCK) {
            throw ParserException.createForUnsupportedContainerFeature(
                "DSF: format $formatId, $channels channels, $bitsPerSample bits per sample, " +
                    "block size $blockSize (supported: raw stereo, LSB-first, 4096)"
            )
        }

        val dataSize = readChunkHeader(input, "data")
        dataStart = input.position
        rateHz = rate
        // Valid bytes per channel: by sample count, capped by what the data chunk holds.
        val perChannel = minOf(sampleCount / 8, (dataSize - CHUNK_HEADER_BYTES) / PAIR * BLOCK)
        fullPairs = perChannel / BLOCK
        tailBytes = (perChannel % BLOCK).toInt()

        output.format(
            Format.Builder()
                .setContainerMimeType(CONTAINER_MIME)
                .setSampleMimeType(AudigmaAudioSink.MIME_DSD_DSF)
                .setSampleRate(rate)
                .setChannelCount(2)
                .setAverageBitrate(2 * rate)
                .setPeakBitrate(2 * rate)
                .setMaxInputSize(MAX_PAIRS * PAIR)
                .build()
        )
        extractorOutput.seekMap(SeekByPair(timeOfBytes(perChannel)))
        extractorOutput.endTracks()
    }

    /** Reads a chunk header, checks its id; returns the chunk size including the header. */
    private fun readChunkHeader(input: ExtractorInput, id: String): Long {
        val h = ParsableByteArray(CHUNK_HEADER_BYTES)
        input.readFully(h.data, 0, CHUNK_HEADER_BYTES)
        val got = h.readString(4)
        if (got != id) throw ParserException.createForMalformedContainer("DSF: $got, not $id", null)
        return h.readLittleEndianLong()
    }

    private fun timeOfPair(pair: Long) = timeOfBytes(pair * BLOCK)

    /** Duration of [bytes] per channel (8 DSD samples per byte). */
    private fun timeOfBytes(bytes: Long) =
        Util.scaleLargeTimestamp(bytes * 8, C.MICROS_PER_SECOND, rateHz.toLong())

    private inner class SeekByPair(private val durationUs: Long) : SeekMap {
        private val lastPair = maxOf(0L, fullPairs + (if (tailBytes > 0) 1 else 0) - 1)

        override fun isSeekable() = true

        override fun getDurationUs() = durationUs

        override fun getSeekPoints(timeUs: Long): SeekMap.SeekPoints {
            val pair =
                (timeUs.coerceAtLeast(0) * rateHz / (8 * C.MICROS_PER_SECOND * BLOCK)).coerceAtMost(
                    lastPair
                )
            return SeekMap.SeekPoints(SeekPoint(timeOfPair(pair), dataStart + pair * PAIR))
        }
    }

    override fun seek(position: Long, timeUs: Long) {
        samplePending = 0
    }

    override fun release() {}

    private companion object {
        const val DSD_CHUNK_BYTES = 28
        const val CHUNK_HEADER_BYTES = 12
        /** Bytes per channel in one block. */
        const val BLOCK = 4096
        const val PAIR = 2 * BLOCK
        /** Block pairs per sample: 64 KiB, the track's write size. */
        const val MAX_PAIRS = 8
        const val CONTAINER_MIME = "audio/x-dsf"
    }
}
