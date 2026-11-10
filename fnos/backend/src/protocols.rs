//! Three-protocol hosts for the fnOS receiver.
//!
//! * AirPlay 2 + DLNA reuse `airplay-receiver-core` in-process. AirPlay PCM is
//!   forwarded through the core `PcmOutput` hook into the unified engine; DLNA
//!   media URLs arrive as `CoreEvent::DlnaMedia` and are fetched/decoded by
//!   [`crate::dlna_player`].
//! * MiPlay uses `fusionplay-miplay-sdk` with the process-wide PCM sink.
//!
//! Cross-protocol arbitration is owned here (`crate::session::Arbiter`): one
//! primary source at a time, the previous source is suspended (its protocol
//! session stays alive) and queued PCM is flushed.

use anyhow::{Context, Result};
use serde_json::Value;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use fusionplay_core::{
    CoreEvent, EventSink, MediaSource, PcmOutput, PcmSource, PlaybackArbiter, Settings,
    run_receiver_with_arbiter,
};
use fusionplay_miplay_sdk::{
    EventEmitter, MiPlayDeviceType, MiPlayPcmOutput, MiPlayReceiver, ReceiverConfig,
    ReceiverIdentity, ReceiverController,
};

use crate::audio::AudioEngine;
use crate::config::Config;
use crate::dlna_player::DlnaPlayer;
use crate::session::{Arbiter, Lease, Source, SuspendHook};
use crate::state::{ProtocolRuntime, SharedState};

/// Canonical protocol states shared by all three protocols and the Web UI.
/// Keeping one vocabulary prevents "就绪 / 监听中 / 运行中" style drift between
/// the overview and the receive page: both read the same backend state.
mod proto_state {
    pub const DISABLED: &str = "disabled";
    pub const STARTING: &str = "starting";
    pub const WAITING: &str = "waiting";
    pub const LOADING: &str = "loading";
    pub const STREAMING: &str = "streaming";
    pub const PAUSED: &str = "paused";
    pub const ERROR: &str = "error";
}

fn set_proto(runtime: &mut ProtocolRuntime, state: &str, detail: impl Into<String>) {
    runtime.state = state.to_string();
    runtime.detail = detail.into();
}

/// Map the persisted device identity to the SDK enum. `display_speaker` is an
/// Android legacy alias and maps to Speaker, matching the upstream app.
fn miplay_device_type(value: &str) -> MiPlayDeviceType {
    match value {
        "vehicle" => MiPlayDeviceType::Vehicle,
        "tablet" => MiPlayDeviceType::Tablet,
        "speaker" | "display_speaker" => MiPlayDeviceType::Speaker,
        _ => MiPlayDeviceType::Television,
    }
}

/// Map a core lifecycle status to the canonical AirPlay state.
fn airplay_core_state(core_state: &str) -> Option<(&'static str, &'static str)> {
    match core_state {
        "starting" => Some((proto_state::STARTING, "AirPlay 2 接收器启动中")),
        "ready" | "waiting" | "listening" => Some((proto_state::WAITING, "等待 AirPlay 2 投放")),
        "streaming" | "playing" => Some((proto_state::STREAMING, "正在播放 AirPlay 2 音频")),
        "paused" => Some((proto_state::PAUSED, "AirPlay 2 已暂停")),
        "error" | "failed" => Some((proto_state::ERROR, "AirPlay 2 接收异常（详见日志）")),
        _ => None,
    }
}

/// Shared PCM sink: claims the source on first block and gates PCM to the
/// engine through the arbiter.
pub struct EngineSink {
    engine: Arc<AudioEngine>,
    arbiter: Arc<Arbiter>,
    state: SharedState,
    lease: Mutex<Option<Lease>>,
    hooks: Mutex<std::collections::HashMap<Source, SuspendHook>>,
}

impl EngineSink {
    pub fn new(engine: Arc<AudioEngine>, arbiter: Arc<Arbiter>, state: SharedState) -> Arc<Self> {
        Arc::new(Self {
            engine,
            arbiter,
            state,
            lease: Mutex::new(None),
            hooks: Mutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn set_hook(&self, source: Source, hook: SuspendHook) {
        self.hooks.lock().unwrap().insert(source, hook);
    }

    fn hook(&self, source: Source) -> Option<SuspendHook> {
        self.hooks.lock().unwrap().get(&source).cloned()
    }

    fn claim(&self, source: Source) -> Lease {
        let mut lease = self.lease.lock().unwrap();
        if let Some(current) = *lease {
            if current.source == source {
                return current;
            }
        }
        let claim = self.arbiter.claim(source, "pcm");
        *lease = Some(claim.lease);
        let suspended = claim.suspended;
        drop(lease);
        if let Some(previous) = suspended {
            tracing::info!(
                "音源接管：{} -> {}",
                previous.source.as_str(),
                source.as_str()
            );
            if let Some(hook) = self.hook(previous.source) {
                hook(previous);
            }
        }
        self.state.set_current_source(Some(source));
        claim.lease
    }

    fn release(&self, source: Source) {
        let mut lease = self.lease.lock().unwrap();
        if let Some(current) = *lease {
            if current.source == source {
                self.engine.flush(source);
                self.arbiter.release(current);
                *lease = None;
                self.state.set_current_source(None);
            }
        }
    }
}

impl PcmOutput for EngineSink {
    fn write(
        &self,
        source: PcmSource,
        _epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        let source = match source {
            PcmSource::AirPlay => Source::AirPlay,
            PcmSource::Dlna => Source::Dlna,
            PcmSource::MiPlay => Source::MiPlay,
        };
        let lease = self.claim(source);
        if !self.arbiter.accepts(lease) {
            return;
        }
        self.engine.push_pcm(
            source,
            lease.epoch,
            sender_gain.clamp(0.0, 1.0),
            sample_rate,
            channels,
            samples,
        );
        {
            let mut state = self.state.state.write().unwrap();
            match source {
                Source::AirPlay => {
                    state.sender_volume.airplay_percent = Some((sender_gain.clamp(0.0, 1.0) * 100.0).round() as u8);
                }
                Source::Dlna => {
                    state.sender_volume.dlna_percent = Some((sender_gain.clamp(0.0, 1.0) * 100.0).round() as u8);
                }
                Source::MiPlay => {
                    state.sender_volume.miplay_percent = Some((sender_gain.clamp(0.0, 1.0) * 100.0).round() as u8);
                }
            }
        }
    }

    fn flush(&self, source: PcmSource) {
        let source = match source {
            PcmSource::AirPlay => Source::AirPlay,
            PcmSource::Dlna => Source::Dlna,
            PcmSource::MiPlay => Source::MiPlay,
        };
        self.release(source);
    }
}

impl MiPlayPcmOutput for EngineSink {
    fn write(&self, sender_gain: f32, sample_rate: u32, channels: u16, samples: &[f32]) {
        <Self as PcmOutput>::write(
            self,
            PcmSource::MiPlay,
            0,
            sender_gain,
            sample_rate,
            channels,
            samples,
        );
    }

    fn flush(&self) {
        <Self as PcmOutput>::flush(self, PcmSource::MiPlay);
    }
}

struct CoreHandle {
    commands: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    thread: Option<JoinHandle<()>>,
}

struct MiPlayHandle {
    receiver: MiPlayReceiver,
    controller: ReceiverController,
}

/// Owns the three protocol hosts and applies config restarts.
pub struct ProtocolHosts {
    engine: Arc<AudioEngine>,
    state: SharedState,
    arbiter: Arc<Arbiter>,
    sink: Arc<EngineSink>,
    dlna: Arc<DlnaPlayer>,
    core: Mutex<Option<CoreHandle>>,
    miplay: Mutex<Option<MiPlayHandle>>,
    shutdown: Arc<AtomicBool>,
    /// Fingerprint of the protocol-relevant config that is currently applied.
    /// Audio-only saves must NOT restart the protocol hosts (that would cut
    /// AirPlay/DLNA/MiPlay for 1–3 s on every volume tweak).
    applied: Mutex<Option<String>>,
}

impl ProtocolHosts {
    pub fn start(config: &Config, engine: Arc<AudioEngine>, state: SharedState) -> Result<Arc<Self>> {
        let arbiter = Arc::new(Arbiter::new());
        let sink = EngineSink::new(Arc::clone(&engine), Arc::clone(&arbiter), Arc::clone(&state));
        let dlna = DlnaPlayer::new(Arc::clone(&engine), Arc::clone(&arbiter), Arc::clone(&state));
        sink.set_hook(
            Source::Dlna,
            Arc::new({
                let dlna = Arc::clone(&dlna);
                move |_lease| dlna.stop()
            }),
        );
        let hosts = Arc::new(Self {
            engine,
            state,
            arbiter,
            sink,
            dlna,
            core: Mutex::new(None),
            miplay: Mutex::new(None),
            shutdown: Arc::new(AtomicBool::new(false)),
            applied: Mutex::new(None),
        });
        hosts.apply(config)?;
        Ok(hosts)
    }

    fn fingerprint(config: &Config) -> String {
        serde_json::json!({
            "name": config.receiver.name,
            "pin": config.receiver.pin,
            "interface": config.receiver.interface,
            "airplay": config.protocols.airplay.enabled,
            "dlna": config.protocols.dlna.enabled,
            "miplay": config.protocols.miplay.enabled,
            "miplay_device_type": config.protocols.miplay.device_type,
        })
        .to_string()
    }

    fn apply(&self, config: &Config) -> Result<()> {
        self.stop_hosts();
        let state_dir = state_dir();
        {
            let mut state = self.state.state.write().unwrap();
            let airplay = config.protocols.airplay.enabled;
            let dlna = config.protocols.dlna.enabled;
            let miplay = config.protocols.miplay.enabled;
            state.airplay.enabled = airplay;
            state.dlna.enabled = dlna;
            state.miplay.enabled = miplay;
            if airplay {
                set_proto(&mut state.airplay, proto_state::STARTING, "AirPlay 2 接收器启动中");
            } else {
                set_proto(&mut state.airplay, proto_state::DISABLED, "已在设置中关闭");
            }
            if dlna {
                set_proto(&mut state.dlna, proto_state::STARTING, "DLNA 接收器启动中");
            } else {
                set_proto(&mut state.dlna, proto_state::DISABLED, "已在设置中关闭");
            }
            if miplay {
                set_proto(&mut state.miplay, proto_state::STARTING, "小米妙播启动中");
            } else {
                set_proto(&mut state.miplay, proto_state::DISABLED, "已在设置中关闭");
            }
        }

        if config.protocols.airplay.enabled || config.protocols.dlna.enabled {
            self.start_core(config, &state_dir)?;
        }
        if config.protocols.miplay.enabled {
            if let Err(error) = self.start_miplay(config, &state_dir) {
                let message = format!("小米妙播启动失败：{error:#}");
                tracing::error!("{message}");
                self.state.record_error(message);
                let mut state = self.state.state.write().unwrap();
                state.miplay.state = "error".into();
                state.miplay.detail = "无法启动妙播接收（详见日志）".into();
            }
        } else {
            fusionplay_miplay_sdk::set_process_pcm_output(None);
            let mut state = self.state.state.write().unwrap();
            state.miplay.state = "disabled".into();
        }
        *self.applied.lock().unwrap() = Some(Self::fingerprint(config));
        Ok(())
    }

    /// Apply protocol-relevant config changes. Audio-only saves (curves/gain/
    /// delay/mute) are a no-op so playback is not interrupted.
    pub fn restart(&self, config: &Config) -> Result<()> {
        let fingerprint = Self::fingerprint(config);
        if self.applied.lock().unwrap().as_deref() == Some(fingerprint.as_str()) {
            let mut state = self.state.state.write().unwrap();
            state.airplay.enabled = config.protocols.airplay.enabled;
            state.dlna.enabled = config.protocols.dlna.enabled;
            state.miplay.enabled = config.protocols.miplay.enabled;
            return Ok(());
        }
        self.apply(config)
    }

    fn start_core(&self, config: &Config, state_dir: &Path) -> Result<()> {
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::clone(&self.state);
        let dlna = Arc::clone(&self.dlna);
        let events = Arc::new(EventSink::with_callback(Arc::new(move |json: String| {
            handle_core_event(json, Arc::clone(&state), Arc::clone(&dlna));
        })));
        let core_arbiter = Arc::new(PlaybackArbiter::new(Arc::clone(&events)));
        let sink: Arc<dyn PcmOutput> = Arc::clone(&self.sink) as Arc<dyn PcmOutput>;
        let settings = Settings {
            name: config.receiver.name.clone(),
            pin: config.receiver.pin.clone(),
            state_path: state_dir.join("airplay_state.json"),
            output_device: None,
            airplay_enabled: config.protocols.airplay.enabled,
            dlna_enabled: config.protocols.dlna.enabled,
            // fnOS is an audio receiver: advertise the audio-only AirPlay
            // profile so iPhones do not offer video/AirPlay mirroring to it.
            airplay_audio_only: true,
            // fnOS has no user-facing low-latency mode; keep the 2 s default.
            airplay_latency_frames: fusionplay_core::DEFAULT_AIRPLAY_LATENCY_FRAMES,
            audio_output: Some(sink),
        };
        let state_for_thread = Arc::clone(&self.state);
        let thread = std::thread::Builder::new()
            .name("fusionplay-core".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        state_for_thread.record_error(format!("无法创建核心运行时：{error}"));
                        return;
                    }
                };
                runtime.block_on(async move {
                    if let Err(error) =
                        run_receiver_with_arbiter(settings, events, receiver, core_arbiter).await
                    {
                        tracing::error!("核心接收器退出：{error:#}");
                        state_for_thread.record_error(format!("接收器退出：{error:#}"));
                    }
                });
            })
            .context("无法启动核心接收线程")?;
        *self.core.lock().unwrap() = Some(CoreHandle {
            commands: Some(commands),
            thread: Some(thread),
        });
        Ok(())
    }

    fn start_miplay(&self, config: &Config, state_dir: &Path) -> Result<()> {
        // Android enables the legacy fixed-key media compatibility by default
        // (commit 7d3cf88/8af8545). fnOS must do the same or older HyperOS
        // senders (no 0x6c session key, PES private IV) fail to play. Opt-out
        // is the same marker file the Android app uses.
        let legacy_compat = !state_dir.join("disable-legacy-fixed-media").is_file();
        if legacy_compat {
            fusionplay_miplay_sdk::media::enable_legacy_fixed_media_compatibility();
        }
        tracing::info!(
            "小米妙播旧版固定密钥媒体兼容：{}",
            if legacy_compat { "启用" } else { "已通过标记文件关闭" }
        );

        let (interface, ip, mac) = pick_interface(&config.receiver.interface)?;
        let identity = load_identity(state_dir)?;
        let retired_names = receiver_name_history(state_dir, &config.receiver.name);
        let state = Arc::clone(&self.state);
        let sink = Arc::clone(&self.sink);
        let events: EventEmitter = Arc::new(move |value: Value| {
            handle_miplay_event(value, Arc::clone(&state), Arc::clone(&sink));
        });
        fusionplay_miplay_sdk::set_process_pcm_output(Some(
            Arc::clone(&self.sink) as Arc<dyn MiPlayPcmOutput>
        ));
        let receiver_config = ReceiverConfig::new(
            config.receiver.name.clone(),
            identity,
            ip,
            interface,
        )
        .with_device_type(miplay_device_type(&config.protocols.miplay.device_type))
        .with_retired_receiver_names(retired_names)
        .with_initial_volume_percent(50)
        .with_hardware_address(Some(mac));
        // `MiPlayReceiver::start()` emits `receiver_ready` synchronously *before*
        // it returns. That event sets the protocol to "waiting"; overwriting it
        // afterwards with "starting" would leave the UI stuck on 启动中 with no
        // later event to correct it (observed in 0.2.1). Set the transitional
        // state first and never overwrite a state the event handler produced.
        {
            let mut state = self.state.state.write().unwrap();
            if state.miplay.state != proto_state::WAITING {
                set_proto(
                    &mut state.miplay,
                    proto_state::STARTING,
                    "小米妙播启动中（等待设备发现广播）",
                );
            }
        }
        let receiver = MiPlayReceiver::start(receiver_config, events)
            .context("启动小米妙播接收器失败（发现/鉴权/端口）")?;
        let controller = receiver.controller();
        {
            // If (unexpectedly) no ready event arrived, stay in "starting"
            // rather than faking a ready state; the UI will surface it.
            let mut state = self.state.state.write().unwrap();
            if state.miplay.state.is_empty() {
                set_proto(
                    &mut state.miplay,
                    proto_state::STARTING,
                    "小米妙播启动中（等待设备发现广播）",
                );
            }
        }
        *self.miplay.lock().unwrap() = Some(MiPlayHandle { receiver, controller });
        Ok(())
    }

    fn stop_hosts(&self) {
        if let Some(mut handle) = self.miplay.lock().unwrap().take() {
            handle.controller.stop();
            drop(handle.receiver);
            // Give media workers a moment to observe shutdown before the next
            // session (config restart) re-claims discovery sockets.
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        fusionplay_miplay_sdk::set_process_pcm_output(None);
        if let Some(mut handle) = self.core.lock().unwrap().take() {
            if let Some(commands) = handle.commands.take() {
                let _ = commands.send(r#"{"command":"shutdown"}"#.to_owned());
            }
            if let Some(thread) = handle.thread.take() {
                let _ = thread.join();
            }
        }
        self.dlna.stop();
        self.sink.release(Source::AirPlay);
        self.sink.release(Source::Dlna);
        self.sink.release(Source::MiPlay);
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.stop_hosts();
    }

    pub fn dlna(&self) -> Arc<DlnaPlayer> {
        Arc::clone(&self.dlna)
    }

    pub fn arbiter(&self) -> Arc<Arbiter> {
        Arc::clone(&self.arbiter)
    }
}

fn state_dir() -> PathBuf {
    std::env::var_os("FUSIONPLAY_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TRIM_PKGVAR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn pick_interface(requested: &str) -> Result<(String, std::net::Ipv4Addr, String)> {
    let interfaces = if_addrs::get_if_addrs().context("无法枚举网络接口")?;
    let mut candidates: Vec<_> = interfaces
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(ip) => Some((interface.name, ip)),
            IpAddr::V6(_) => None,
        })
        .collect();
    if !requested.trim().is_empty() {
        candidates.retain(|(name, _)| name == requested.trim());
    }
    let (name, ip) = candidates
        .into_iter()
        .next()
        .with_context(|| format!("没有可用的 IPv4 网络接口（请求：{requested}）"))?;
    let mac = std::fs::read_to_string(format!("/sys/class/net/{name}/address"))
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    Ok((name, ip, mac))
}

/// Persist receiver display names and return previous ones for MiPlay route
/// cleanup (renaming changes the Mi-Connect DNS instance label).
fn receiver_name_history(state_dir: &Path, current: &str) -> Vec<String> {
    let path = state_dir.join("miplay-receiver-names.json");
    let mut names: Vec<String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let retired: Vec<String> = names
        .iter()
        .filter(|name| name.as_str() != current && !name.trim().is_empty())
        .cloned()
        .collect();
    if !names.iter().any(|name| name == current) {
        names.push(current.to_owned());
    }
    if names.len() > 8 {
        let drain = names.len() - 8;
        names.drain(..drain);
    }
    if let Ok(text) = serde_json::to_string_pretty(&names) {
        let _ = std::fs::write(&path, text);
    }
    retired
}

fn load_identity(state_dir: &Path) -> Result<ReceiverIdentity> {
    let path = state_dir.join("miplay_identity.json");
    #[derive(serde::Serialize, serde::Deserialize)]
    struct StoredIdentity {
        idm_instance_id: String,
        lyra_instance_id: String,
        media_device_id: String,
    }
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(stored) = serde_json::from_str::<StoredIdentity>(&text) {
            return Ok(ReceiverIdentity::new(
                stored.idm_instance_id,
                stored.lyra_instance_id,
                stored.media_device_id,
            ));
        }
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_nanos(), std::process::id()).hash(&mut hasher);
    let base = hasher.finish();
    let idm = format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (base >> 32) as u32,
        (base >> 16) as u16,
        (base & 0xfff) as u16,
        ((base >> 12) & 0xfff) as u16,
        (base & 0xffffffffffff) as u64
    );
    let lyra = format!("{:08X}", (base >> 8) as u32);
    let identity = StoredIdentity {
        idm_instance_id: idm,
        lyra_instance_id: lyra,
        media_device_id: "fusionplay-nas".to_string(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&identity)?);
    Ok(ReceiverIdentity::new(
        identity.idm_instance_id,
        identity.lyra_instance_id,
        identity.media_device_id,
    ))
}

fn handle_core_event(json: String, state: SharedState, dlna: Arc<DlnaPlayer>) {
    let Ok(value) = serde_json::from_str::<Value>(&json) else {
        return;
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(|v| v.to_string());
    match kind {
        "receiver_ready" => {
            let mut runtime = state.state.write().unwrap();
            if runtime.airplay.enabled {
                runtime.airplay.port =
                    value.get("port").and_then(Value::as_u64).unwrap_or(0) as u16;
                set_proto(&mut runtime.airplay, proto_state::WAITING, "等待 AirPlay 2 投放");
            }
        }
        "status" => {
            let mut runtime = state.state.write().unwrap();
            if runtime.airplay.enabled {
                if let Some(core_state) = text("state") {
                    if let Some((state, detail)) = airplay_core_state(&core_state) {
                        set_proto(&mut runtime.airplay, state, detail);
                    }
                }
            }
        }
        "now_playing" => {
            let mut runtime = state.state.write().unwrap();
            runtime.media.title = text("title");
            runtime.media.artist = text("artist");
            runtime.media.album = text("album");
            runtime.media.cover_url = text("cover_url");
        }
        "volume" => {
            let mut runtime = state.state.write().unwrap();
            if value.get("source").and_then(Value::as_str) == Some("dlna") {
                runtime.sender_volume.dlna_percent =
                    value.get("percent").and_then(Value::as_u64).map(|v| v as u8);
            } else {
                runtime.sender_volume.airplay_db =
                    value.get("db").and_then(Value::as_f64).map(|v| v as f32);
                runtime.sender_volume.airplay_percent =
                    value.get("percent").and_then(Value::as_u64).map(|v| v as u8);
            }
        }
        "stream_started" => {
            let mut runtime = state.state.write().unwrap();
            let source = text("source").unwrap_or_default();
            if source == "dlna" {
                if runtime.dlna.enabled {
                    set_proto(&mut runtime.dlna, proto_state::STREAMING, "正在播放 DLNA 音频");
                }
            } else if runtime.airplay.enabled {
                set_proto(&mut runtime.airplay, proto_state::STREAMING, "正在播放 AirPlay 2 音频");
            }
        }
        "stream_stopped" => {
            let mut runtime = state.state.write().unwrap();
            if text("source").as_deref() == Some("dlna") {
                if runtime.dlna.enabled {
                    set_proto(&mut runtime.dlna, proto_state::WAITING, "等待 DLNA 投放");
                }
            } else if runtime.airplay.enabled {
                set_proto(&mut runtime.airplay, proto_state::WAITING, "等待 AirPlay 2 投放");
            }
        }
        "client_connected" => {
            state.state.write().unwrap().airplay.clients += 1;
        }
        "client_disconnected" => {
            let mut runtime = state.state.write().unwrap();
            runtime.airplay.clients = runtime.airplay.clients.saturating_sub(1);
        }
        "dlna_ready" => {
            let mut runtime = state.state.write().unwrap();
            if runtime.dlna.enabled {
                runtime.dlna.port = value.get("port").and_then(Value::as_u64).unwrap_or(0) as u16;
                set_proto(&mut runtime.dlna, proto_state::WAITING, "等待 DLNA 投放");
            }
        }
        "dlna_media" => {
            let Some(url) = text("url") else { return };
            {
                let mut runtime = state.state.write().unwrap();
                if runtime.dlna.enabled {
                    set_proto(&mut runtime.dlna, proto_state::LOADING, "正在加载 DLNA 媒体");
                }
                runtime.media.title = text("title");
                runtime.media.artist = text("artist");
                runtime.media.album = text("album");
                runtime.media.cover_url = text("artwork_url");
                runtime.media.duration_ms = value.get("duration_ms").and_then(Value::as_u64);
                runtime.media.position_ms = value.get("start_position_ms").and_then(Value::as_u64);
            }
            let state = Arc::clone(&state);
            std::thread::Builder::new()
                .name("dlna-media".into())
                .spawn(move || {
                    if let Err(error) = dlna.play_uri(&url) {
                        let message = format!("DLNA 播放失败：{error:#}");
                        tracing::error!("{message}");
                        state.record_error(message);
                        let mut runtime = state.state.write().unwrap();
                        if runtime.dlna.enabled {
                            set_proto(
                                &mut runtime.dlna,
                                proto_state::ERROR,
                                "DLNA 播放失败（详见诊断日志）",
                            );
                        }
                    }
                })
                .ok();
        }
        "dlna_volume" => {
            let mut runtime = state.state.write().unwrap();
            runtime.sender_volume.dlna_percent =
                value.get("percent").and_then(Value::as_u64).map(|v| v as u8);
        }
        "dlna_stop" => {
            dlna.stop();
            let mut runtime = state.state.write().unwrap();
            if runtime.dlna.enabled {
                set_proto(&mut runtime.dlna, proto_state::WAITING, "等待 DLNA 投放");
            }
        }
        "dlna_seek" => {
            let mut runtime = state.state.write().unwrap();
            let position = value.get("position_ms").and_then(Value::as_u64);
            runtime.media.position_ms = position;
            runtime.dlna.detail = "DLNA Seek 暂未支持（首版），播放位置由控制端显示".into();
        }
        "error" => {
            let message = text("message").unwrap_or_else(|| "未知错误".into());
            state.record_error(message);
        }
        "source_takeover" => {
            let mut runtime = state.state.write().unwrap();
            runtime.current_source = text("source");
        }
        "log" => {
            if let Some(message) = text("message") {
                tracing::info!("core: {message}");
            }
        }
        _ => {}
    }
}

fn handle_miplay_event(value: Value, state: SharedState, sink: Arc<EngineSink>) {
    let kind = value.get("event").and_then(Value::as_str).unwrap_or_default();
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(|v| v.to_string());
    match kind {
        "discovery_backend_failed" => {
            let mut runtime = state.state.write().unwrap();
            if runtime.miplay.enabled {
                set_proto(
                    &mut runtime.miplay,
                    proto_state::ERROR,
                    "小米妙播发现服务启动失败（详见诊断日志）",
                );
            }
        }
        "receiver_ready" | "discovery_backend_ready" => {
            let mut runtime = state.state.write().unwrap();
            if runtime.miplay.enabled {
                set_proto(&mut runtime.miplay, proto_state::WAITING, "等待小米妙播投放");
            }
        }
        "stream_started" => {
            let mut runtime = state.state.write().unwrap();
            runtime.miplay.state = proto_state::STREAMING.to_string();
            runtime.miplay.detail = "正在播放小米妙播音频".to_string();
            runtime.current_source = Some("miplay".into());
            runtime.media.title = text("title");
            runtime.media.artist = text("artist");
            runtime.media.album = text("album");
        }
        "stream_stopped" | "media_channel_closed" | "media_session_replaced" => {
            let mut runtime = state.state.write().unwrap();
            set_proto(&mut runtime.miplay, proto_state::WAITING, "等待小米妙播投放");
            if runtime.current_source.as_deref() == Some("miplay") {
                runtime.current_source = None;
            }
            sink.release(Source::MiPlay);
        }
        "playback_state" => {
            if let Some(playing) = value.get("playing").and_then(Value::as_bool) {
                let mut runtime = state.state.write().unwrap();
                if runtime.miplay.enabled {
                    if playing {
                        set_proto(&mut runtime.miplay, proto_state::STREAMING, "正在播放小米妙播音频");
                    } else {
                        set_proto(&mut runtime.miplay, proto_state::PAUSED, "小米妙播已暂停");
                    }
                }
            }
        }
        "audio_output_ready" => {
            // Audio-engine readiness must not replace the user-facing protocol
            // status text; it is logged for diagnostics only.
            tracing::info!("小米妙播统一音频引擎就绪");
        }
        "audio_format" => {
            let mut runtime = state.state.write().unwrap();
            if let Some(codec) = text("source_codec") {
                runtime.media.detail_codec = Some(codec);
            }
        }
        "error" => {
            let message = text("message").unwrap_or_else(|| "妙播错误".into());
            state.record_error(message);
            let mut runtime = state.state.write().unwrap();
            if runtime.miplay.enabled {
                set_proto(&mut runtime.miplay, proto_state::ERROR, "小米妙播异常（详见诊断日志）");
            }
        }
        "warning" => {
            if let Some(message) = text("message") {
                tracing::warn!("miplay: {message}");
            }
        }
        other => {
            // Keep discovery/transport event names visible at debug level; they
            // are essential when diagnosing duplicate or missing receivers.
            tracing::debug!("miplay event: {other}");
        }
    }
}

/// Start the AirPlay/DLNA/MiPlay hosts.
pub fn start(config: &Config, engine: Arc<AudioEngine>, state: SharedState) -> Result<Arc<ProtocolHosts>> {
    ProtocolHosts::start(config, engine, state)
}

/// Map a core media source (used for diagnostics only).
pub fn core_source_name(source: MediaSource) -> &'static str {
    source.protocol_name()
}

/// Map an internal source to the core event source string.
pub fn source_name(source: Source) -> &'static str {
    source.as_str()
}

/// Ensure `CoreEvent` is referenced so the import stays meaningful for hosts
/// that inspect event names; keeps the module self-documenting.
pub fn core_event_kind(event: &CoreEvent<'_>) -> &'static str {
    match event {
        CoreEvent::ReceiverReady { .. } => "receiver_ready",
        CoreEvent::DlnaMedia { .. } => "dlna_media",
        _ => "other",
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_lifecycle_states_map_to_one_vocabulary() {
        let waiting = airplay_core_state("ready").unwrap();
        assert_eq!(waiting, (proto_state::WAITING, "等待 AirPlay 2 投放"));
        assert_eq!(airplay_core_state("listening"), Some(waiting));
        assert_eq!(
            airplay_core_state("streaming"),
            Some((proto_state::STREAMING, "正在播放 AirPlay 2 音频"))
        );
        assert_eq!(
            airplay_core_state("paused"),
            Some((proto_state::PAUSED, "AirPlay 2 已暂停"))
        );
        assert_eq!(
            airplay_core_state("error").map(|(state, _)| state),
            Some(proto_state::ERROR)
        );
        assert_eq!(airplay_core_state("unknown-state"), None);
    }

    #[test]
    fn core_events_produce_canonical_airplay_and_dlna_states() {
        use crate::audio::NullBackend;
        use crate::config::AudioSettings;
        use serde_json::json;

        let engine = AudioEngine::new(AudioSettings::default(), NullBackend::new()).unwrap();
        let arbiter = Arc::new(Arbiter::new());
        let state = crate::state::AppState::new();
        {
            let mut runtime = state.state.write().unwrap();
            runtime.airplay.enabled = true;
            runtime.dlna.enabled = true;
        }
        let dlna = DlnaPlayer::new(Arc::clone(&engine), Arc::clone(&arbiter), Arc::clone(&state));
        let sink = EngineSink::new(Arc::clone(&engine), arbiter, Arc::clone(&state));
        let _ = &sink;

        handle_core_event(
            json!({"type":"receiver_ready","name":"FusionPlay","port":5000,"device_id":"AA:BB"}).to_string(),
            Arc::clone(&state),
            Arc::clone(&dlna),
        );
        {
            let runtime = state.snapshot();
            assert_eq!(runtime.airplay.state, "waiting");
            assert_eq!(runtime.airplay.detail, "等待 AirPlay 2 投放");
        }
        handle_core_event(
            json!({"type":"dlna_ready","friendly_name":"FusionPlay","port":34653}).to_string(),
            Arc::clone(&state),
            Arc::clone(&dlna),
        );
        {
            let runtime = state.snapshot();
            assert_eq!(runtime.dlna.state, "waiting");
            assert_eq!(runtime.dlna.detail, "等待 DLNA 投放");
        }
        handle_core_event(
            json!({"type":"stream_started","source":"airplay","epoch":1}).to_string(),
            Arc::clone(&state),
            Arc::clone(&dlna),
        );
        assert_eq!(state.snapshot().airplay.state, "streaming");
        handle_core_event(
            json!({"type":"stream_stopped","source":"airplay","epoch":1}).to_string(),
            Arc::clone(&state),
            Arc::clone(&dlna),
        );
        let runtime = state.snapshot();
        assert_eq!(runtime.airplay.state, "waiting");
        assert_eq!(runtime.airplay.detail, "等待 AirPlay 2 投放");
    }

    #[test]
    fn miplay_ready_event_uses_the_same_waiting_vocabulary() {
        use crate::audio::NullBackend;
        use crate::config::AudioSettings;
        use serde_json::json;

        let engine = AudioEngine::new(AudioSettings::default(), NullBackend::new()).unwrap();
        let arbiter = Arc::new(Arbiter::new());
        let state = crate::state::AppState::new();
        state.state.write().unwrap().miplay.enabled = true;
        let sink = EngineSink::new(engine, arbiter, state.clone());
        handle_miplay_event(
            json!({"event":"receiver_ready","name":"FusionPlay","address":"192.168.31.50","port":8899}),
            Arc::clone(&state),
            sink,
        );
        let runtime = state.snapshot();
        assert_eq!(runtime.miplay.state, "waiting");
        assert_eq!(runtime.miplay.detail, "等待小米妙播投放");
    }

    #[test]
    fn miplay_device_identity_maps_like_android() {
        assert_eq!(miplay_device_type("vehicle"), MiPlayDeviceType::Vehicle);
        assert_eq!(miplay_device_type("television"), MiPlayDeviceType::Television);
        assert_eq!(miplay_device_type("tablet"), MiPlayDeviceType::Tablet);
        assert_eq!(miplay_device_type("speaker"), MiPlayDeviceType::Speaker);
        assert_eq!(miplay_device_type("display_speaker"), MiPlayDeviceType::Speaker);
        assert_eq!(miplay_device_type("unknown"), MiPlayDeviceType::Television);
    }

    #[test]
    fn set_proto_keeps_state_and_detail_together() {
        let mut runtime = ProtocolRuntime::default();
        set_proto(&mut runtime, proto_state::WAITING, "等待 DLNA 投放");
        assert_eq!(runtime.state, "waiting");
        assert_eq!(runtime.detail, "等待 DLNA 投放");
        set_proto(&mut runtime, proto_state::DISABLED, "已在设置中关闭");
        assert_eq!(runtime.state, "disabled");
    }
}
