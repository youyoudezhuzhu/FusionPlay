//! FusionPlay Windows host sidecar.
//!
//! `FusionPlayCore.exe` hosts the three receiver protocols (AirPlay 2, DLNA,
//! MiPlay) in one process, owns the audio output and the persisted settings,
//! and exposes its whole state to the Flutter UI as NDJSON on stdin/stdout.
//!
//! The interface is specified in `docs/WINDOWS_ARCHITECTURE.md`.

pub mod app;
pub mod audio;
pub mod dlna_player;
pub mod logging;
pub mod protocols;
pub mod session;
pub mod settings;
pub mod state;
pub mod ui;
pub mod ui_protocol;
pub mod windows_api;

pub use app::{AppRuntime, RuntimeConfig};
pub use ui_protocol::{AppStateView, PlaybackView, RemoteControlView, SettingsView, UiCommand};

/// Product version reported by `--version`, the `ready` event and the Windows
/// version resource. Keep in sync with the repository `VERSION` file.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
