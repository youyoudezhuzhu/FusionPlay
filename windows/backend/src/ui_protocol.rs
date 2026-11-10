//! NDJSON wire types shared by the sidecar and the Flutter UI.
//!
//! The JSON field names must match `flutter/lib/models/app_state.dart`
//! (`AppState.fromMap`) field for field, and the command names must match the
//! methods of `NativeAppController` on Android. See
//! `docs/WINDOWS_ARCHITECTURE.md` §5/§6.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// app_state
// ---------------------------------------------------------------------------

/// Mirrors Dart `AppState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateView {
    pub initialized: bool,
    pub busy: bool,
    pub core_running: bool,
    pub receiver_ready: bool,
    pub receiver_port: Option<u16>,
    pub receiver_device_id: Option<String>,
    pub connected_client: Option<String>,
    pub active_media_source: Option<String>,
    pub selected_core_media_source: Option<String>,
    pub receiver_name: String,
    pub settings: SettingsView,
    pub playback: PlaybackView,
    pub remote_control: RemoteControlView,
    pub last_error: Option<String>,
}

impl Default for AppStateView {
    fn default() -> Self {
        Self {
            initialized: false,
            busy: false,
            core_running: false,
            receiver_ready: false,
            receiver_port: None,
            receiver_device_id: None,
            connected_client: None,
            active_media_source: None,
            selected_core_media_source: None,
            receiver_name: "FusionPlay".to_owned(),
            settings: SettingsView::default(),
            playback: PlaybackView::default(),
            remote_control: RemoteControlView::default(),
            last_error: None,
        }
    }
}

/// Mirrors Dart `AppSettings` (schemaVersion 8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub receiver_name: Option<String>,
    #[serde(default = "default_true")]
    pub startup_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_wake_enabled: bool,
    #[serde(default)]
    pub advanced_effects_enabled: bool,
    #[serde(default = "default_true")]
    pub mi_play_enabled: bool,
    #[serde(default = "default_miplay_identity")]
    pub mi_play_device_identity: String,
    #[serde(default = "default_true")]
    pub air_play_enabled: bool,
    #[serde(default = "default_true")]
    pub dlna_enabled: bool,
}

const fn default_schema_version() -> u32 {
    8
}
const fn default_true() -> bool {
    true
}
fn default_miplay_identity() -> String {
    "television".to_owned()
}

impl Default for SettingsView {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            receiver_name: None,
            startup_enabled: true,
            auto_wake_enabled: true,
            advanced_effects_enabled: false,
            mi_play_enabled: true,
            mi_play_device_identity: default_miplay_identity(),
            air_play_enabled: true,
            dlna_enabled: true,
        }
    }
}

/// Mirrors Dart `PlaybackSnapshot`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackView {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub cover_art: Option<String>,
    #[serde(default)]
    pub media_url: Option<String>,
    #[serde(default)]
    pub media_kind: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub quality_text: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub volume_percent: Option<u8>,
    #[serde(default)]
    pub is_playing: bool,
    #[serde(default)]
    pub stream_active: bool,
    #[serde(default)]
    pub source_epoch: Option<u64>,
    #[serde(default)]
    pub track_identity: Option<String>,
    #[serde(default)]
    pub track_change_direction: Option<String>,
}

/// Mirrors Dart `RemoteControlState`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlView {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub experimental: bool,
}

// ---------------------------------------------------------------------------
// events (sidecar -> UI)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyInfo {
    pub app: String,
    pub role: String,
    pub version: String,
    pub data_dir: String,
    pub log_file: String,
    pub null_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    Ready {
        app: String,
        role: String,
        version: String,
        #[serde(rename = "dataDir")]
        data_dir: String,
        #[serde(rename = "logFile")]
        log_file: String,
        #[serde(rename = "nullAudio")]
        null_audio: bool,
    },
    AppState {
        state: Box<AppStateView>,
    },
    Log {
        timestamp: String,
        level: LogLevel,
        source: String,
        message: String,
    },
    Response {
        #[serde(rename = "requestId")]
        request_id: Option<String>,
        command: String,
        ok: bool,
        message: Option<String>,
    },
    Fatal {
        message: String,
        #[serde(rename = "exitCode")]
        exit_code: i32,
    },
    Exiting {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// commands (UI -> sidecar)
// ---------------------------------------------------------------------------

/// Flat command envelope: command names mirror the Android MethodChannel
/// methods. Most commands carry their value in `value`; `seek` / `volume` use
/// their dedicated numeric field.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UiCommand {
    pub command: String,
    #[serde(default, rename = "requestId")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default, rename = "positionMs")]
    pub position_ms: Option<u64>,
    #[serde(default)]
    pub percent: Option<u8>,
}

impl UiCommand {
    pub fn value_str(&self) -> Option<&str> {
        self.value.as_ref().and_then(serde_json::Value::as_str)
    }

    pub fn value_bool(&self) -> Option<bool> {
        self.value.as_ref().and_then(serde_json::Value::as_bool)
    }

    pub fn value_string(&self) -> String {
        match self.value.as_ref() {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_channels_have_fresh_epoch_scoped_defaults() {
        let state = AppStateView::default();
        assert_eq!(state.playback.position_ms, 0);
        assert!(!state.playback.is_playing);
        assert_eq!(state.settings.schema_version, 8);
        assert!(state.settings.air_play_enabled);
    }

    #[test]
    fn app_state_serializes_camel_case_keys_expected_by_dart() {
        let json = serde_json::to_value(AppStateView::default()).unwrap();
        assert!(json.get("coreRunning").is_some());
        assert!(json.get("receiverReady").is_some());
        assert!(json.get("activeMediaSource").is_some());
        assert_eq!(json["settings"]["schemaVersion"], 8);
        assert_eq!(json["settings"]["miPlayDeviceIdentity"], "television");
        assert_eq!(json["playback"]["positionMs"], 0);
        assert_eq!(json["remoteControl"]["available"], false);
    }

    #[test]
    fn commands_accept_both_value_shapes_and_request_ids() {
        let command: UiCommand =
            serde_json::from_str(r#"{"command":"setReceiverName","value":"客厅","requestId":"c-1"}"#)
                .unwrap();
        assert_eq!(command.command, "setReceiverName");
        assert_eq!(command.value_str(), Some("客厅"));
        assert_eq!(command.request_id.as_deref(), Some("c-1"));

        let command: UiCommand =
            serde_json::from_str(r#"{"command":"setStartupEnabled","value":false}"#).unwrap();
        assert_eq!(command.value_bool(), Some(false));

        let command: UiCommand =
            serde_json::from_str(r#"{"command":"seek","positionMs":42500}"#).unwrap();
        assert_eq!(command.position_ms, Some(42_500));

        let command: UiCommand = serde_json::from_str(r#"{"command":"playback","value":"play_pause"}"#)
            .unwrap();
        assert_eq!(command.value_str(), Some("play_pause"));
    }

    #[test]
    fn fatal_and_ready_events_use_the_documented_envelope() {
        let ready = UiEvent::Ready {
            app: "FusionPlay".into(),
            role: "core".into(),
            version: "1.3.0".into(),
            data_dir: "C:/Users/me/AppData/Local/FusionPlay".into(),
            log_file: "log.txt".into(),
            null_audio: false,
        };
        let json = serde_json::to_value(ready).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["dataDir"], "C:/Users/me/AppData/Local/FusionPlay");
        assert_eq!(json["nullAudio"], false);

        let fatal = UiEvent::Fatal {
            message: "FusionPlay 已在运行".into(),
            exit_code: 2,
        };
        let json = serde_json::to_value(fatal).unwrap();
        assert_eq!(json["type"], "fatal");
        assert_eq!(json["exitCode"], 2);
    }
}
