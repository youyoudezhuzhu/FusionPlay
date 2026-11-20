package com.airplayreceiver.desktop.backend

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AirPlayMetadataContinuityTest {
    @Test
    fun titleLyricIsPublishedWithoutResettingClock() {
        val current = AppState(activeMediaSource = MediaSource.AIRPLAY,
            playback = PlaybackSnapshot(protocol = "AirPlay", title = "上一句", positionMs = 42000))
        val event = AppEventParser.parse(
            """{"type":"now_playing","source":"airplay","title":"晴天","artist":"周杰伦","lyric_line":"故事的小黄花","track_changed":false}"""
        ) as AppEvent.NowPlaying
        val result = AppStateTransitions.nowPlaying(current, event).playback
        assertEquals("晴天", result.title)
        assertEquals("周杰伦", result.artist)
        assertEquals("故事的小黄花", result.lyrics?.lines?.single()?.text)
        assertEquals(42000L, result.positionMs)
    }

    @Test
    fun stableDmapIdKeepsProgressThroughTitleLines() {
        val current = AppState(
            activeMediaSource = MediaSource.AIRPLAY,
            playback = PlaybackSnapshot(protocol = "AirPlay", title = "Line A",
                trackIdentity = "abcd", positionMs = 42000, sourceEpoch = 7),
        )
        val event = AppEventParser.parse(
            """{"type":"now_playing","source":"airplay","epoch":7,"track_id":"abcd","title":"Line B"}"""
        ) as AppEvent.NowPlaying
        val next = AppStateTransitions.nowPlaying(current, event)
        assertEquals("Line B", next.playback.title)
        assertEquals(42000L, next.playback.positionMs)
        assertEquals(0L, AppStateTransitions.nowPlaying(next, event.copy(trackId = "def0")).playback.positionMs)
    }

    @Test
    fun dlnaRelativeCoverResolvesWithEmbeddedLyrics() {
        val event = AppEventParser.parse(
            """{"type":"dlna_media","source":"dlna","url":"https://example.test/music/a.mp3","title":"Track","artwork_url":"../art/a.jpg","lyrics_text":"[00:01.00]Line A"}"""
        ) as AppEvent.DlnaMedia
        val next = AppStateTransitions.dlnaMedia(AppState(activeMediaSource = MediaSource.DLNA), event)
        assertEquals("https://example.test/art/a.jpg", next.playback.coverArt)
        assertEquals("Line A", next.playback.lyrics?.lines?.first()?.text)
    }

    @Test
    fun dlnaCoverAcceptsSpacesWithoutReencodingSignedQuery() {
        val event = AppEventParser.parse(
            """{"type":"dlna_media","url":"http://example.test/My Music/a.mp3","artwork_url":"../Cover Art/a.jpg?token=a%2Fb&size=300"}"""
        ) as AppEvent.DlnaMedia
        val next = AppStateTransitions.dlnaMedia(AppState(activeMediaSource = MediaSource.DLNA), event)
        assertEquals("http://example.test/Cover%20Art/a.jpg?token=a%2Fb&size=300", next.playback.coverArt)
    }

    @Test
    fun trackTakeoverKeepsVisualMetadataAndControlsUntilFreshEventsArrive() {
        val playback = PlaybackSnapshot(
            title = "Old title",
            artist = "Old artist",
            album = "Old album",
            coverArt = "/old-cover.png",
            protocol = "AirPlay",
            durationMs = 180_000,
            positionMs = 72_000,
            isPlaying = true,
            streamActive = true,
            sourceEpoch = 40,
        )
        val controls = RemoteControlState(
            available = true,
            commands = setOf("play", "pause", "previous_track", "next_track"),
        )
        val current = AppState(
            activeMediaSource = MediaSource.AIRPLAY,
            selectedCoreMediaSource = MediaSource.AIRPLAY,
            playback = playback,
            remoteControl = controls,
            sourcePlaybackStates = mapOf(
                MediaSource.AIRPLAY to SourcePlaybackState(playback, controls),
            ),
        )

        val takeover = AppStateTransitions.sourceTakeover(
            current,
            AppEvent.SourceTakeover(
                source = "airplay",
                mediaKind = "audio",
                epoch = 41,
                previousSource = "airplay",
                previousMediaKind = "audio",
                previousEpoch = 40,
                reason = "airplay_audio_replacement",
                rawJson = "{}",
            ),
        )
        val metadata = AppStateTransitions.nowPlaying(
            takeover,
            AppEvent.NowPlaying(
                title = "New title",
                artist = "New artist",
                album = "New album",
                genre = null,
                durationMs = 200_000,
                rawJson = "{}",
                source = "airplay",
                epoch = 41,
            ),
        )

        assertEquals("New title", metadata.playback.title)
        assertEquals("/old-cover.png", metadata.playback.coverArt)
        assertEquals(controls, metadata.remoteControl)
        assertTrue(metadata.remoteControl.available)
    }
}
