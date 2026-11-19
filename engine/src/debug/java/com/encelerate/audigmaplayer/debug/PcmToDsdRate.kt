/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import com.encelerate.audigmaplayer.audio.AudigmaAudioSink

/**
 * Debug-only: sets [AudigmaAudioSink.pcmToDsdRateHz] until the settings UI exists. Not persisted;
 * applies from the next track (the next sink `configure`). Logs under tag `PCMTODSD`.
 *
 * ```
 * am broadcast -n org.oxycblt.auxio.debug/com.encelerate.audigmaplayer.debug.PcmToDsdRate \
 *   --ei dsd 128
 * ```
 *
 * `dsd`: 64, 128 or 256; 0 plays PCM bit-perfect.
 */
class PcmToDsdRate : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val dsd = intent.getIntExtra("dsd", -1)
        val hz =
            when (dsd) {
                0 -> 0
                64,
                128,
                256 -> dsd / 64 * 2_822_400
                else -> {
                    Log.e(TAG, "dsd must be 0, 64, 128 or 256, got $dsd")
                    return
                }
            }
        AudigmaAudioSink.pcmToDsdRateHz = hz
        Log.i(TAG, "PCM to DSD rate: $hz Hz, from the next track")
    }

    private companion object {
        const val TAG = "PCMTODSD"
    }
}
