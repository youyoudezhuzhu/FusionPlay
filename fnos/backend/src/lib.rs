//! FusionPlay fnOS host library.
//!
//! Reuses `airplay-receiver-core` (AirPlay 2 + DLNA DMR control plane) and
//! `fusionplay-miplay-sdk` (MiPlay), and adds the fnOS-specific pieces:
//! unified multi-device ALSA engine, volume/calibration chain, three-protocol
//! arbitration, DLNA media fetching/decoding, HTTP API and management UI.

pub mod audio;
pub mod config;
pub mod dlna_player;
pub mod http;
pub mod instances;
#[path = "../../../src/shared_logging/mod.rs"]
mod shared_logging;
pub mod logging;
pub mod protocols;
pub mod session;
pub mod state;
pub mod volume;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
