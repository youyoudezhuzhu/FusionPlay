//! Shared runtime state exposed by the HTTP API and updated by protocol hosts.

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
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub detail_codec: Option<String>,
    pub lyrics_text: Option<String>,
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
        let mut state = self.state.write().unwrap();
        if source == Some(Source::MiPlay) && state.current_source.as_deref() != Some("miplay") {
            state.media = state.pending_miplay.take().unwrap_or_default();
        }
        state.current_source = source.map(|source| source.as_str().to_string());
        if source != Some(Source::AirPlay) {
            state.sender_volume.airplay_db = None;
            state.sender_volume.airplay_percent = None;
        }
        if source != Some(Source::Dlna) {
            state.sender_volume.dlna_percent = None;
        }
        if source != Some(Source::MiPlay) {
            state.sender_volume.miplay_percent = None;
        }
    }

    pub fn record_error(&self, message: impl Into<String>) {
        let message = message.into();
        let mut state = self.state.write().unwrap();
        state.last_error = Some(message.clone());
        state.errors.push(message);
        if state.errors.len() > 20 {
            state.errors.remove(0);
        }
    }
}
