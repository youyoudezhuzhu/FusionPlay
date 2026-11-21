package com.fusionplay.android

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.Tracks
import androidx.media3.extractor.metadata.id3.Id3Frame
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import com.airplayreceiver.desktop.backend.FusionPlayDiagnosticLogger
import com.airplayreceiver.desktop.backend.MediaSource
import java.io.Closeable
import java.io.File

internal class NativeNetworkPlayer(
    context: Context,
    private val diagnostics: FusionPlayDiagnosticLogger,
    private val onDlnaStateChanged: () -> Unit = {},
    private val onError: (String) -> Unit = {},
) : Closeable {
    private companion object {
        val lyricFrameIds = setOf("USLT", "SYLT", "ULT", "SLT")
    }

    private val applicationContext = context.applicationContext
    private val mainHandler = Handler(Looper.getMainLooper())
    private var player: ExoPlayer? = null
    private val publishDlnaState = Runnable(onDlnaStateChanged)
    private var embeddedCover: File? = null
    val embeddedArtworkPath: String? get() = embeddedCover?.absolutePath

    private fun createPlayer(): ExoPlayer = ExoPlayer.Builder(applicationContext).build().apply {
        setAudioAttributes(
            AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(),
            // FusionPlayMediaChannel owns system audio focus for all receivers.
            // A second request here can revoke our own focus and pause DLNA.
            /* handleAudioFocus = */ false,
        )
        addListener(object : Player.Listener {
            @androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
            override fun onTracksChanged(tracks: Tracks) {
                if (source != MediaSource.DLNA) return
                val observed = mutableSetOf<String>()
                var audioTracks = 0
                for (group in tracks.groups) {
                    if (group.type != C.TRACK_TYPE_AUDIO) continue
                    for (index in 0 until group.length) {
                        if (!group.isTrackSelected(index)) continue
                        audioTracks++
                        val tags = group.getTrackFormat(index).metadata ?: continue
                        for (i in 0 until tags.length()) {
                            val id = (tags[i] as? Id3Frame)?.id ?: continue
                            if (id in lyricFrameIds) observed.add(id)
                        }
                    }
                }
                if (audioTracks > 0) {
                    recordPlayback("decoder_id3_lyrics", mapOf("audio_tracks" to audioTracks,
                        "frames" to observed.sorted().joinToString(","), "lyrics_frame_present" to observed.isNotEmpty()))
                }
            }

            override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
                recordPlayback("decoder_play_intent", mapOf("play_when_ready" to playWhenReady, "reason_code" to reason))
            }

            override fun onIsPlayingChanged(isPlaying: Boolean) {
                playing = isPlaying
                recordPlayback("decoder_playing", mapOf("playing" to isPlaying))
            }

            override fun onPlaybackStateChanged(playbackState: Int) {
                ready = playbackState == Player.STATE_READY
                recordPlayback("decoder_state", mapOf("state" to when (playbackState) {
                    Player.STATE_BUFFERING -> "buffering"
                    Player.STATE_READY -> "ready"
                    Player.STATE_ENDED -> "ended"
                    else -> "idle"
                }))
            }

            override fun onMediaMetadataChanged(mediaMetadata: MediaMetadata) {
                if (source != MediaSource.DLNA || embeddedCover != null) return
                val bytes = mediaMetadata.artworkData ?: return
                if (bytes.isEmpty() || bytes.size > 1024 * 1024) return
                runCatching {
                    val file = File.createTempFile("dlna-cover-", ".img", applicationContext.cacheDir)
                    try {
                        file.writeBytes(bytes)
                        embeddedCover = file
                    } finally {
                        if (embeddedCover != file) file.delete()
                    }
                }.onFailure { recordPlayback("decoder_embedded_cover", mapOf("available" to false)) }
            }

            override fun onEvents(player: Player, events: Player.Events) {
                if (source == MediaSource.DLNA && events.containsAny(
                        Player.EVENT_PLAYBACK_STATE_CHANGED,
                        Player.EVENT_PLAY_WHEN_READY_CHANGED,
                        Player.EVENT_IS_PLAYING_CHANGED,
                        Player.EVENT_POSITION_DISCONTINUITY,
                        Player.EVENT_MEDIA_METADATA_CHANGED,
                    )
                ) {
                    mainHandler.removeCallbacks(publishDlnaState)
                    mainHandler.post(publishDlnaState)
                }
            }

            override fun onPlayerError(error: PlaybackException) {
                recordPlayback("decoder_error", mapOf("code" to error.errorCodeName))
                onError("${error.errorCodeName}：${error.message ?: "未知播放器错误"}")
            }
        })
        volume = if (dlnaMuted) 0f else dlnaGain
    }

    private fun recordPlayback(event: String, details: Map<String, Any?>) {
        val component = when (source) {
            MediaSource.DLNA -> "dlna"
            MediaSource.AIRPLAY -> "airplay"
            else -> return
        }
        diagnostics.write(component, event, if (event == "decoder_error") "failure" else "observed",
            details + ("epoch" to sourceEpoch))
    }

    var source: MediaSource? = null
        private set
    var sourceEpoch: Long? = null
        private set
    var url: String? = null
        private set
    var playing: Boolean = false
        private set
    var ready: Boolean = false
        private set
    var rate: Double = 1.0
        private set
    private var dlnaGain = 1f
    private var dlnaMuted = false
    val positionMs: Long get() = player?.currentPosition?.coerceAtLeast(0) ?: 0
    val durationMs: Long get() = player?.duration?.takeIf { it > 0 } ?: 0

    fun open(source: MediaSource, url: String, epoch: Long?, positionMs: Long, autoPlay: Boolean) = onMain {
        val activePlayer = player ?: createPlayer().also { player = it }
        clearEmbeddedCover()
        if (this.source != source || sourceEpoch != epoch) {
            dlnaMuted = false
            dlnaGain = 1f
            activePlayer.volume = 1f
        }
        this.source = source
        this.url = url
        sourceEpoch = epoch
        rate = 1.0
        activePlayer.playbackParameters = PlaybackParameters.DEFAULT
        activePlayer.setMediaItem(MediaItem.fromUri(url), positionMs.coerceAtLeast(0))
        activePlayer.prepare()
        activePlayer.playWhenReady = autoPlay
    }

    fun play() = onMain { player?.play() }
    fun pause() = onMain { player?.pause() }
    fun seek(positionMs: Long) = onMain { player?.seekTo(positionMs.coerceAtLeast(0)) }
    fun setRate(value: Double) = onMain {
        rate = value.coerceAtLeast(0.0)
        val activePlayer = player ?: return@onMain
        if (rate == 0.0) activePlayer.pause() else {
            activePlayer.playbackParameters = PlaybackParameters(rate.toFloat())
            activePlayer.play()
        }
    }
    fun setDlnaVolume(fallbackPercent: Int?, muted: Boolean?) = onMain {
        if (source != MediaSource.DLNA) return@onMain
        dlnaGain = (fallbackPercent?.coerceIn(0, 100) ?: 100) / 100f
        if (muted != null) dlnaMuted = muted
        player?.volume = if (dlnaMuted) 0f else dlnaGain
    }

    private fun clearEmbeddedCover() {
        embeddedCover?.delete()
        embeddedCover = null
    }

    fun stop() = onMain {
        mainHandler.removeCallbacks(publishDlnaState)
        clearEmbeddedCover()
        player?.release()
        player = null
        source = null
        url = null
        sourceEpoch = null
        playing = false
        ready = false
    }

    override fun close() { stop() }

    private fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else mainHandler.post(block)
    }
}
