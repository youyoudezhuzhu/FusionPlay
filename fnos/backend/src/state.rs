//! Shared runtime state exposed by the HTTP API and updated by protocol hosts.

use fusionplay_miplay_sdk::track::TrackCredit;
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::session::Source;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProtocolRuntime {
    pub enabled: bool,
    pub state: String,
    pub detail: String,
    pub clients: usize,
    pub port: u16,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MediaInfo {
    #[serde(skip)]
    miplay_track_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub detail_codec: Option<String>,
    pub lyrics_text: Option<String>,
}

impl MediaInfo {
    // SDK field updates may contain only lyrics or timing. Its "metadata:"
    // IDs are derived from optional display fields and can change when those
    // fields arrive in separate messages.
    pub(crate) fn update_miplay(&mut self, value: &serde_json::Value) {
        let text = |key: &str| value[key].as_str().filter(|s| !s.trim().is_empty());
        let title = text("title");
        let artist = text("artist");
        let incoming = TrackCredit { id: text("track_id"), title, artist };
        let changed = incoming.changed_from(&TrackCredit {
            id: self.miplay_track_id.as_deref(),
            title: self.title.as_deref(),
            artist: self.artist.as_deref(),
        });
        if changed {
            *self = Self { detail_codec: self.detail_codec.take(), ..Default::default() };
        }
        if let Some(id) = incoming.sender_id() { self.miplay_track_id = Some(id.to_owned()); }
        if let Some(title) = title { self.title = Some(title.to_owned()); }
        if let Some(artist) = artist { self.artist = Some(artist.to_owned()); }
        if let Some(album) = text("album") { self.album = Some(album.to_owned()); }
        if let Some(lyrics) = text("lyrics") { self.lyrics_text = Some(lyrics.to_owned()); }
        if let Some(art) = text("artwork") { self.cover_url = Some(art.to_owned()); }
        if let Some(ms) = value["duration_ms"].as_u64().filter(|ms| *ms > 0) { self.duration_ms = Some(ms); }
        if let Some(ms) = value["position_ms"].as_u64() { self.position_ms = Some(ms); }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SenderVolume {
    pub airplay_db: Option<f32>,
    pub airplay_percent: Option<u8>,
    pub dlna_percent: Option<u8>,
    pub miplay_percent: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeState {
    pub airplay: ProtocolRuntime,
    pub dlna: ProtocolRuntime,
    pub miplay: ProtocolRuntime,
    pub current_source: Option<String>,
    #[serde(skip)]
    pub commands: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(skip)]
    pub pending_miplay: Option<MediaInfo>,
    pub media: MediaInfo,
    pub sender_volume: SenderVolume,
    pub last_error: Option<String>,
    pub errors: Vec<String>,
}

impl RuntimeState {
    pub(crate) fn select_source(&mut self, source: Option<Source>) {
        if let Some(next) = source {
            if self.current_source.as_deref() != Some(next.as_str()) {
                self.media = if next == Source::MiPlay {
                    self.pending_miplay.take().unwrap_or_default()
                } else {
                    MediaInfo::default()
                };
            }
        }
        self.current_source = source.map(|source| source.as_str().to_string());
        if source != Some(Source::AirPlay) {
            self.sender_volume.airplay_db = None;
            self.sender_volume.airplay_percent = None;
        }
        if source != Some(Source::Dlna) {
            self.sender_volume.dlna_percent = None;
        }
        if source != Some(Source::MiPlay) {
            self.sender_volume.miplay_percent = None;
        }
    }

    pub fn player_commands(&self) -> Vec<String> {
        match self.current_source.as_deref() {
            Some("miplay") => ["play_pause", "previous_track", "next_track", "seek", "set_volume"]
                .into_iter().map(String::from).collect(),
            Some(source) => self.commands.get(source).cloned().unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

pub struct AppState {
    pub started: Instant,
    pub state: RwLock<RuntimeState>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new() -> SharedState {
        Arc::new(Self { started: Instant::now(), state: RwLock::new(RuntimeState::default()) })
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn snapshot(&self) -> RuntimeState {
        self.state.read().unwrap().clone()
    }

    pub fn set_current_source(&self, source: Option<Source>) {
        self.state.write().unwrap().select_source(source);
    }

    pub fn clear_source_if_current(&self, source: Source) {
        let mut state = self.state.write().unwrap();
        if state.current_source.as_deref() == Some(source.as_str()) {
            state.current_source = None;
            state.sender_volume = SenderVolume::default();
        }
    }

    pub fn record_error(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::error!(%message, "接收器错误");
        let mut state = self.state.write().unwrap();
        state.last_error = Some(message.clone());
        state.errors.push(message);
        if state.errors.len() > 20 {
            state.errors.remove(0);
        }
    }
}
