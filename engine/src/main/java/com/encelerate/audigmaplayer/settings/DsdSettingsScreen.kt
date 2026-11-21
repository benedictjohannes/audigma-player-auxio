/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.settings

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import com.encelerate.audigmaplayer.audio.DsdSettings
import com.encelerate.audigmaplayer.audio.DsdSettingsStore
import kotlin.math.roundToInt

/** One selectable value of a setting: [value] is what goes to audigma, the rest is our copy. */
private class Choice<T>(
    val value: T,
    val label: String,
    val summary: String,
    /** Shown in the error colour while this choice is selected. */
    val warning: String? = null,
)

private val MULTIPLIERS =
    listOf(
        Choice(0, "Off", "Bit-perfect PCM. Everything below is ignored."),
        Choice(64, "DSD64", "2.8 MHz. Least CPU and battery."),
        Choice(128, "DSD128", "5.6 MHz."),
        Choice(256, "DSD256", "11.3 MHz. Most CPU and battery."),
    )

private val FAMILIES =
    listOf(
        Choice(
            DsdSettings.FAMILY_44100,
            "44.1 kHz family",
            "Sets DSD multiplier to 44.1 kHz rate family. Most compatible.",
        ),
        Choice(
            DsdSettings.FAMILY_SOURCE,
            "Follow track",
            "DSD multiplier follows track's rate family. Device might not support this.",
            "Tracks in the 48 kHz family (48, 96, 192 kHz) can play silent or crackle on the " +
                "FiiO JM21. Switch back to 44.1 kHz family to clear it.",
        ),
        Choice(
            DsdSettings.FAMILY_48000,
            "48 kHz family",
            "Sets DSD multiplier to 48 kHz rate family. Device might not support this.",
            "The FiiO JM21 does not play the 48 kHz family: expect silence or crackle. Switch " +
                "back to 44.1 kHz family to clear it.",
        ),
    )

private val NTFS =
    listOf(
        Choice(
            "reference",
            "Reference Grade",
            "Noise below −141 dBFS with the least volume reduction that still meets it, adapts " +
                "H∞ per rate (DSD64 uses 1.40). Recommended.",
        ),
        Choice(
            "comfortable",
            "Comfortable",
            "Dynamic range ≥ 120 dB and spurs ≤ −120 dBFS, adapts H∞ per rate.",
        ),
        Choice(
            "fixed_h_inf_1_4",
            "Fixed H∞ at 1.40 for every rate",
            "Conservative design, quieter at most rates.",
        ),
        Choice(
            "fixed_h_inf_1_5",
            "Fixed H∞ at 1.50 for every rate",
            "Classic textbook design, quietest at every rate.",
        ),
    )

private val ORDERS =
    listOf(
        Choice(5, "5th order", "Recommended. 7th order shows no consistent benefit at any rate."),
        Choice(7, "7th order", "About 10% more CPU, no consistent benefit measured."),
    )

private val PHASES =
    listOf(
        Choice("linear", "Linear phase", "Pre- and post-ringing, no phase shift."),
        Choice("minimum", "Minimum phase", "No pre-ringing; uneven phase near the top of the band."),
    )

private val APODIZATIONS =
    listOf(
        Choice("off", "Off", "Flat to 20 kHz, steepest cutoff."),
        Choice("gentle", "Gentle", "Rolls off from 19 kHz, -0.8 dB at 20 kHz. Less ringing."),
        Choice("18500", "18.5 kHz edge", "-2.8 dB at 20 kHz."),
        Choice("18000", "18 kHz edge", "-5.7 dB at 20 kHz. Strongest roll-off."),
        Choice("20000", "20 kHz edge", "Flat at 20 kHz, longest filter, more CPU."),
    )

private const val HEADROOM_MIN = -12f
private const val HEADROOM_STEP = 0.5f

/** The PCM to DSD settings; persisted by [DsdSettingsStore], applied from the next track. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DsdSettingsScreen(onBack: () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()
    ) {
        var settings by remember { mutableStateOf(DsdSettingsStore.current) }
        fun change(transform: (DsdSettings) -> DsdSettings) {
            DsdSettingsStore.update(transform)
            settings = DsdSettingsStore.current
        }
        Scaffold(
            containerColor = MaterialTheme.colorScheme.surface,
            topBar = {
                TopAppBar(
                    title = { Text("PCM to DSD") },
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    },
                )
            },
        ) { padding ->
            Column(
                Modifier.padding(padding)
                    .verticalScroll(rememberScrollState())
                    .padding(bottom = 24.dp)
            ) {
                Text(
                    "Changes apply from the next track.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
                Section(
                    "PCM to DSD",
                    MULTIPLIERS,
                    settings.multiplier,
                    "Offload ΔΣ modulation to the CPU instead of the DAC, trading battery and " +
                        "CPU usage for a modulation stage you can tune.\n" +
                        "At DSD64, software modulation noise performance can fall short of the " +
                        "audibility threshold.\n" +
                        "At DSD128 and above, software modulation can match or exceed hardware " +
                        "DAC filters.",
                ) { m ->
                    change { it.copy(multiplier = m) }
                }
                if (!settings.enabled) return@Column
                Section("DSD rate family", FAMILIES, settings.rateFamily) { f ->
                    change { it.copy(rateFamily = f) }
                }
                Section(
                    "Modulator noise shaping",
                    NTFS,
                    settings.ntf,
                    "DSD modulation needs volume reduction to prevent overload. Reduced volume " +
                        "versus SNR and SINAD performance is a mathematical tradeoff that drives " +
                        "the H∞ value.",
                ) { n ->
                    change { it.copy(ntf = n) }
                }
                Section("Modulator", ORDERS, settings.modulatorOrder) { o ->
                    change { it.copy(modulatorOrder = o) }
                }
                Section("Filter phase", PHASES, settings.phase) { p ->
                    change { it.copy(phase = p) }
                }
                Section("Filter roll-off", APODIZATIONS, settings.apodization) { a ->
                    change { it.copy(apodization = a) }
                }
                Headroom(settings.headroomDb) { db -> change { it.copy(headroomDb = db) } }
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                Text(
                    buildAnnotatedString {
                        append("ΔΣ modulation powered by ")
                        withLink(
                            LinkAnnotation.Url(
                                "https://github.com/benedictjohannes/audigma",
                                TextLinkStyles(
                                    SpanStyle(
                                        color = MaterialTheme.colorScheme.primary,
                                        textDecoration = TextDecoration.Underline,
                                    )
                                ),
                            )
                        ) {
                            append("Audigma")
                        }
                    },
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
        }
    }
}

@Composable
private fun <T> Section(
    title: String,
    choices: List<Choice<T>>,
    selected: T,
    description: String? = null,
    onSelect: (T) -> Unit,
) {
    HorizontalDivider(Modifier.padding(vertical = 8.dp))
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
    )
    if (description != null) {
        Text(
            description,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
    }
    for (choice in choices) {
        Row(
            Modifier.fillMaxWidth()
                .selectable(choice.value == selected, role = Role.RadioButton) {
                    onSelect(choice.value)
                }
                .padding(horizontal = 16.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RadioButton(choice.value == selected, onClick = null)
            Column(Modifier.padding(start = 16.dp)) {
                Text(choice.label, style = MaterialTheme.typography.bodyLarge)
                Text(
                    choice.summary,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (choice.value == selected && choice.warning != null) {
            Text(
                choice.warning,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(start = 56.dp, end = 16.dp, top = 2.dp, bottom = 6.dp),
            )
        }
    }
}

@Composable
private fun Headroom(db: Double, onChange: (Double) -> Unit) {
    HorizontalDivider(Modifier.padding(vertical = 8.dp))
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            "DSD headroom",
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        TextButton(
            onClick = { onChange(DsdSettings.DEFAULT_HEADROOM_DB) },
            enabled = db != DsdSettings.DEFAULT_HEADROOM_DB,
            contentPadding = PaddingValues(horizontal = 8.dp),
            modifier = Modifier.size(width = 96.dp, height = 36.dp),
        ) {
            Text("Reset")
        }
    }
    Text(
        "%.1f dB".format(db),
        style = MaterialTheme.typography.bodyLarge,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    Slider(
        value = db.toFloat(),
        onValueChange = { onChange((it / HEADROOM_STEP).roundToInt() * HEADROOM_STEP.toDouble()) },
        valueRange = HEADROOM_MIN..0f,
        steps = (-HEADROOM_MIN / HEADROOM_STEP).roundToInt() - 1,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    Text(
        "Level reduction before the modulator, on top of the player's own headroom (about " +
            "-2.25 dB at DSD256, -3.75 dB at DSD128 for 44.1 kHz tracks). Loud masters can " +
            "crackle on some DACs; set a larger reduction to prevent it. -3 dB is a " +
            "conservative reduction that should be enough.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
}
