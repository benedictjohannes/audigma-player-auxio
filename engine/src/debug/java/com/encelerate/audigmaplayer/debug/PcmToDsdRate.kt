/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import com.encelerate.audigmaplayer.audio.DsdSettings
import com.encelerate.audigmaplayer.audio.DsdSettingsStore

/**
 * Debug-only: sets the multiplier and rate family of [DsdSettingsStore] without the settings screen
 * (the other options stay as they are). Persisted like the screen's values; applies from the next
 * track. Logs under tag `PCMTODSD`.
 *
 * ```
 * am broadcast -n org.oxycblt.auxio.debug/com.encelerate.audigmaplayer.debug.PcmToDsdRate \
 *   --ei dsd 128 [--ei family 48]
 * ```
 *
 * `dsd`: 64, 128 or 256; 0 plays PCM bit-perfect. `family`: 44 (default), 48 or 0 (follow source).
 */
class PcmToDsdRate : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        DsdSettingsStore.init(context)
        val dsd = intent.getIntExtra("dsd", -1)
        if (dsd !in setOf(0, 64, 128, 256)) {
            Log.e(TAG, "dsd must be 0, 64, 128 or 256, got $dsd")
            return
        }
        val family =
            when (intent.getIntExtra("family", 44)) {
                48 -> DsdSettings.FAMILY_48000
                0 -> DsdSettings.FAMILY_SOURCE
                else -> DsdSettings.FAMILY_44100
            }
        DsdSettingsStore.update { it.copy(multiplier = dsd, rateFamily = family) }
        Log.i(TAG, "PCM to DSD: ${DsdSettingsStore.current}, from the next track")
    }

    private companion object {
        const val TAG = "PCMTODSD"
    }
}
