//! Runtime state projection: protocol/core events -> [`AppStateView`].
//! Ownership: host-state (T2).
//!
//! The Android implementation keeps one playback snapshot per source
//! (`SourcePlaybackProjection`) and projects the active source into the UI. The
//! Windows sidecar uses the same model internally: every source gets a cached
//! [`SourceSnapshot`], while `AppStateView.playback` always mirrors the current
//! foreground source. Writes come from several protocol/audio threads, so all
//! state is guarded by a `Mutex` and every method takes `&self`.
//!
//! Two semantics matter most and are covered by unit tests:
//! * **epoch scoping** — `progress`/`playback_state`/`volume` events from an old
//!   stream epoch must never overwrite the current one;
//! * **metadata continuity** — a `now_playing`/`cover_art` event with missing
//!   optional fields keeps the previous title/cover instead of clearing it.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::dlna_player::DlnaEvent;
use crate::ui_protocol::{AppStateView, PlaybackView, RemoteControlView, SettingsView};

/// AirPlay epoch zero-position sentinel tolerance (mirrors Android).
const PAUSED_ZERO_SENTINEL_TOLERANCE_MS: u64 = 1_500;
/// How long the `exportLogs` success hint stays in `lastError`.
pub const EXPORT_HINT_TIMEOUT: Duration = Duration::from_secs(5);

const AIRPLAY_PROTOCOL: &str = "AirPlay";
const DLNA_PROTOCOL: &str = "DLNA";
const MIPLAY_PROTOCOL: &str = "小米妙播";
const MIPLAY_PLACEHOLDER_TITLE: &str = "小米妙播音频";

/// One of the three supported audio sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SourceKey {
    AirPlay,
    Dlna,
    MiPlay,
}

impl SourceKey {
    fn from_id(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "airplay" => Some(Self::AirPlay),
            "dlna" => Some(Self::Dlna),
            "xiaomi_miplay" | "miplay" | "xiaomi" => Some(Self::MiPlay),
            _ => None,
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::AirPlay => "airplay",
            Self::Dlna => "dlna",
            Self::MiPlay => "xiaomi_miplay",
        }
    }

    const fn protocol(self) -> &'static str {
        match self {
            Self::AirPlay => AIRPLAY_PROTOCOL,
            Self::Dlna => DLNA_PROTOCOL,
            Self::MiPlay => MIPLAY_PROTOCOL,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::AirPlay => 0,
            Self::Dlna => 1,
            Self::MiPlay => 2,
        }
    }
}

/// Cached per-source playback + remote-control snapshot.
#[derive(Debug, Clone, Default)]
struct SourceSnapshot {
    playback: PlaybackView,
    remote_control: RemoteControlView,
}

impl SourceSnapshot {
    fn fresh(playback: PlaybackView) -> Self {
        Self {
            playback,
            remote_control: RemoteControlView::default(),
        }
    }
}

struct Inner {
    initialized: bool,
    busy: bool,
    core_running: bool,
    receiver_ready: bool,
    receiver_port: Option<u16>,
    receiver_device_id: Option<String>,
    connected_client: Option<String>,
    settings: SettingsView,
    receiver_name: String,
    sources: [SourceSnapshot; 3],
    /// Current `session::Arbiter` lease holder (set by `AppRuntime`).
    active: Option<SourceKey>,
    /// Which source's snapshot is projected into [`AppStateView::playback`].
    foreground: Option<SourceKey>,
    /// The core's most recent sounding source (`selectedCoreMediaSource`).
    selected: Option<SourceKey>,
    /// Foreground projection, always kept in sync by [`Inner::project`].
    playback: PlaybackView,
    remote_control: RemoteControlView,
    last_error: Option<String>,
    export_hint: Option<(String, Instant)>,
}

impl Inner {
    fn new(settings: SettingsView, receiver_name: String) -> Self {
        Self {
            initialized: false,
            busy: false,
            core_running: false,
            receiver_ready: false,
            receiver_port: None,
            receiver_device_id: None,
            connected_client: None,
            settings,
            receiver_name,
            sources: std::array::from_fn(|_| SourceSnapshot::default()),
            active: None,
            foreground: None,
            selected: None,
            playback: PlaybackView::default(),
            remote_control: RemoteControlView::default(),
            last_error: None,
            export_hint: None,
        }
    }

    fn snapshot(&self, source: SourceKey) -> &SourceSnapshot {
        &self.sources[source.index()]
    }

    fn snapshot_mut(&mut self, source: SourceKey) -> &mut SourceSnapshot {
        &mut self.sources[source.index()]
    }

    /// Refresh the foreground projection from the foreground source cache.
    fn project(&mut self) {
        let Some(foreground) = self.foreground else {
            return;
        };
        let snapshot = self.sources[foreground.index()].clone();
        self.playback = snapshot.playback;
        self.remote_control = snapshot.remote_control;
    }

    /// Whether `source` should replace the currently projected source.
    ///
    /// While an arbiter lease is active only the lease holder may take over the
    /// foreground; without a lease the first source to produce media wins and
    /// later background metadata (for example stale AirPlay `now_playing`
    /// events after a DLNA takeover) stays cached only.
    fn should_promote(&self, source: SourceKey) -> bool {
        match self.active {
            Some(active) => active == source,
            None => self.foreground.is_none(),
        }
    }

    fn mark_selected(&mut self, source: SourceKey) {
        self.selected = Some(source);
    }

    /// Cache-only event for `source`; promote it when no other source owns the
    /// foreground. `selectedCoreMediaSource` is *not* touched (it only tracks
    /// sources that actually start/own playback, like Android).
    fn note_source_event(&mut self, source: SourceKey) {
        if self.should_promote(source) {
            self.foreground = Some(source);
        }
    }

    fn refresh_projection(&mut self, source: SourceKey) {
        if self.foreground == Some(source) {
            self.project();
        } else if self.should_promote(source) {
            self.foreground = Some(source);
            self.project();
        }
    }

    fn epoch_matches(&self, source: SourceKey, epoch: Option<u64>) -> bool {
        match (epoch, self.snapshot(source).playback.source_epoch) {
            (Some(event_epoch), Some(current_epoch)) => event_epoch == current_epoch,
            // A missing epoch on either side means "unknown scope": accept it.
            _ => true,
        }
    }

    fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return;
        }
        self.last_error = Some(trimmed.to_owned());
        self.export_hint = None;
    }

    fn clear_error(&mut self) {
        self.last_error = None;
        self.export_hint = None;
    }

    /// Drop the pending export hint if its 5 second window elapsed.
    fn expire_export_hint(&mut self, now: Instant) {
        if let Some((message, deadline)) = self.export_hint.clone()
            && now >= deadline
        {
            if self.last_error.as_deref() == Some(message.as_str()) {
                self.last_error = None;
            }
            self.export_hint = None;
        }
    }

    fn remove_source(&mut self, source: SourceKey) {
        self.sources[source.index()] = SourceSnapshot::default();
        if self.selected == Some(source) {
            self.selected = None;
        }
        if self.active == Some(source) {
            self.active = None;
        }
        if self.foreground == Some(source) {
            self.foreground = None;
            self.playback = PlaybackView::default();
            self.remote_control = RemoteControlView::default();
        }
    }

    fn play_state(&mut self, source: SourceKey, playing: bool, epoch: Option<u64>) {
        let snapshot = self.snapshot_mut(source);
        snapshot.playback.is_playing = playing;
        snapshot.playback.stream_active = snapshot.playback.stream_active || playing;
        if let Some(epoch) = epoch {
            snapshot.playback.source_epoch = Some(epoch);
        }
        if playing {
            self.mark_selected(source);
            if self.active.is_none() || self.active == Some(source) {
                self.foreground = Some(source);
            }
        }
        self.project();
    }

    // -- core events ------------------------------------------------------

    fn stream_started(&mut self, source: SourceKey, event: &Value) {
        let epoch = event_epoch(event);
        let quality = quality_text(source, event);
        let previous = self.snapshot(source).playback.clone();
        let resumes = has_retainable_media(&previous);
        let playback = if resumes {
            let mut playback = previous;
            playback.protocol = Some(source.protocol().to_owned());
            if quality.is_some() {
                playback.quality_text = quality;
            }
            playback.stream_active = true;
            playback.source_epoch = epoch.or(playback.source_epoch);
            playback
        } else {
            PlaybackView {
                media_kind: Some("audio".to_owned()),
                protocol: Some(source.protocol().to_owned()),
                quality_text: quality,
                is_playing: true,
                stream_active: true,
                source_epoch: epoch,
                ..PlaybackView::default()
            }
        };
        self.sources[source.index()] = SourceSnapshot::fresh(playback);
        self.mark_selected(source);
        if self.active.is_none() || self.active == Some(source) {
            self.foreground = Some(source);
        }
        self.project();
    }

    fn stream_stopped(&mut self, source: SourceKey, event: &Value) {
        if !self.epoch_matches(source, event_epoch(event)) {
            return;
        }
        let snapshot = self.snapshot_mut(source);
        snapshot.playback.stream_active = false;
        snapshot.playback.is_playing = false;
        if let Some(epoch) = event_epoch(event) {
            snapshot.playback.source_epoch.get_or_insert(epoch);
        }
        self.project();
    }

    fn source_takeover(&mut self, event: &Value) {
        let Some(source) =
            field_str(event, &["source"]).and_then(|value| SourceKey::from_id(&value))
        else {
            return;
        };
        let media_kind = field_str(event, &["media_kind", "mediaKind"])
            .filter(|kind| !kind.eq_ignore_ascii_case("unknown"));
        let epoch = event_epoch(event);
        let previous = self.snapshot(source).playback.clone();
        let continues_session = source == SourceKey::AirPlay && has_retainable_media(&previous);

        let mut playback = if source == SourceKey::AirPlay && !continues_session {
            PlaybackView {
                media_kind,
                protocol: Some(AIRPLAY_PROTOCOL.to_owned()),
                source_epoch: epoch,
                ..PlaybackView::default()
            }
        } else {
            let mut playback = previous;
            if media_kind.is_some() {
                playback.media_kind = media_kind;
            }
            playback.protocol = Some(source.protocol().to_owned());
            if epoch.is_some() {
                playback.source_epoch = epoch;
            }
            playback
        };
        if playback.protocol.is_none() {
            playback.protocol = Some(source.protocol().to_owned());
        }
        let remote_control = if continues_session || source == SourceKey::MiPlay {
            self.snapshot(source).remote_control.clone()
        } else {
            RemoteControlView::default()
        };
        self.sources[source.index()] = SourceSnapshot {
            playback,
            remote_control,
        };
        self.mark_selected(source);
        if self.active.is_none() {
            self.foreground = Some(source);
        }
        self.project();
    }

    fn now_playing(&mut self, source: SourceKey, event: &Value) {
        if !self.epoch_matches(source, event_epoch(event)) {
            return;
        }
        let title = field_str(event, &["title", "track", "name"]);
        let artist = field_str(event, &["artist", "album_artist", "albumArtist"]);
        let album = field_str(event, &["album"]);
        let duration =
            field_u64(event, &["duration_ms", "durationMs"]).filter(|duration| *duration > 0);
        let epoch = event_epoch(event);
        if title.is_none()
            && artist.is_none()
            && album.is_none()
            && duration.is_none()
            && epoch.is_none()
        {
            return;
        }

        let previous = self.snapshot(source).playback.clone();
        let track_changed = matches!(
            (&title, &previous.title),
            (Some(new_title), Some(old_title)) if new_title != old_title
        );
        let mut playback = previous;
        if track_changed {
            playback.title = title;
            playback.artist = artist;
            playback.album = album;
            playback.duration_ms = duration;
            playback.position_ms = 0;
            playback.track_change_direction = None;
        } else {
            if title.is_some() {
                playback.title = title;
            }
            if artist.is_some() {
                playback.artist = artist;
            }
            if album.is_some() {
                playback.album = album;
            }
            if duration.is_some() {
                playback.duration_ms = duration;
            }
        }
        if let Some(epoch) = epoch {
            playback.source_epoch = Some(epoch);
        }
        if let Some(title) = playback.title.clone() {
            let identity = match source {
                SourceKey::AirPlay => airplay_track_identity(&title, playback.artist.as_deref()),
                SourceKey::Dlna => {
                    dlna_track_identity(playback.media_url.as_deref().unwrap_or_default())
                }
                SourceKey::MiPlay => {
                    miplay_track_identity(&title, field_str(event, &["source"]).as_deref())
                }
            };
            if playback.track_identity.as_deref() != Some(identity.as_str()) {
                playback.track_change_direction = None;
            }
            playback.track_identity = Some(identity);
        }
        self.sources[source.index()].playback = playback;
        self.note_source_event(source);
        self.project();
    }

    fn cover_art(&mut self, source: SourceKey, event: &Value) {
        if !self.epoch_matches(source, event_epoch(event)) {
            return;
        }
        let Some(path) = field_str(event, &["path", "file", "file_path", "filePath"]) else {
            return;
        };
        let snapshot = self.snapshot_mut(source);
        snapshot.playback.cover_art = Some(path);
        if let Some(epoch) = event_epoch(event) {
            snapshot.playback.source_epoch.get_or_insert(epoch);
        }
        self.note_source_event(source);
        self.project();
    }

    fn volume(&mut self, source: Option<SourceKey>, event: &Value) {
        let epoch = event_epoch(event);
        let percent = field_u64(event, &["percent", "volume"])
            .map(|percent| percent.min(100) as u8)
            .or_else(|| field_f64(event, &["db", "decibels"]).map(decibels_to_percent));
        let Some(source) = source else {
            if let Some(percent) = percent {
                self.playback.volume_percent = Some(percent);
            }
            return;
        };
        if !self.epoch_matches(source, epoch) {
            return;
        }
        let snapshot = self.snapshot_mut(source);
        if let Some(percent) = percent {
            snapshot.playback.volume_percent = Some(percent);
        }
        if let Some(epoch) = epoch {
            snapshot.playback.source_epoch = Some(epoch);
        }
        self.refresh_projection(source);
    }

    fn progress(&mut self, event: &Value) {
        let epoch = event_epoch(event);
        let position = field_u64(event, &["position_ms", "positionMs", "elapsed_ms"])
            .or_else(|| {
                field_f64(event, &["position", "elapsed", "current"])
                    .map(|seconds| (seconds * 1_000.0).max(0.0) as u64)
            })
            .unwrap_or(0);
        let duration = field_u64(event, &["duration_ms", "durationMs", "total_ms"])
            .or_else(|| {
                field_f64(event, &["duration", "total"])
                    .map(|seconds| (seconds * 1_000.0).max(0.0) as u64)
            })
            .unwrap_or(0);

        let source = resolve_source(self, event, None);
        let Some(source) = source else {
            let preserve = !self.playback.is_playing
                && position <= 1
                && self.playback.position_ms > PAUSED_ZERO_SENTINEL_TOLERANCE_MS;
            if !preserve {
                self.playback.position_ms = position;
            }
            if duration > 0 {
                self.playback.duration_ms = Some(duration);
            }
            return;
        };
        if !self.epoch_matches(source, epoch) {
            return;
        }
        let snapshot = self.snapshot_mut(source);
        let preserve = !snapshot.playback.is_playing
            && position <= 1
            && snapshot.playback.position_ms > PAUSED_ZERO_SENTINEL_TOLERANCE_MS;
        if !preserve {
            snapshot.playback.position_ms = position;
        }
        if duration > 0 {
            snapshot.playback.duration_ms = Some(duration);
        }
        if let Some(epoch) = epoch {
            snapshot.playback.source_epoch = Some(epoch);
        }
        self.refresh_projection(source);
    }

    fn playback_state(&mut self, source: Option<SourceKey>, event: &Value) {
        let Some(source) = source else {
            return;
        };
        if !self.epoch_matches(source, event_epoch(event)) {
            return;
        }
        let playing =
            field_bool(event, &["playing", "is_playing", "isPlaying"]).unwrap_or_else(|| {
                field_str(event, &["state", "status"])
                    .map(|state| state.eq_ignore_ascii_case("playing"))
                    .unwrap_or(false)
            });
        self.play_state(source, playing, event_epoch(event));
    }

    fn remote_control_available(&mut self, source: Option<SourceKey>, event: &Value) {
        let Some(source) = source else {
            return;
        };
        if !self.epoch_matches(source, event_epoch(event)) {
            return;
        }
        let commands = field_string_array(event, "commands");
        let available = !commands.is_empty();
        let remote_control = RemoteControlView {
            available,
            commands,
            transport: field_str(event, &["transport"]),
            experimental: field_bool(event, &["experimental"]).unwrap_or(false),
        };
        self.sources[source.index()].remote_control = remote_control;
        if let Some(epoch) = event_epoch(event) {
            self.snapshot_mut(source).playback.source_epoch = Some(epoch);
        }
        self.refresh_projection(source);
    }

    fn remote_control_unavailable(&mut self, source: Option<SourceKey>, event: &Value) {
        let Some(source) = source else {
            return;
        };
        self.sources[source.index()].remote_control = RemoteControlView::default();
        if let Some(epoch) = event_epoch(event) {
            self.snapshot_mut(source).playback.source_epoch = Some(epoch);
        }
        if self.foreground == Some(source) {
            self.project();
        }
    }

    fn dlna_media(&mut self, event: &Value) {
        let Some(url) = field_str(event, &["url"]) else {
            return;
        };
        let epoch = event_epoch(event);
        let previous = self.snapshot(SourceKey::Dlna).playback.clone();
        let same_media = previous.media_url.as_deref() == Some(url.as_str())
            && (previous.source_epoch.is_none()
                || epoch.is_none()
                || previous.source_epoch == epoch);
        let start_position =
            field_u64(event, &["start_position_ms", "startPositionMs"]).unwrap_or(0);
        let preserve_position = same_media
            && start_position <= 1
            && previous.position_ms > PAUSED_ZERO_SENTINEL_TOLERANCE_MS;
        let duration = field_u64(event, &["duration_ms", "durationMs"]).filter(|value| *value > 0);
        let artwork = field_str(event, &["artwork_url", "artworkUrl", "album_art_url"]);

        let playback = PlaybackView {
            title: field_str(event, &["title"])
                .or_else(|| previous.title.clone().filter(|_| same_media)),
            artist: field_str(event, &["artist"])
                .or_else(|| previous.artist.clone().filter(|_| same_media)),
            album: field_str(event, &["album"])
                .or_else(|| previous.album.clone().filter(|_| same_media)),
            cover_art: artwork.or_else(|| previous.cover_art.clone().filter(|_| same_media)),
            media_url: Some(url.clone()),
            media_kind: Some(
                field_str(event, &["media_kind", "mediaKind"])
                    .unwrap_or_else(|| "audio".to_owned()),
            ),
            protocol: Some(DLNA_PROTOCOL.to_owned()),
            quality_text: dlna_quality_text(event, &url)
                .or_else(|| previous.quality_text.clone().filter(|_| same_media)),
            duration_ms: duration.or_else(|| previous.duration_ms.filter(|_| same_media)),
            position_ms: if preserve_position {
                previous.position_ms
            } else {
                start_position
            },
            volume_percent: previous.volume_percent.filter(|_| same_media),
            is_playing: previous.is_playing && same_media,
            stream_active: true,
            source_epoch: epoch.or_else(|| previous.source_epoch.filter(|_| same_media)),
            track_identity: Some(dlna_track_identity(&url)),
            track_change_direction: if same_media {
                previous.track_change_direction
            } else {
                None
            },
        };
        let remote_control = previous_remote_control(self, SourceKey::Dlna, same_media);
        self.sources[SourceKey::Dlna.index()] = SourceSnapshot {
            playback,
            remote_control,
        };
        self.mark_selected(SourceKey::Dlna);
        self.refresh_projection(SourceKey::Dlna);
    }

    fn dlna_seek(&mut self, event: &Value) {
        if !self.epoch_matches(SourceKey::Dlna, event_epoch(event)) {
            return;
        }
        if let Some(position) = field_u64(event, &["position_ms", "positionMs"]) {
            self.snapshot_mut(SourceKey::Dlna).playback.position_ms = position;
        }
        if self.foreground == Some(SourceKey::Dlna) {
            self.project();
        }
    }

    fn dlna_rate(&mut self, event: &Value) {
        if !self.epoch_matches(SourceKey::Dlna, event_epoch(event)) {
            return;
        }
        let rate = field_f64(event, &["rate"]).unwrap_or(0.0);
        {
            let snapshot = self.snapshot_mut(SourceKey::Dlna);
            snapshot.playback.is_playing = rate > 0.0;
            snapshot.playback.stream_active = true;
        }
        if rate > 0.0 {
            self.mark_selected(SourceKey::Dlna);
            if self.active.is_none() || self.active == Some(SourceKey::Dlna) {
                self.foreground = Some(SourceKey::Dlna);
            }
        }
        self.project();
    }

    fn dlna_stop(&mut self, event: &Value) {
        if !self.epoch_matches(SourceKey::Dlna, event_epoch(event)) {
            return;
        }
        let snapshot = self.snapshot_mut(SourceKey::Dlna);
        snapshot.playback.is_playing = false;
        snapshot.playback.stream_active = true;
        if self.foreground == Some(SourceKey::Dlna) {
            self.project();
        }
    }

    fn dlna_volume(&mut self, event: &Value) {
        if !self.epoch_matches(SourceKey::Dlna, event_epoch(event)) {
            return;
        }
        let percent = field_u64(event, &["percent", "volume"]).map(|value| value.min(100) as u8);
        let muted = field_bool(event, &["muted", "mute"]).unwrap_or(false);
        let snapshot = self.snapshot_mut(SourceKey::Dlna);
        if muted {
            snapshot.playback.volume_percent = Some(0);
        } else if let Some(percent) = percent {
            snapshot.playback.volume_percent = Some(percent);
        }
        if let Some(epoch) = event_epoch(event) {
            snapshot.playback.source_epoch = Some(epoch);
        }
        self.refresh_projection(SourceKey::Dlna);
    }

    // -- DLNA player events (audio-dlna T1) ------------------------------

    fn dlna_player_progress(&mut self, epoch: u64, position_ms: u64, duration_ms: u64) {
        if !self.epoch_matches(SourceKey::Dlna, Some(epoch)) {
            return;
        }
        let snapshot = self.snapshot_mut(SourceKey::Dlna);
        let preserve = !snapshot.playback.is_playing
            && position_ms <= 1
            && snapshot.playback.position_ms > PAUSED_ZERO_SENTINEL_TOLERANCE_MS;
        if !preserve {
            snapshot.playback.position_ms = position_ms;
        }
        if duration_ms > 0 {
            snapshot.playback.duration_ms = Some(duration_ms);
        }
        if snapshot.playback.source_epoch.is_none() {
            snapshot.playback.source_epoch = Some(epoch);
        }
        if self.foreground == Some(SourceKey::Dlna) {
            self.project();
        }
    }

    fn dlna_player_playback(&mut self, epoch: u64, playing: bool) {
        if !self.epoch_matches(SourceKey::Dlna, Some(epoch)) {
            return;
        }
        {
            let snapshot = self.snapshot_mut(SourceKey::Dlna);
            snapshot.playback.is_playing = playing;
            snapshot.playback.stream_active = snapshot.playback.stream_active || playing;
            if snapshot.playback.source_epoch.is_none() {
                snapshot.playback.source_epoch = Some(epoch);
            }
        }
        if playing {
            self.mark_selected(SourceKey::Dlna);
            if self.active.is_none() || self.active == Some(SourceKey::Dlna) {
                self.foreground = Some(SourceKey::Dlna);
            }
        }
        self.project();
    }

    fn dlna_player_stopped(&mut self, epoch: u64) {
        if !self.epoch_matches(SourceKey::Dlna, Some(epoch)) {
            return;
        }
        let snapshot = self.snapshot_mut(SourceKey::Dlna);
        snapshot.playback.is_playing = false;
        snapshot.playback.stream_active = true;
        if self.foreground == Some(SourceKey::Dlna) {
            self.project();
        }
    }

    // -- MiPlay events ----------------------------------------------------

    fn miplay_stream_active(&mut self) {
        let snapshot = self.snapshot_mut(SourceKey::MiPlay);
        snapshot.playback.protocol = Some(MIPLAY_PROTOCOL.to_owned());
        snapshot.playback.stream_active = true;
        if snapshot.playback.title.is_none() {
            snapshot.playback.title = Some(MIPLAY_PLACEHOLDER_TITLE.to_owned());
        }
        self.mark_selected(SourceKey::MiPlay);
        if self.active.is_none() || self.active == Some(SourceKey::MiPlay) {
            self.foreground = Some(SourceKey::MiPlay);
        }
        self.project();
    }

    fn miplay_media_info(&mut self, event: &Value) {
        let title = field_str(event, &["title"]);
        let artist = field_str(event, &["artist"]);
        let album = field_str(event, &["album"]);
        let artwork = field_str(
            event,
            &["artwork", "artwork_url", "artworkUrl", "cover_url", "cover"],
        );
        let duration =
            field_u64(event, &["duration_ms", "durationMs"]).filter(|duration| *duration > 0);
        let position = field_u64(event, &["position_ms", "positionMs"]);
        let track_id = field_str(event, &["track_id", "trackId"]);
        let source_name = field_str(event, &["source"]);

        {
            let snapshot = self.snapshot_mut(SourceKey::MiPlay);
            snapshot.playback.protocol = Some(MIPLAY_PROTOCOL.to_owned());
            let track_changed = title
                .as_deref()
                .is_some_and(|new_title| snapshot.playback.title.as_deref() != Some(new_title));
            if title.is_some() {
                snapshot.playback.title = title;
            }
            if artist.is_some() {
                snapshot.playback.artist = artist;
            }
            if album.is_some() {
                snapshot.playback.album = album;
            }
            if artwork.is_some() {
                snapshot.playback.cover_art = artwork;
            }
            if duration.is_some() {
                snapshot.playback.duration_ms = duration;
            }
            if track_changed {
                snapshot.playback.position_ms = position.unwrap_or(0);
            } else if let Some(position) = position {
                snapshot.playback.position_ms = position;
            }
            if let Some(title) = snapshot.playback.title.clone() {
                let identity =
                    miplay_track_identity(&title, source_name.as_deref().or(track_id.as_deref()));
                if snapshot.playback.track_identity.as_deref() != Some(identity.as_str()) {
                    snapshot.playback.track_change_direction = None;
                }
                snapshot.playback.track_identity = Some(identity);
            }
        }
        self.note_source_event(SourceKey::MiPlay);
        self.project();
    }

    fn miplay_audio_format(&mut self, event: &Value) {
        let quality = miplay_quality_text(event);
        let snapshot = self.snapshot_mut(SourceKey::MiPlay);
        snapshot.playback.protocol = Some(MIPLAY_PROTOCOL.to_owned());
        if quality.is_some() {
            snapshot.playback.quality_text = quality;
        }
        if self.foreground == Some(SourceKey::MiPlay) {
            self.project();
        }
    }

    fn miplay_playback_state(&mut self, event: &Value) {
        let playing = field_bool(event, &["playing", "is_playing", "isPlaying"]).unwrap_or(false);
        self.play_state(SourceKey::MiPlay, playing, None);
    }

    fn miplay_volume(&mut self, event: &Value) {
        let Some(percent) = field_u64(event, &["percent", "volume"]) else {
            return;
        };
        let snapshot = self.snapshot_mut(SourceKey::MiPlay);
        snapshot.playback.volume_percent = Some(percent.min(100) as u8);
        if self.foreground == Some(SourceKey::MiPlay) {
            self.project();
        }
    }

    fn miplay_stream_started(&mut self, event: &Value) {
        let quality = if field_str(event, &["codec"]).is_some() {
            miplay_quality_text(event)
        } else {
            None
        };
        let snapshot = self.snapshot_mut(SourceKey::MiPlay);
        snapshot.playback.protocol = Some(MIPLAY_PROTOCOL.to_owned());
        snapshot.playback.stream_active = true;
        snapshot.playback.is_playing = true;
        if quality.is_some() {
            snapshot.playback.quality_text = quality;
        }
        self.mark_selected(SourceKey::MiPlay);
        if self.active.is_none() || self.active == Some(SourceKey::MiPlay) {
            self.foreground = Some(SourceKey::MiPlay);
        }
        self.project();
    }

    fn miplay_stream_stopped(&mut self) {
        let snapshot = self.snapshot_mut(SourceKey::MiPlay);
        snapshot.playback.stream_active = false;
        snapshot.playback.is_playing = false;
        if self.foreground == Some(SourceKey::MiPlay) {
            self.project();
        }
    }
}

fn previous_remote_control(inner: &Inner, source: SourceKey, keep: bool) -> RemoteControlView {
    if keep {
        inner.snapshot(source).remote_control.clone()
    } else {
        RemoteControlView::default()
    }
}

/// Thread-safe runtime state shared by all protocol/audio threads.
#[derive(Clone)]
pub struct RuntimeState {
    inner: Arc<Mutex<Inner>>,
}

impl RuntimeState {
    pub fn new(settings: SettingsView, receiver_name: String) -> Self {
        let settings = crate::settings::validate(settings);
        let receiver_name = normalise_receiver_name(&receiver_name)
            .or_else(|| settings.receiver_name.clone())
            .unwrap_or_else(|| "FusionPlay".to_owned());
        Self {
            inner: Arc::new(Mutex::new(Inner::new(settings, receiver_name))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            // A panic in a protocol thread must not brick the UI projection.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Snapshot of the whole UI state. Also expires the 5 second export hint.
    pub fn view(&self) -> AppStateView {
        let mut inner = self.lock();
        inner.expire_export_hint(Instant::now());
        AppStateView {
            initialized: inner.initialized,
            busy: inner.busy,
            core_running: inner.core_running,
            receiver_ready: inner.receiver_ready,
            receiver_port: inner.receiver_port,
            receiver_device_id: inner.receiver_device_id.clone(),
            connected_client: inner.connected_client.clone(),
            active_media_source: inner.active.map(|source| source.id().to_owned()),
            selected_core_media_source: inner.selected.map(|source| source.id().to_owned()),
            receiver_name: inner.receiver_name.clone(),
            settings: inner.settings.clone(),
            playback: inner.playback.clone(),
            remote_control: inner.remote_control.clone(),
            last_error: inner.last_error.clone(),
        }
    }

    pub fn settings(&self) -> SettingsView {
        self.lock().settings.clone()
    }

    pub fn apply_settings(&self, settings: SettingsView) {
        let settings = crate::settings::validate(settings);
        let mut inner = self.lock();
        if let Some(name) = settings.receiver_name.clone() {
            inner.receiver_name = name;
        }
        inner.settings = settings;
    }

    pub fn set_receiver_name(&self, name: String) {
        if let Some(name) = normalise_receiver_name(&name) {
            self.lock().receiver_name = name;
        }
    }

    pub fn set_busy(&self, busy: bool) {
        self.lock().busy = busy;
    }

    pub fn set_core_running(&self, running: bool) {
        let mut inner = self.lock();
        inner.core_running = running;
        if !running {
            // Mirrors Android: a stopped core is no longer receiver-ready.
            inner.receiver_ready = false;
        }
    }

    pub fn set_initialized(&self, initialized: bool) {
        self.lock().initialized = initialized;
    }

    /// Switch the projected foreground source to the current arbiter lease.
    pub fn set_active_source(&self, source: Option<&str>) {
        let mut inner = self.lock();
        match source {
            Some(source) => {
                let Some(source) = SourceKey::from_id(source) else {
                    tracing::warn!("忽略未知活动音源：{source}");
                    return;
                };
                inner.active = Some(source);
                inner.foreground = Some(source);
                inner.project();
            }
            None => {
                inner.active = None;
                // Nothing is producing audio any more: keep the last metadata
                // visible but show the transport as paused.
                if let Some(foreground) = inner.foreground {
                    inner.snapshot_mut(foreground).playback.is_playing = false;
                }
                inner.project();
            }
        }
    }

    /// Ingest one NDJSON event emitted by the Rust core (`type` field).
    pub fn ingest_core_event(&self, event: &Value) {
        runtime_state_ingest_core_event(self, event);
    }

    /// Ingest one event emitted by the MiPlay SDK (`event` field).
    pub fn ingest_miplay_event(&self, event: &Value) {
        runtime_state_ingest_miplay_event(self, event);
    }

    /// Ingest one event emitted by the DLNA player (`audio-dlna` T1).
    pub fn ingest_dlna_event(&self, event: DlnaEvent) {
        let mut inner = self.lock();
        match event {
            DlnaEvent::Progress {
                epoch,
                position_ms,
                duration_ms,
            } => inner.dlna_player_progress(epoch, position_ms, duration_ms),
            DlnaEvent::Playback { epoch, playing } => inner.dlna_player_playback(epoch, playing),
            DlnaEvent::Error { message } => inner.set_error(message),
            DlnaEvent::Stopped { epoch } => inner.dlna_player_stopped(epoch),
        }
    }

    pub fn set_last_error(&self, message: Option<String>) {
        let mut inner = self.lock();
        match message {
            Some(message) => inner.set_error(message),
            None => inner.clear_error(),
        }
    }

    /// Show a transient success hint (for example the diagnostics path) that
    /// [`RuntimeState::view`] clears after [`EXPORT_HINT_TIMEOUT`].
    pub fn set_export_hint(&self, message: String) {
        self.set_export_hint_with_timeout(message, EXPORT_HINT_TIMEOUT);
    }

    fn set_export_hint_with_timeout(&self, message: String, timeout: Duration) {
        let mut inner = self.lock();
        let trimmed = message.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        inner.last_error = Some(trimmed.clone());
        inner.export_hint = Some((trimmed, Instant::now() + timeout));
    }

    pub fn clear_error(&self) {
        self.lock().clear_error();
    }

    /// Clear remote-control capabilities for every source.
    pub fn reset_remote_control(&self) {
        let mut inner = self.lock();
        for source in [SourceKey::AirPlay, SourceKey::Dlna, SourceKey::MiPlay] {
            inner.sources[source.index()].remote_control = RemoteControlView::default();
        }
        inner.remote_control = RemoteControlView::default();
    }

    /// Clear every cached media snapshot (protocol restart / client gone).
    pub fn reset_playback(&self) {
        let mut inner = self.lock();
        for source in [SourceKey::AirPlay, SourceKey::Dlna, SourceKey::MiPlay] {
            inner.sources[source.index()] = SourceSnapshot::default();
        }
        inner.active = None;
        inner.foreground = None;
        inner.selected = None;
        inner.playback = PlaybackView::default();
        inner.remote_control = RemoteControlView::default();
    }

    /// Mark the direction of a previous/next track command.
    ///
    /// The direction stays visible until the new track metadata arrives (and
    /// changes `trackIdentity`), mirroring the Android artwork animation.
    pub fn note_track_change(&self, direction: &str) {
        let direction = direction.trim().to_ascii_lowercase();
        if direction != "previous" && direction != "next" {
            return;
        }
        let mut inner = self.lock();
        match inner.foreground {
            Some(foreground) => {
                inner
                    .snapshot_mut(foreground)
                    .playback
                    .track_change_direction = Some(direction);
                inner.project();
            }
            None => {
                inner.playback.track_change_direction = Some(direction);
            }
        }
    }
}

fn runtime_state_ingest_core_event(state: &RuntimeState, event: &Value) {
    let Some(kind) = field_str(event, &["type", "event"]) else {
        return;
    };
    let mut inner = state.lock();
    match kind.to_ascii_lowercase().as_str() {
        "status" => {
            if field_str(event, &["state"]).is_some_and(|value| value.eq_ignore_ascii_case("error"))
                && let Some(message) = field_str(event, &["message", "detail"])
            {
                inner.set_error(message);
            }
        }
        // A core without AirPlay can still report a generic `ready`.
        "receiver_ready" | "ready" => {
            inner.core_running = true;
            inner.receiver_ready = true;
            if let Some(port) =
                field_u64(event, &["port"]).and_then(|port| u16::try_from(port).ok())
                && port > 0
            {
                inner.receiver_port = Some(port);
            }
            if let Some(device_id) = field_str(event, &["device_id", "deviceId"]) {
                inner.receiver_device_id = Some(device_id);
            }
        }
        "dlna_ready" => {
            inner.receiver_ready = true;
        }
        "output_device" => {
            // The Windows UI has no output-device picker in 1.3.0; the audio
            // engine status is exposed through logs instead.
        }
        "client_connected" => {
            if let Some(address) = field_str(event, &["address", "client"]) {
                inner.connected_client = Some(address);
            }
        }
        "client_disconnected" => {
            let disconnected = field_str(event, &["address", "client"]);
            let connects_to_current = match (&disconnected, &inner.connected_client) {
                (Some(disconnected), Some(connected)) => {
                    disconnected.eq_ignore_ascii_case(connected)
                }
                (None, _) => true,
                (Some(_), None) => true,
            };
            if connects_to_current {
                inner.connected_client = None;
                inner.remove_source(SourceKey::AirPlay);
            }
        }
        "stream_started" => {
            let source = event_source(event).unwrap_or(SourceKey::AirPlay);
            inner.stream_started(source, event);
        }
        "stream_stopped" | "stopped" => {
            let source = event_source(event).unwrap_or(SourceKey::AirPlay);
            inner.stream_stopped(source, event);
        }
        "source_takeover" => inner.source_takeover(event),
        "now_playing" | "metadata" => {
            let source = resolve_source(&inner, event, Some(SourceKey::AirPlay))
                .unwrap_or(SourceKey::AirPlay);
            inner.now_playing(source, event);
        }
        "cover_art" => {
            let source = resolve_source(&inner, event, Some(SourceKey::AirPlay))
                .unwrap_or(SourceKey::AirPlay);
            inner.cover_art(source, event);
        }
        "volume" => {
            let source = resolve_source(&inner, event, None);
            inner.volume(source, event);
        }
        "progress" => inner.progress(event),
        "playback_state" => {
            let source = resolve_source(&inner, event, None);
            inner.playback_state(source, event);
        }
        "remote_control_available" => {
            let source = resolve_source(&inner, event, None);
            inner.remote_control_available(source, event);
        }
        "remote_control_unavailable" => {
            let source = resolve_source(&inner, event, None);
            inner.remote_control_unavailable(source, event);
        }
        "command_result" => {
            if field_bool(event, &["ok", "success"]) == Some(false) {
                let message = field_str(event, &["message", "error"])
                    .unwrap_or_else(|| "播放命令失败".to_owned());
                inner.set_error(message);
            }
        }
        "error" => {
            let message = field_str(event, &["message", "error", "detail"])
                .unwrap_or_else(|| "核心报告未知错误".to_owned());
            inner.set_error(message);
        }
        "log" => {}
        "dlna_media" => inner.dlna_media(event),
        "dlna_seek" => inner.dlna_seek(event),
        "dlna_rate" => inner.dlna_rate(event),
        "dlna_stop" => inner.dlna_stop(event),
        "dlna_volume" => inner.dlna_volume(event),
        // Windows 1.3.0 does not cast video (see architecture §9).
        kind if kind.starts_with("video_") => {}
        _ => {}
    }
}

fn runtime_state_ingest_miplay_event(state: &RuntimeState, event: &Value) {
    let Some(name) = field_str(event, &["event"]) else {
        return;
    };
    let mut inner = state.lock();
    match name.to_ascii_lowercase().as_str() {
        // Protocol readiness only; never overwrite AirPlay/DLNA state.
        "receiver_ready" => {}
        "interconnect_stage" => {
            let stage = field_str(event, &["stage"]).unwrap_or_default();
            if stage.eq_ignore_ascii_case("media_session_established") {
                inner.miplay_stream_active();
            }
        }
        "media_session_established" => inner.miplay_stream_active(),
        "media_info" => inner.miplay_media_info(event),
        "audio_format" => inner.miplay_audio_format(event),
        "playback_state" => inner.miplay_playback_state(event),
        "volume" => inner.miplay_volume(event),
        "stream_started" => inner.miplay_stream_started(event),
        "stream_stopped" => inner.miplay_stream_stopped(),
        "error" | "warning" => {
            let message = field_str(event, &["message", "error", "detail"])
                .unwrap_or_else(|| "小米妙播报告错误".to_owned());
            inner.set_error(message);
        }
        "status" => {
            if field_str(event, &["state"]).is_some_and(|value| value.eq_ignore_ascii_case("error"))
                && let Some(message) = field_str(event, &["message", "detail"])
            {
                inner.set_error(message);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Event helpers
// ---------------------------------------------------------------------------

fn event_source(event: &Value) -> Option<SourceKey> {
    field_str(event, &["source", "media_source", "mediaSource"])
        .and_then(|value| SourceKey::from_id(&value))
}

fn resolve_source(inner: &Inner, event: &Value, default: Option<SourceKey>) -> Option<SourceKey> {
    event_source(event)
        .or(inner.selected)
        .or(inner.active)
        .or_else(|| {
            // Last resort: derive the source from the projected protocol.
            inner.playback.protocol.as_deref().and_then(protocol_source)
        })
        .or(default)
}

fn protocol_source(protocol: &str) -> Option<SourceKey> {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "airplay" => Some(SourceKey::AirPlay),
        "dlna" => Some(SourceKey::Dlna),
        "小米妙播" | "xiaomi" | "miplay" => Some(SourceKey::MiPlay),
        _ => None,
    }
}

fn event_epoch(event: &Value) -> Option<u64> {
    field_u64(event, &["epoch", "source_epoch", "sourceEpoch"])
}

fn field_str(event: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = event.get(*key) else {
            continue;
        };
        let Value::String(text) = value else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn field_u64(event: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        let Some(value) = event.get(*key) else {
            continue;
        };
        if let Some(number) = value.as_u64() {
            return Some(number);
        }
        if let Some(number) = value.as_f64()
            && number >= 0.0
        {
            return Some(number as u64);
        }
        if let Some(text) = value.as_str() {
            let trimmed = text.trim();
            if let Ok(number) = trimmed.parse::<u64>() {
                return Some(number);
            }
            if let Ok(number) = trimmed.parse::<f64>()
                && number >= 0.0
            {
                return Some(number as u64);
            }
        }
    }
    None
}

fn field_f64(event: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(value) = event.get(*key) else {
            continue;
        };
        if let Some(number) = value.as_f64() {
            return Some(number);
        }
        if let Some(text) = value.as_str()
            && let Ok(number) = text.trim().parse::<f64>()
        {
            return Some(number);
        }
    }
    None
}

fn field_bool(event: &Value, keys: &[&str]) -> Option<bool> {
    for key in keys {
        let Some(value) = event.get(*key) else {
            continue;
        };
        if let Some(flag) = value.as_bool() {
            return Some(flag);
        }
        if let Some(text) = value.as_str() {
            match text.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => return Some(true),
                "false" | "0" | "no" => return Some(false),
                _ => {}
            }
        }
        if let Some(number) = value.as_i64() {
            return Some(number != 0);
        }
    }
    None
}

fn field_string_array(event: &Value, key: &str) -> Vec<String> {
    event
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Quality / identity helpers (mirror Android `AppStateTransitions`)
// ---------------------------------------------------------------------------

fn quality_text(source: SourceKey, event: &Value) -> Option<String> {
    match source {
        SourceKey::AirPlay => airplay_quality_text(event),
        SourceKey::Dlna => {
            let url = field_str(event, &["url"]).unwrap_or_default();
            dlna_quality_text(event, &url)
        }
        SourceKey::MiPlay => miplay_quality_text(event),
    }
}

fn airplay_quality_text(event: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(codec) = field_str(event, &["source_codec", "sourceCodec"]) {
        parts.push(codec.to_ascii_uppercase());
    }
    // Never substitute `decoded_bits`: AAC commonly decodes to 32-bit float,
    // which is not its source depth.
    push_bits(&mut parts, field_u64(event, &["source_bits", "sourceBits"]));
    push_sample_rate(
        &mut parts,
        field_u64(event, &["source_sample_rate", "sourceSampleRate"]),
    );
    push_channels(
        &mut parts,
        field_u64(event, &["source_channels", "sourceChannels"]),
    );
    join_quality(parts)
}

fn dlna_quality_text(event: &Value, url: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(codec) = dlna_codec(
        field_str(event, &["content_type", "contentType", "mime_type"]).as_deref(),
        url,
    ) {
        parts.push(codec);
    }
    push_bits(
        &mut parts,
        field_u64(event, &["bits_per_sample", "bitsPerSample"]),
    );
    push_sample_rate(&mut parts, field_u64(event, &["sample_rate", "sampleRate"]));
    push_channels(&mut parts, field_u64(event, &["channels"]));
    if let Some(bitrate) =
        field_u64(event, &["bitrate_bps", "bitrateBps"]).filter(|value| *value > 0)
    {
        parts.push(format_bitrate(bitrate));
    }
    join_quality(parts)
}

fn miplay_quality_text(event: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(codec) = field_str(
        event,
        &[
            "codec",
            "audio_format",
            "audioFormat",
            "source_codec",
            "sourceCodec",
        ],
    ) {
        let codec = codec
            .trim()
            .trim_start_matches("audio/")
            .to_ascii_uppercase();
        if !codec.is_empty() {
            parts.push(codec);
        }
    }
    push_bits(
        &mut parts,
        field_u64(event, &["bits_per_sample", "bitsPerSample", "source_bits"]),
    );
    push_sample_rate(
        &mut parts,
        field_u64(event, &["sample_rate", "sampleRate", "source_sample_rate"]),
    );
    push_channels(
        &mut parts,
        field_u64(event, &["channels", "source_channels"]),
    );
    if let Some(bitrate) =
        field_u64(event, &["bitrate", "bitrate_bps", "bitrateBps"]).filter(|value| *value > 0)
    {
        parts.push(format_bitrate(bitrate));
    }
    join_quality(parts)
}

fn push_bits(parts: &mut Vec<String>, bits: Option<u64>) {
    if let Some(bits) = bits
        && bits > 0
    {
        parts.push(format!("{bits}-bit"));
    }
}

fn push_sample_rate(parts: &mut Vec<String>, rate: Option<u64>) {
    if let Some(rate) = rate
        && rate > 0
    {
        parts.push(format_sample_rate(rate));
    }
}

fn push_channels(parts: &mut Vec<String>, channels: Option<u64>) {
    if let Some(channels) = channels
        && channels > 0
    {
        parts.push(match channels {
            1 => "单声道".to_owned(),
            2 => "立体声".to_owned(),
            other => format!("{other} 声道"),
        });
    }
}

fn join_quality(parts: Vec<String>) -> Option<String> {
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn format_sample_rate(rate: u64) -> String {
    if rate.is_multiple_of(1_000) {
        format!("{} kHz", rate / 1_000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1_000.0)
    }
}

fn format_bitrate(bitrate_bps: u64) -> String {
    if bitrate_bps >= 1_000_000 {
        format!("{:.2} Mbps", bitrate_bps as f64 / 1_000_000.0)
    } else if bitrate_bps >= 1_000 {
        format!("{:.0} kbps", bitrate_bps as f64 / 1_000.0)
    } else {
        format!("{bitrate_bps} bps")
    }
}

fn dlna_codec(content_type: Option<&str>, url: &str) -> Option<String> {
    let mime = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty());
    let extension = url
        .split('?')
        .next()
        .unwrap_or_default()
        .split('#')
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    let matches = |mime_values: &[&str], extensions: &[&str]| {
        mime.as_deref()
            .is_some_and(|value| mime_values.contains(&value))
            || extensions.contains(&extension.as_str())
    };
    if matches(&["audio/flac", "audio/x-flac"], &["flac"]) {
        return Some("FLAC".to_owned());
    }
    if matches(&["audio/mpeg"], &["mp3"]) {
        return Some("MP3".to_owned());
    }
    if matches(&["audio/aac", "audio/aacp"], &["aac"]) {
        return Some("AAC".to_owned());
    }
    if matches(&["audio/alac", "audio/x-alac"], &["alac"]) {
        return Some("ALAC".to_owned());
    }
    if matches(&["audio/wav", "audio/x-wav"], &["wav"]) {
        return Some("WAV".to_owned());
    }
    if matches(&["audio/ogg"], &["ogg"]) {
        return Some("OGG".to_owned());
    }
    if matches(&["audio/opus"], &["opus"]) {
        return Some("OPUS".to_owned());
    }
    if extension == "m4a" {
        return Some("M4A".to_owned());
    }
    if mime.as_deref() == Some("audio/mpeg") {
        return Some("MPEG Audio".to_owned());
    }
    if let Some(mime) = mime {
        let subtype = mime.split('/').nth(1).unwrap_or_default();
        if !subtype.is_empty() {
            return Some(subtype.to_ascii_uppercase());
        }
        return None;
    }
    (!extension.is_empty()).then(|| extension.to_ascii_uppercase())
}

fn airplay_track_identity(title: &str, artist: Option<&str>) -> String {
    format!("airplay:{title}|{}", artist.unwrap_or_default())
}

fn dlna_track_identity(url: &str) -> String {
    format!("dlna:{url}")
}

fn miplay_track_identity(title: &str, source: Option<&str>) -> String {
    format!("miplay:{title}|{}", source.unwrap_or_default())
}

fn decibels_to_percent(decibels: f64) -> u8 {
    if !decibels.is_finite() {
        return 0;
    }
    let linear = 10f64.powf(decibels / 20.0);
    (linear * 100.0).clamp(0.0, 100.0).round() as u8
}

fn has_retainable_media(playback: &PlaybackView) -> bool {
    playback.stream_active
        || playback.protocol.is_some()
        || playback.source_epoch.is_some()
        || playback.title.is_some()
        || playback.artist.is_some()
        || playback.album.is_some()
        || playback.cover_art.is_some()
        || playback.duration_ms.is_some()
        || playback.position_ms > 0
        || playback.track_identity.is_some()
}

fn normalise_receiver_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(63).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> RuntimeState {
        RuntimeState::new(SettingsView::default(), "客厅电脑".to_owned())
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn runtime_state_is_send_and_sync() {
        assert_send_sync::<RuntimeState>();
    }

    #[test]
    fn receiver_ready_populates_receiver_fields() {
        let state = state();
        state.set_initialized(true);
        state.ingest_core_event(&json!({
            "type": "receiver_ready",
            "port": 7000,
            "device_id": "02:11:22:33:44:55",
        }));
        let view = state.view();
        assert!(view.initialized);
        assert!(view.core_running);
        assert!(view.receiver_ready);
        assert_eq!(view.receiver_port, Some(7000));
        assert_eq!(
            view.receiver_device_id.as_deref(),
            Some("02:11:22:33:44:55")
        );
        assert_eq!(view.receiver_name, "客厅电脑");
    }

    #[test]
    fn stream_started_builds_airplay_quality_and_playback() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "stream_started",
            "source": "airplay",
            "epoch": 1,
            "source_codec": "aac",
            "source_sample_rate": 44100,
            "source_bits": 16,
            "source_channels": 2,
        }));
        let view = state.view();
        assert_eq!(view.playback.protocol.as_deref(), Some("AirPlay"));
        assert_eq!(
            view.playback.quality_text.as_deref(),
            Some("AAC · 16-bit · 44.1 kHz · 立体声")
        );
        assert!(view.playback.stream_active);
        assert!(view.playback.is_playing);
        assert_eq!(view.playback.source_epoch, Some(1));
        assert_eq!(view.selected_core_media_source.as_deref(), Some("airplay"));
        assert_eq!(view.active_media_source, None);
    }

    #[test]
    fn stream_started_without_source_bits_does_not_invent_depth() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "stream_started",
            "source_codec": "aac",
            "source_sample_rate": 48000,
            "decoded_bits": 32,
        }));
        assert_eq!(
            state.view().playback.quality_text.as_deref(),
            Some("AAC · 48 kHz")
        );
    }

    #[test]
    fn old_epoch_progress_and_playback_do_not_overwrite_new_epoch() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 2}));
        state.ingest_core_event(&json!({
            "type": "progress", "source": "airplay", "epoch": 2,
            "position_ms": 1200, "duration_ms": 200_000
        }));
        assert_eq!(state.view().playback.position_ms, 1200);

        // Stale epoch events are ignored.
        state.ingest_core_event(&json!({
            "type": "progress", "source": "airplay", "epoch": 1,
            "position_ms": 9000, "duration_ms": 100_000
        }));
        state.ingest_core_event(&json!({
            "type": "volume", "source": "airplay", "epoch": 1, "percent": 10
        }));
        state.ingest_core_event(&json!({
            "type": "playback_state", "source": "airplay", "epoch": 1, "playing": false
        }));
        let view = state.view();
        assert_eq!(view.playback.position_ms, 1200);
        assert_eq!(view.playback.volume_percent, None);
        assert!(view.playback.is_playing);

        // The same epoch is accepted.
        state.ingest_core_event(&json!({
            "type": "playback_state", "source": "airplay", "epoch": 2, "playing": false
        }));
        assert!(!state.view().playback.is_playing);
    }

    #[test]
    fn paused_zero_progress_sentinel_keeps_known_position() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "progress", "source": "airplay", "epoch": 1,
            "position_ms": 42_000, "duration_ms": 200_000
        }));
        state.ingest_core_event(&json!({
            "type": "playback_state", "source": "airplay", "epoch": 1, "playing": false
        }));
        // Some senders emit position 0 while paused; keep the known position.
        state.ingest_core_event(&json!({
            "type": "progress", "source": "airplay", "epoch": 1,
            "position_ms": 0, "duration_ms": 0
        }));
        let view = state.view();
        assert_eq!(view.playback.position_ms, 42_000);
        assert_eq!(view.playback.duration_ms, Some(200_000));
        assert!(!view.playback.is_playing);
    }

    #[test]
    fn now_playing_missing_metadata_keeps_previous_values() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1,
            "title": "晴天", "artist": "周杰伦", "album": "叶惠美", "duration_ms": 269_000
        }));
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1
        }));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("晴天"));
        assert_eq!(view.playback.artist.as_deref(), Some("周杰伦"));
        assert_eq!(view.playback.album.as_deref(), Some("叶惠美"));
        assert_eq!(view.playback.duration_ms, Some(269_000));
        assert_eq!(
            view.playback.track_identity.as_deref(),
            Some("airplay:晴天|周杰伦")
        );

        state.ingest_core_event(&json!({
            "type": "cover_art", "source": "airplay", "epoch": 1,
            "path": "C:\\art\\cover.jpg"
        }));
        state.ingest_core_event(&json!({
            "type": "cover_art", "source": "airplay", "epoch": 1
        }));
        assert_eq!(
            state.view().playback.cover_art.as_deref(),
            Some("C:\\art\\cover.jpg")
        );

        // A new title resets position and refreshes the identity.
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1,
            "title": "稻香", "duration_ms": 223_000
        }));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("稻香"));
        assert_eq!(view.playback.position_ms, 0);
        assert_eq!(
            view.playback.track_identity.as_deref(),
            Some("airplay:稻香|")
        );
    }

    #[test]
    fn source_takeover_switches_protocol_and_scopes_epochs() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1,
            "title": "AirPlay Song", "artist": "A"
        }));
        state.ingest_core_event(&json!({
            "type": "source_takeover", "source": "dlna",
            "media_kind": "audio", "epoch": 5
        }));
        let view = state.view();
        assert_eq!(view.playback.protocol.as_deref(), Some("DLNA"));
        assert_eq!(view.playback.source_epoch, Some(5));
        assert_eq!(view.selected_core_media_source.as_deref(), Some("dlna"));

        // AirPlay metadata keeps arriving in the same epoch while DLNA owns
        // the foreground: it is cached but not projected.
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1,
            "title": "Stale Song", "artist": "B"
        }));
        assert_eq!(state.view().playback.title, None);

        // Activating AirPlay projects its latest cached metadata.
        state.set_active_source(Some("airplay"));
        let view = state.view();
        assert_eq!(view.active_media_source.as_deref(), Some("airplay"));
        assert_eq!(view.playback.title.as_deref(), Some("Stale Song"));
        assert_eq!(view.playback.protocol.as_deref(), Some("AirPlay"));
    }

    #[test]
    fn active_source_projection_isolates_non_active_events() {
        let state = state();
        state
            .ingest_core_event(&json!({"type": "stream_started", "source": "airplay", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "epoch": 1, "title": "AirPlay"
        }));
        // While no arbitration lease exists, background metadata of another
        // source is cached but must not steal the foreground.
        state.ingest_core_event(&json!({
            "type": "dlna_media", "url": "http://192.168.1.2/a.flac",
            "title": "DLNA", "content_type": "audio/flac", "sample_rate": 96000,
            "duration_ms": 100_000, "epoch": 9
        }));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("AirPlay"));
        assert_eq!(view.selected_core_media_source.as_deref(), Some("dlna"));

        // The arbiter claims DLNA: its cached snapshot becomes visible.
        state.set_active_source(Some("dlna"));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("DLNA"));
        assert_eq!(view.playback.quality_text.as_deref(), Some("FLAC · 96 kHz"));
        assert_eq!(view.active_media_source.as_deref(), Some("dlna"));

        // AirPlay is still cached while DLNA owns the lease.
        state.ingest_core_event(&json!({
            "type": "progress", "source": "dlna", "epoch": 9,
            "position_ms": 33_000, "duration_ms": 100_000
        }));
        state.set_active_source(Some("airplay"));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("AirPlay"));
        assert_eq!(view.playback.position_ms, 0);

        state.set_active_source(Some("dlna"));
        assert_eq!(state.view().playback.position_ms, 33_000);
        assert_eq!(
            state.view().playback.track_identity.as_deref(),
            Some("dlna:http://192.168.1.2/a.flac")
        );

        state.set_active_source(None);
        let view = state.view();
        assert_eq!(view.active_media_source, None);
        assert!(!view.playback.is_playing);
        assert_eq!(view.playback.title.as_deref(), Some("DLNA"));
    }

    #[test]
    fn dlna_media_continuity_and_player_events() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "dlna_media", "url": "http://192.168.1.2/song.mp3",
            "title": "Song", "artist": "Artist", "content_type": "audio/mpeg",
            "duration_ms": 180_000, "start_position_ms": 0, "epoch": 3
        }));
        state.ingest_core_event(&json!({
            "type": "dlna_rate", "epoch": 3, "rate": 1.0
        }));
        let view = state.view();
        assert!(view.playback.is_playing);
        assert!(view.playback.stream_active);
        assert_eq!(view.playback.protocol.as_deref(), Some("DLNA"));
        assert_eq!(view.playback.quality_text.as_deref(), Some("MP3"));

        // Same URL without metadata keeps the title/artist.
        state.ingest_core_event(&json!({
            "type": "dlna_media", "url": "http://192.168.1.2/song.mp3",
            "epoch": 3
        }));
        assert_eq!(state.view().playback.title.as_deref(), Some("Song"));

        // A new URL without metadata must not leak the old title.
        state.ingest_core_event(&json!({
            "type": "dlna_media", "url": "http://192.168.1.2/other.mp3",
            "epoch": 4
        }));
        let view = state.view();
        assert_eq!(view.playback.title, None);
        assert_eq!(
            view.playback.media_url.as_deref(),
            Some("http://192.168.1.2/other.mp3")
        );

        state.ingest_core_event(&json!({
            "type": "dlna_seek", "epoch": 4, "position_ms": 12_000
        }));
        assert_eq!(state.view().playback.position_ms, 12_000);
        state.ingest_core_event(&json!({"type": "dlna_volume", "epoch": 4, "percent": 35}));
        assert_eq!(state.view().playback.volume_percent, Some(35));
        state.ingest_core_event(&json!({"type": "dlna_stop", "epoch": 4}));
        let view = state.view();
        assert!(!view.playback.is_playing);
        assert!(view.playback.stream_active);
    }

    #[test]
    fn dlna_player_events_drive_progress_and_playback() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "dlna_media", "url": "http://host/a.wav", "epoch": 7,
            "duration_ms": 60_000
        }));
        state.ingest_dlna_event(DlnaEvent::Playback {
            epoch: 7,
            playing: true,
        });
        state.ingest_dlna_event(DlnaEvent::Progress {
            epoch: 7,
            position_ms: 5_000,
            duration_ms: 60_000,
        });
        let view = state.view();
        assert!(view.playback.is_playing);
        assert_eq!(view.playback.position_ms, 5_000);

        // Stale epochs are ignored.
        state.ingest_dlna_event(DlnaEvent::Progress {
            epoch: 6,
            position_ms: 50_000,
            duration_ms: 60_000,
        });
        assert_eq!(state.view().playback.position_ms, 5_000);

        state.ingest_dlna_event(DlnaEvent::Playback {
            epoch: 7,
            playing: false,
        });
        assert!(!state.view().playback.is_playing);
        state.ingest_dlna_event(DlnaEvent::Playback {
            epoch: 7,
            playing: true,
        });
        state.ingest_dlna_event(DlnaEvent::Error {
            message: "DLNA 拉流失败".to_owned(),
        });
        assert_eq!(state.view().last_error.as_deref(), Some("DLNA 拉流失败"));
        state.ingest_dlna_event(DlnaEvent::Stopped { epoch: 7 });
        assert!(!state.view().playback.is_playing);
    }

    #[test]
    fn miplay_events_map_to_playback_fields() {
        let state = state();
        // receiver_ready must not flip the AirPlay/DLNA readiness flag.
        state.ingest_miplay_event(&json!({
            "event": "receiver_ready", "name": "FusionPlay", "port": 9000
        }));
        assert!(!state.view().receiver_ready);

        state.ingest_miplay_event(&json!({
            "event": "interconnect_stage", "stage": "media_session_established"
        }));
        let view = state.view();
        assert!(view.playback.stream_active);
        assert_eq!(view.playback.protocol.as_deref(), Some("小米妙播"));
        assert_eq!(view.playback.title.as_deref(), Some("小米妙播音频"));

        state.ingest_miplay_event(&json!({
            "event": "media_info", "source": "xiaomi", "track_id": "t-1",
            "title": "起风了", "artist": "买辣椒也用券", "album": "起风了",
            "duration_ms": 325_000, "position_ms": 0,
            "artwork": "http://cover/1.jpg"
        }));
        let view = state.view();
        assert_eq!(view.playback.title.as_deref(), Some("起风了"));
        assert_eq!(view.playback.artist.as_deref(), Some("买辣椒也用券"));
        assert_eq!(view.playback.album.as_deref(), Some("起风了"));
        assert_eq!(
            view.playback.cover_art.as_deref(),
            Some("http://cover/1.jpg")
        );
        assert_eq!(view.playback.duration_ms, Some(325_000));
        assert_eq!(
            view.playback.track_identity.as_deref(),
            Some("miplay:起风了|xiaomi")
        );

        state.ingest_miplay_event(&json!({
            "event": "audio_format", "codec": "AAC-LC",
            "sample_rate": 44100, "bits_per_sample": 16, "channels": 2,
            "bitrate": 128000
        }));
        assert_eq!(
            state.view().playback.quality_text.as_deref(),
            Some("AAC-LC · 16-bit · 44.1 kHz · 立体声 · 128 kbps")
        );

        state.ingest_miplay_event(&json!({"event": "volume", "percent": 42}));
        assert_eq!(state.view().playback.volume_percent, Some(42));

        state.ingest_miplay_event(&json!({
            "event": "playback_state", "playing": true, "session_active": true
        }));
        assert!(state.view().playback.is_playing);
        state.ingest_miplay_event(&json!({
            "event": "playback_state", "playing": false, "session_active": true
        }));
        let view = state.view();
        assert!(!view.playback.is_playing, "pause must be kept");
        assert!(view.playback.stream_active, "paused stream stays active");

        state.ingest_miplay_event(&json!({"event": "stream_stopped"}));
        assert!(!state.view().playback.stream_active);

        state.ingest_miplay_event(&json!({
            "event": "error", "message": "小米妙播连接断开"
        }));
        assert_eq!(state.view().last_error.as_deref(), Some("小米妙播连接断开"));
    }

    #[test]
    fn miplay_track_change_direction_is_cleared_by_new_metadata() {
        let state = state();
        state.ingest_miplay_event(&json!({
            "event": "media_info", "source": "xiaomi", "title": "第一首"
        }));
        state.note_track_change("next");
        assert_eq!(
            state.view().playback.track_change_direction.as_deref(),
            Some("next")
        );
        state.ingest_miplay_event(&json!({
            "event": "media_info", "source": "xiaomi", "title": "第二首"
        }));
        let view = state.view();
        assert_eq!(view.playback.track_change_direction, None);
        assert_eq!(
            view.playback.track_identity.as_deref(),
            Some("miplay:第二首|xiaomi")
        );
    }

    #[test]
    fn miplay_stream_started_and_stopped_toggle_stream_state() {
        let state = state();
        state.ingest_miplay_event(&json!({
            "event": "stream_started", "codec": "aac",
            "sample_rate": 44100, "source_bits": 16
        }));
        let view = state.view();
        assert!(view.playback.is_playing);
        assert!(view.playback.stream_active);
        assert_eq!(
            view.playback.quality_text.as_deref(),
            Some("AAC · 16-bit · 44.1 kHz")
        );
        state.ingest_miplay_event(&json!({"event": "stream_stopped"}));
        let view = state.view();
        assert!(!view.playback.is_playing);
        assert!(!view.playback.stream_active);
    }

    #[test]
    fn remote_control_commands_are_projected() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "remote_control_available", "source": "airplay", "epoch": 1,
            "commands": ["play", "pause", "next_track", "previous_track"],
            "transport": "airplay", "experimental": false
        }));
        let view = state.view();
        assert!(view.remote_control.available);
        assert_eq!(view.remote_control.commands.len(), 4);
        assert_eq!(view.remote_control.transport.as_deref(), Some("airplay"));

        state.ingest_core_event(&json!({
            "type": "remote_control_unavailable", "source": "airplay", "epoch": 1
        }));
        let view = state.view();
        assert!(!view.remote_control.available);
        assert!(view.remote_control.commands.is_empty());
    }

    #[test]
    fn note_track_change_sets_direction_until_new_title() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "title": "第一首"
        }));
        state.note_track_change("previous");
        assert_eq!(
            state.view().playback.track_change_direction.as_deref(),
            Some("previous")
        );
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "title": "第二首"
        }));
        assert_eq!(state.view().playback.track_change_direction, None);
        // Unknown directions are ignored.
        state.note_track_change("sideways");
        assert_eq!(state.view().playback.track_change_direction, None);
    }

    #[test]
    fn command_failure_and_core_errors_set_last_error() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "command_result", "requestId": "c-1",
            "command": "playback", "ok": false, "message": "暂停失败"
        }));
        assert_eq!(state.view().last_error.as_deref(), Some("暂停失败"));

        state.clear_error();
        assert_eq!(state.view().last_error, None);
        state.ingest_core_event(&json!({"type": "error", "message": "核心错误"}));
        assert_eq!(state.view().last_error.as_deref(), Some("核心错误"));

        // Successful command results do not clear an unrelated user-visible
        // error (mirrors Android).
        state.ingest_core_event(&json!({
            "type": "command_result", "command": "seek", "ok": true
        }));
        assert_eq!(state.view().last_error.as_deref(), Some("核心错误"));
    }

    #[test]
    fn export_hint_expires_and_real_errors_survive() {
        let state = state();
        state.set_export_hint_with_timeout(
            "诊断包已导出：C:\\x.txt".to_owned(),
            Duration::from_millis(20),
        );
        assert!(state.view().last_error.is_some());
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(state.view().last_error, None);

        // A hint that is superseded by a real error must not clear it later.
        state.set_export_hint_with_timeout("提示".to_owned(), Duration::from_millis(20));
        state.set_last_error(Some("真正的错误".to_owned()));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(state.view().last_error.as_deref(), Some("真正的错误"));

        // clear_error also cancels a pending hint.
        state.set_export_hint("提示2".to_owned());
        state.clear_error();
        assert_eq!(state.view().last_error, None);
    }

    #[test]
    fn client_disconnect_only_clears_matching_client() {
        let state = state();
        state.ingest_core_event(&json!({
            "type": "client_connected", "address": "192.168.1.20"
        }));
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "title": "Song"
        }));

        state.ingest_core_event(&json!({
            "type": "client_disconnected", "address": "192.168.1.99"
        }));
        assert_eq!(
            state.view().connected_client.as_deref(),
            Some("192.168.1.20")
        );
        assert_eq!(state.view().playback.title.as_deref(), Some("Song"));

        state.ingest_core_event(&json!({
            "type": "client_disconnected", "address": "192.168.1.20"
        }));
        let view = state.view();
        assert_eq!(view.connected_client, None);
        assert_eq!(view.playback.title, None);
        assert_eq!(view.playback.protocol, None);
        assert_eq!(view.active_media_source, None);
    }

    #[test]
    fn settings_apply_and_receiver_name_projection() {
        let state = RuntimeState::new(
            SettingsView {
                receiver_name: Some("旧名字".to_owned()),
                ..SettingsView::default()
            },
            "旧名字".to_owned(),
        );
        assert_eq!(state.view().receiver_name, "旧名字");
        assert_eq!(state.settings().schema_version, 8);

        state.apply_settings(SettingsView {
            receiver_name: Some("新名字".to_owned()),
            mi_play_device_identity: "invalid".to_owned(),
            ..SettingsView::default()
        });
        let view = state.view();
        assert_eq!(view.receiver_name, "新名字");
        // Invalid identities are normalised for the projection (save() rejects
        // them, which is what the command dispatcher reports to the UI).
        assert_eq!(view.settings.mi_play_device_identity, "television");

        state.set_receiver_name("另一个名字".to_owned());
        assert_eq!(state.view().receiver_name, "另一个名字");
        state.set_busy(true);
        assert!(state.view().busy);
        state.set_core_running(false);
        assert!(!state.view().core_running);
    }

    #[test]
    fn reset_playback_and_remote_control_clear_caches() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "now_playing", "source": "airplay", "title": "Song"
        }));
        state.ingest_core_event(&json!({
            "type": "remote_control_available", "source": "airplay",
            "commands": ["play"]
        }));
        state.set_active_source(Some("airplay"));

        state.reset_remote_control();
        assert!(!state.view().remote_control.available);
        assert_eq!(state.view().playback.title.as_deref(), Some("Song"));

        state.reset_playback();
        let view = state.view();
        assert_eq!(view.playback, PlaybackView::default());
        assert_eq!(view.active_media_source, None);
        assert_eq!(view.selected_core_media_source, None);
    }

    #[test]
    fn dlna_ready_marks_receiver_ready() {
        let state = state();
        state.ingest_core_event(&json!({"type": "dlna_ready", "port": 8899}));
        assert!(state.view().receiver_ready);
    }

    #[test]
    fn video_events_are_ignored_in_1_3_0() {
        let state = state();
        state.ingest_core_event(&json!({"type": "stream_started", "epoch": 1}));
        state.ingest_core_event(&json!({
            "type": "video_play", "url": "http://host/video.mp4",
            "start_position_ms": 0, "epoch": 1
        }));
        let view = state.view();
        assert_eq!(view.playback.media_url, None);
        assert_eq!(view.playback.media_kind.as_deref(), Some("audio"));
        assert_eq!(view.playback.protocol.as_deref(), Some("AirPlay"));
    }

    #[test]
    fn unknown_and_malformed_events_are_ignored() {
        let state = state();
        state.ingest_core_event(&json!({}));
        state.ingest_core_event(&json!({"type": "future_event", "value": 1}));
        state.ingest_core_event(&json!({"type": "now_playing", "title": 42}));
        state.ingest_miplay_event(&json!({"no_event_field": true}));
        state.ingest_core_event(&serde_json::Value::Null);
        let view = state.view();
        assert_eq!(view.playback, PlaybackView::default());
        assert_eq!(view.last_error, None);
        assert_eq!(view.selected_core_media_source, None);
        assert_eq!(view.active_media_source, None);
    }
}
