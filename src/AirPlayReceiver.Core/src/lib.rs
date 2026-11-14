mod audio;
mod dlna;
pub mod dlna_flv;
mod events;
mod host;
#[cfg(any(target_os = "android", test))]
mod network_identity;
mod pairing;
mod takeover;
mod video;

#[cfg(target_os = "android")]
mod android_jni;

pub use audio::{PcmOutput, PcmSource};
#[cfg(not(target_os = "android"))]
pub use audio::start_audio_external;
pub use events::{CoreEvent, EventSink};
pub use host::{
    DEFAULT_AIRPLAY_LATENCY_FRAMES, Settings, desktop_main, parse_settings_from, run_receiver,
    run_receiver_with_arbiter,
};
pub use takeover::{MediaSource, PlaybackArbiter};
