/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.media3.common.Format
import androidx.media3.exoplayer.audio.DefaultAudioSink
import com.encelerate.audigmaplayer.audio.AudigmaAudioSink
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Debug-only: feeds a DSF file to [AudigmaAudioSink] by hand, the way a renderer would, and runs
 * control operations against it while it plays. Logs under tag `DSDSELFTEST`.
 *
 * ```
 * am broadcast -n com.encelerate.audigmaplayer.debug/com.encelerate.audigmaplayer.debug.DsdSelfTest \
 *   --es file full.dsf --es script "play:6000,pause:1500,play:4000,seek:30:4000,end"
 * ```
 *
 * The file is read from the app's `files/` directory. Script steps, separated by commas:
 * - `play:<ms>`: sink.play(), then feed for that long (also resumes after `pause`).
 * - `pause:<ms>`: sink.pause(), stop feeding, wait.
 * - `seek:<sec>:<ms>`: sink.flush() while playing, continue from that position, feed for ms.
 * - `flushp:<sec>`: flush while the writer is blocked mid-write, no wait (stress).
 * - `end`: playToEndOfStream and wait for isEnded, then release.
 * - `release`: release the sink while playing.
 *
 * Every control call is timed; anything above 100 ms is logged as SLOW.
 */
class DsdSelfTest : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val file = intent.getStringExtra("file") ?: "full.dsf"
        val script = intent.getStringExtra("script") ?: "play:5000,end"
        val pending = goAsync()
        Thread(
                {
                    try {
                        run(context, File(context.filesDir, file), script)
                    } catch (t: Throwable) {
                        Log.e(TAG, "FAILED", t)
                    } finally {
                        pending.finish()
                    }
                },
                "dsd-selftest",
            )
            .start()
    }

    private fun timed(what: String, block: () -> Unit) {
        val t0 = System.nanoTime()
        block()
        val ms = (System.nanoTime() - t0) / 1_000_000
        Log.i(TAG, "$what took $ms ms" + if (ms > 100) " SLOW" else "")
    }

    private fun run(context: Context, path: File, script: String) {
        val f = RandomAccessFile(path, "r")
        val hdr = ByteArray(64).also { f.readFully(it) }
        val le = ByteBuffer.wrap(hdr).order(ByteOrder.LITTLE_ENDIAN)
        // DSD chunk 28 B, then fmt chunk: channel num at +24, sampling frequency at +28.
        val channels = le.getInt(52)
        val rate = le.getInt(56)
        require(channels == 2) { "not stereo" }
        val dataStart = 0x5cL
        val pairs = (f.length() - dataStart) / PAIR
        Log.i(TAG, "file=$path rate=$rate pairs=$pairs script=$script")

        val sink = AudigmaAudioSink("selftest", DefaultAudioSink.Builder(context).build()) { 1f }
        val format =
            Format.Builder()
                .setSampleMimeType(AudigmaAudioSink.MIME_DSD_DSF)
                .setSampleRate(rate)
                .setChannelCount(2)
                .build()
        Log.i(TAG, "support=${sink.getFormatSupport(format)}")
        sink.configure(format, 0, null)

        var pair = 0L
        val chunk = ByteBuffer.allocateDirect(4 * PAIR)
        var chunkPending = false
        val bytesPerUs = rate / 4.0 / 1_000_000
        var ended = false

        fun seekTo(sec: Long) {
            pair = (sec * rate / 4 / PAIR).coerceIn(0, pairs - 4)
            chunkPending = false
        }

        fun feedFor(ms: Long) {
            val end = System.nanoTime() + ms * 1_000_000
            var refused = 0
            while (System.nanoTime() < end) {
                if (!chunkPending) {
                    chunk.clear()
                    val n = minOf(4L, pairs - pair).toInt()
                    if (n <= 0) return
                    f.seek(dataStart + pair * PAIR)
                    val arr = ByteArray(n * PAIR)
                    f.readFully(arr)
                    chunk.put(arr).flip()
                    pair += n
                    chunkPending = true
                }
                val presentationUs = (pair * PAIR / bytesPerUs).toLong()
                if (sink.handleBuffer(chunk, presentationUs, 1)) {
                    chunkPending = false
                } else {
                    refused++
                    Thread.sleep(5)
                }
                sink.getCurrentPositionUs(false)
            }
            Log.i(
                TAG,
                "fed until pair=$pair pos=${sink.getCurrentPositionUs(false) / 1000} ms " +
                    "refused=$refused",
            )
        }

        for (step in script.split(',')) {
            val p = step.split(':')
            Log.i(TAG, "STEP $step")
            when (p[0]) {
                "play" -> {
                    timed("play") { sink.play() }
                    feedFor(p[1].toLong())
                }
                "pause" -> {
                    timed("pause") { sink.pause() }
                    Thread.sleep(p[1].toLong())
                    Log.i(TAG, "pos after pause ${sink.getCurrentPositionUs(false) / 1000} ms")
                }
                "seek" -> {
                    timed("flush") { sink.flush() }
                    seekTo(p[1].toLong())
                    feedFor(p[2].toLong())
                }
                "flushp" -> {
                    timed("flush") { sink.flush() }
                    seekTo(p[1].toLong())
                }
                "end" -> {
                    sink.playToEndOfStream()
                    val t0 = System.nanoTime()
                    while (!sink.isEnded && System.nanoTime() - t0 < 10_000_000_000L) {
                        Thread.sleep(10)
                    }
                    ended = sink.isEnded
                    Log.i(TAG, "isEnded=$ended")
                }
                "release" -> {}
                else -> Log.e(TAG, "unknown step $step")
            }
        }
        timed("release") { sink.release() }
        f.close()
        Log.i(TAG, "DONE")
    }

    private companion object {
        const val TAG = "DSDSELFTEST"
        const val PAIR = 8192
    }
}
