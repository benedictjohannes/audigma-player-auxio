/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (c) 2026 Audigma Player Project
 */
package com.encelerate.audigmaplayer.settings

import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.fragment.app.Fragment
import com.encelerate.audigmaplayer.audio.DsdSettingsStore

/** Hosts [DsdSettingsScreen]; Auxio's settings navigate to it. */
class DsdSettingsFragment : Fragment() {
    override fun onCreateView(
        inflater: LayoutInflater,
        container: ViewGroup?,
        savedInstanceState: Bundle?,
    ): View {
        DsdSettingsStore.init(requireContext())
        return ComposeView(requireContext()).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent {
                DsdSettingsScreen(
                    onBack = { requireActivity().onBackPressedDispatcher.onBackPressed() }
                )
            }
        }
    }
}
