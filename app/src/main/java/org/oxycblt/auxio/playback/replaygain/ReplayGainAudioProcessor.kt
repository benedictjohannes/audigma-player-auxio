/*
 * Copyright (c) 2022 Auxio Project
 * ReplayGainAudioProcessor.kt is part of Auxio.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
 
package org.oxycblt.auxio.playback.replaygain

import javax.inject.Inject
import kotlin.math.pow
import org.oxycblt.auxio.playback.PlaybackSettings
import org.oxycblt.auxio.playback.state.PlaybackStateManager
import org.oxycblt.auxio.playback.state.QueueChange
import org.oxycblt.musikr.Album
import org.oxycblt.musikr.MusicParent
import org.oxycblt.musikr.Song
import timber.log.Timber as L

/**
 * Tracks the ReplayGain adjustment of the current song. The audio sink
 * (`com.encelerate.audigmaplayer.audio.AudigmaAudioSink`) applies [volume] to the bitstream itself,
 * which allows the use of positive ReplayGain values. This used to be a Media3 `AudioProcessor`,
 * but `DefaultAudioSink` skips such processors when it outputs float PCM, which bit-perfect
 * high-resolution playback needs.
 *
 * @author Alexander Capehart (OxygenCobalt)
 */
class ReplayGainAudioProcessor
@Inject
constructor(
    private val playbackManager: PlaybackStateManager,
    private val playbackSettings: PlaybackSettings,
) : PlaybackStateManager.Listener, PlaybackSettings.Listener {
    /** Linear gain for the current song; 1 means no adjustment. Read from the playback thread. */
    @Volatile
    var volume = 1f
        private set

    fun attach() {
        playbackManager.addListener(this)
        playbackSettings.registerListener(this)
    }

    /** Remove this instance from the components required for it to function correctly. */
    fun release() {
        playbackManager.removeListener(this)
        playbackSettings.unregisterListener(this)
    }

    // --- OVERRIDES ---

    override fun onIndexMoved(index: Int) {
        L.d("Index moved, updating current song")
        applyReplayGain(playbackManager.currentSong)
    }

    override fun onQueueChanged(queue: List<Song>, index: Int, change: QueueChange) {
        // Other types of queue changes preserve the current song.
        if (change.type == QueueChange.Type.SONG) {
            applyReplayGain(playbackManager.currentSong)
        }
    }

    override fun onNewPlayback(
        parent: MusicParent?,
        queue: List<Song>,
        index: Int,
        isShuffled: Boolean,
    ) {
        L.d("New playback started, updating playback information")
        applyReplayGain(playbackManager.currentSong)
    }

    override fun onReplayGainSettingsChanged() {
        // ReplayGain config changed, we need to set it up again.
        applyReplayGain(playbackManager.currentSong)
    }

    // --- REPLAYGAIN PARSING ---

    /**
     * Updates the volume adjustment based on the given [Format].
     *
     * @param song The [Format] of the currently playing track, or null if nothing is playing.
     */
    private fun applyReplayGain(song: Song?) {
        if (song == null) {
            L.d("Nothing playing, disabling adjustment")
            volume = 1f
            return
        }

        L.d("Applying ReplayGain adjustment for $song")

        val gain = song.replayGainAdjustment
        val preAmp = playbackSettings.replayGainPreAmp

        // ReplayGain is configurable, so determine what to do based off of the mode.
        val resolvedAdjustment =
            when (playbackSettings.replayGainMode) {
                // User wants no adjustment.
                ReplayGainMode.OFF -> {
                    L.d("ReplayGain is off")
                    null
                }
                // User wants track gain to be preferred. Default to album gain only if
                // there is no track gain.
                ReplayGainMode.TRACK -> {
                    L.d("Using track strategy")
                    gain.track ?: gain.album
                }
                // User wants album gain to be preferred. Default to track gain only if
                // here is no album gain.
                ReplayGainMode.ALBUM -> {
                    L.d("Using album strategy")
                    gain.album ?: gain.track
                }
                // User wants album gain to be used when in an album, track gain otherwise.
                ReplayGainMode.DYNAMIC -> {
                    L.d("Using dynamic strategy")
                    gain.album?.takeIf {
                        playbackManager.parent is Album &&
                            playbackManager.currentSong?.album == playbackManager.parent
                    } ?: gain.track
                }
            }

        val amplifiedAdjustment =
            if (resolvedAdjustment != null) {
                // Successfully resolved an adjustment, apply the corresponding pre-amp
                L.d("Applying with pre-amp")
                resolvedAdjustment + preAmp.with
            } else {
                // No adjustment found, use the corresponding user-defined pre-amp
                L.d("Applying without pre-amp")
                preAmp.without
            }

        L.d("Applying ReplayGain adjustment ${amplifiedAdjustment}db")

        // Final adjustment along the volume curve.
        volume = 10f.pow(amplifiedAdjustment / 20f)
    }
}
