/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.audio

import android.content.Context
import android.content.SharedPreferences

/**
 * The PCM to DSD options. The values are audigma's own setting keys (`resampler.modulator_ntf`
 * ...); the glue passes them to audigma's `Settings` as they are, so audigma validates them and
 * this class holds none of its rules. Defaults match the settings screen.
 *
 * @param multiplier 0: PCM plays bit-perfect; 64, 128 or 256: convert to that DSD multiplier.
 * @param rateFamily `target.rate_family`: [FAMILY_44100], [FAMILY_SOURCE] or [FAMILY_48000].
 * @param headroomDb `gain.preamp_db`, on top of audigma's own per-rate headroom.
 */
data class DsdSettings(
    val multiplier: Int = 0,
    val rateFamily: String = FAMILY_44100,
    val ntf: String = "reference",
    val modulatorOrder: Int = 5,
    val phase: String = "linear",
    val apodization: String = "off",
    val headroomDb: Double = DEFAULT_HEADROOM_DB,
) {
    val enabled: Boolean
        get() = multiplier != 0

    /** The `key=value` lines of `nativeCreate`'s options (glue `options.rs`). */
    fun toNative() =
        "multiplier=dsd$multiplier\nfamily=$rateFamily\nntf=$ntf\norder=$modulatorOrder\n" +
            "phase=$phase\napodization=$apodization\npreamp_db=$headroomDb\n"

    companion object {
        const val FAMILY_44100 = "44100"
        const val FAMILY_SOURCE = "source"
        const val FAMILY_48000 = "48000"

        /**
         * -3 dB of ReplayGain preamp cured the crackle of loud passages at DSD256 on the JM21; it
         * was not measured at any other value.
         */
        const val DEFAULT_HEADROOM_DB = -3.5
    }
}

/**
 * Process-wide [DsdSettings], persisted in the app's private preferences (not Auxio's settings).
 * The sink reads [current] at every `configure`, so a change applies from the next track.
 */
object DsdSettingsStore {
    private const val FILE = "audigma_dsd_settings"

    private lateinit var prefs: SharedPreferences

    /** The settings now; defaults until [init] ran. */
    @Volatile
    var current = DsdSettings()
        private set

    /** Loads the stored settings; call once with any context before reading [current]. */
    @Synchronized
    fun init(context: Context) {
        if (::prefs.isInitialized) return
        prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        val d = DsdSettings()
        current =
            DsdSettings(
                multiplier = prefs.getInt("multiplier", d.multiplier),
                rateFamily = prefs.getString("rate_family", d.rateFamily) ?: d.rateFamily,
                ntf = prefs.getString("ntf", d.ntf) ?: d.ntf,
                modulatorOrder = prefs.getInt("modulator_order", d.modulatorOrder),
                phase = prefs.getString("phase", d.phase) ?: d.phase,
                apodization = prefs.getString("apodization", d.apodization) ?: d.apodization,
                headroomDb = prefs.getFloat("headroom_db", d.headroomDb.toFloat()).toDouble(),
            )
    }

    /** Replaces the settings and persists them. */
    @Synchronized
    fun update(transform: (DsdSettings) -> DsdSettings) {
        val s = transform(current)
        current = s
        if (::prefs.isInitialized) {
            prefs
                .edit()
                .putInt("multiplier", s.multiplier)
                .putString("rate_family", s.rateFamily)
                .putString("ntf", s.ntf)
                .putInt("modulator_order", s.modulatorOrder)
                .putString("phase", s.phase)
                .putString("apodization", s.apodization)
                .putFloat("headroom_db", s.headroomDb.toFloat())
                .apply()
        }
    }
}
