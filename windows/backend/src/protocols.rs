//! Three-protocol host wiring for the Windows sidecar.
//!
//! * AirPlay 2 + DLNA control plane: `fusionplay_core` running on a
//!   dedicated tokio thread, with decoded AirPlay PCM pushed into our
//!   [`AudioEngine`] through the core `PcmOutput` hook.
//! * DLNA media playback: [`DlnaPlayer`] pulls the announced URI, decodes it
//!   and writes PCM into the same engine.
//! * MiPlay (小米妙播): `fusionplay_miplay_sdk` with the process-wide PCM sink.
//!
//! A [`session::Arbiter`] guarantees exactly one audible source at a time;
//! MiPlay joins that arbitration through the SDK's suspend/resume hooks and the
//! core `takeover` command.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use fusionplay_core::{
    EventSink, PcmOutput, PlaybackArbiter, Settings, run_receiver_with_arbiter,
};
use fusionplay_miplay_sdk::{
    MediaAction, MiPlayDeviceType, MiPlayPcmOutput, MiPlayReceiver, ReceiverConfig,
    ReceiverController, ReceiverIdentity,
};

use crate::app::RuntimeConfig;
use crate::audio::{AudioEngine, EngineConfig};
use crate::dlna_player::{DlnaEvent, DlnaPlayer, DlnaTrack};
use crate::session::{Arbiter, Lease, Source};
use crate::state::RuntimeState;
use crate::ui::UiSink;
use crate::ui_protocol::{LogLevel, SettingsView};

/// How long to wait for the core thread to wind down on restart/shutdown.
const CORE_JOIN_TIMEOUT: Duration = Duration::from_secs(6);
/// MiPlay keeps discovery sockets bound briefly after stop; the next start must
/// wait so the same UDP/TCP ports can be re-registered.
const MIPLAY_RESTART_SETTLE: Duration = Duration::from_millis(400);

struct CoreHandle {
    commands: tokio::sync::mpsc::UnboundedSender<String>,
    thread: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

struct MiPlayHandle {
    receiver: MiPlayReceiver,
    controller: ReceiverController,
}

/// Fingerprint of the settings that require protocol hosts to restart.
fn fingerprint(settings: &SettingsView, receiver_name: &str) -> Value {
    json!({
        "name": receiver_name,
        "airplay": settings.air_play_enabled,
        "dlna": settings.dlna_enabled,
        "miplay": settings.mi_play_enabled,
        "miplay_identity": settings.mi_play_device_identity,
    })
}

/// Owns the three protocol hosts and applies settings restarts.
pub struct ProtocolHosts {
    config: RuntimeConfig,
    state: Arc<RuntimeState>,
    ui: Arc<dyn UiSink>,
    engine: Arc<AudioEngine>,
    arbiter: Arc<Arbiter>,
    dlna: Arc<DlnaPlayer>,
    core: Mutex<Option<CoreHandle>>,
    miplay: Mutex<Option<MiPlayHandle>>,
    applied: Mutex<Option<Value>>,
    shutdown: AtomicBool,
    self_weak: OnceLock<Weak<ProtocolHosts>>,
}

impl ProtocolHosts {
    pub fn start(
        config: &RuntimeConfig,
        settings: &SettingsView,
        receiver_name: &str,
        state: Arc<RuntimeState>,
        ui: Arc<dyn UiSink>,
    ) -> Result<Arc<Self>> {
        let warn_ui = Arc::clone(&ui);
        let engine = AudioEngine::start(
            EngineConfig {
                null_audio: config.null_audio,
                output_device: config.output_device.clone(),
            },
            Arc::new(move |message: &str| {
                warn_ui.log(LogLevel::Warning, "audio", message);
            }),
        )
        .context("初始化音频输出失败")?;
        let status = engine.status();
        ui.log(
            LogLevel::Info,
            "audio",
            &format!(
                "音频输出：{}（{} Hz / {} ch，模式 {}）",
                status.device_name.as_deref().unwrap_or("无设备"),
                status.sample_rate,
                status.channels,
                status.mode
            ),
        );

        let arbiter = Arc::new(Arbiter::new());
        let emit_ui = Arc::clone(&ui);
        let dlna = DlnaPlayer::new(
            Arc::clone(&engine),
            Arc::new(move |event: DlnaEvent| {
                if let DlnaEvent::Error { message } = &event {
                    emit_ui.log(LogLevel::Error, "dlna", message);
                }
            }),
        );

        let hosts = Arc::new(Self {
            config: config.clone(),
            state,
            ui: Arc::clone(&ui),
            engine,
            arbiter,
            dlna,
            core: Mutex::new(None),
            miplay: Mutex::new(None),
            applied: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            self_weak: OnceLock::new(),
        });
        let _ = hosts.self_weak.set(Arc::downgrade(&hosts));
        hosts.install_suspend_hooks();
        hosts.apply(settings, receiver_name)?;
        Ok(hosts)
    }

    fn weak(&self) -> Weak<ProtocolHosts> {
        self.self_weak.get().cloned().unwrap_or_default()
    }

    /// Suspend hooks: a newly claimed source flushes/stops the previous one.
    fn install_suspend_hooks(&self) {
        let weak = self.weak();
        self.arbiter.set_suspend_hook(
            Source::AirPlay,
            Arc::new(move |_lease: Lease| {
                if let Some(hosts) = weak.upgrade() {
                    hosts.engine.flush_source(Source::AirPlay);
                }
            }),
        );
        let weak = self.weak();
        self.arbiter.set_suspend_hook(
            Source::Dlna,
            Arc::new(move |_lease: Lease| {
                if let Some(hosts) = weak.upgrade() {
                    hosts.dlna.stop();
                }
            }),
        );
        let weak = self.weak();
        self.arbiter.set_suspend_hook(
            Source::MiPlay,
            Arc::new(move |_lease: Lease| {
                if let Some(hosts) = weak.upgrade() {
                    hosts.suspend_miplay_output();
                }
            }),
        );
    }

    fn suspend_miplay_output(&self) {
        if let Ok(guard) = self.miplay.lock()
            && let Some(handle) = guard.as_ref()
        {
            handle.controller.suspend_output();
        }
    }

    /// Claim the audio output for `source` and publish the takeover.
    fn claim(&self, source: Source, tag: &str) {
        let claim = self.arbiter.claim(source, tag);
        self.engine.set_active_source(Some(source));
        self.state.set_active_source(Some(source.as_str()));
        if let Some(suspended) = claim.suspended {
            self.ui.log(
                LogLevel::Info,
                "host",
                &format!(
                    "音源接管：{} -> {}",
                    suspended.source.as_str(),
                    source.as_str()
                ),
            );
            self.state.reset_remote_control();
        }
    }

    fn release(&self, source: Source) {
        if let Some(lease) = self.arbiter.current()
            && lease.source == source
        {
            self.arbiter.release(lease);
            self.engine.set_active_source(None);
            self.state.set_active_source(None);
        }
    }

    pub fn restart(&self, settings: &SettingsView, receiver_name: &str) -> Result<()> {
        let next = fingerprint(settings, receiver_name);
        if self
            .applied
            .lock()
            .map(|value| value.as_ref() == Some(&next))
            .unwrap_or(false)
        {
            self.state.apply_settings(settings.clone());
            return Ok(());
        }
        self.apply(settings, receiver_name)
    }

    fn apply(&self, settings: &SettingsView, receiver_name: &str) -> Result<()> {
        self.stop_hosts();
        self.state.apply_settings(settings.clone());
        if settings.air_play_enabled || settings.dlna_enabled {
            self.start_core(settings, receiver_name)?;
        }
        if settings.mi_play_enabled {
            if let Err(error) = self.start_miplay(settings, receiver_name) {
                let message = format!("小米妙播启动失败：{error:#}");
                self.ui.log(LogLevel::Error, "miplay", &message);
                self.state.set_last_error(Some(message));
            }
        } else {
            fusionplay_miplay_sdk::set_process_pcm_output(None);
            self.ui.log(LogLevel::Info, "miplay", "小米妙播已在设置中关闭");
        }
        if let Ok(mut slot) = self.applied.lock() {
            *slot = Some(fingerprint(settings, receiver_name));
        }
        Ok(())
    }

    fn start_core(&self, settings: &SettingsView, receiver_name: &str) -> Result<()> {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let state = Arc::clone(&self.state);
        let ui = Arc::clone(&self.ui);
        let weak = self.weak();
        let events = Arc::new(EventSink::with_callback(Arc::new(move |json: String| {
            let Ok(value) = serde_json::from_str::<Value>(&json) else {
                return;
            };
            let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
            if kind == "log" {
                let level = value.get("level").and_then(Value::as_str).unwrap_or("info");
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                ui.log(
                    match level {
                        "error" => LogLevel::Error,
                        "warning" | "warn" => LogLevel::Warning,
                        _ => LogLevel::Info,
                    },
                    "core",
                    message,
                );
            }
            state.ingest_core_event(&value);
            if let Some(hosts) = weak.upgrade() {
                hosts.handle_core_event_for_arbiter(&value);
            }
        })));
        let core_arbiter = Arc::new(PlaybackArbiter::new(Arc::clone(&events)));
        let output: Arc<dyn PcmOutput> = Arc::clone(&self.engine) as Arc<dyn PcmOutput>;
        let core_settings = Settings {
            name: receiver_name.to_owned(),
            pin: None,
            state_path: self.config.data_dir.join("airplay_state.json"),
            output_device: None,
            airplay_enabled: settings.air_play_enabled,
            dlna_enabled: settings.dlna_enabled,
            // Windows 1.3.0 is an audio receiver: advertise the audio-only
            // profile so iPhones do not offer mirroring/video we cannot show.
            airplay_audio_only: true,
            audio_output: Some(output),
        };
        let thread_state = Arc::clone(&self.state);
        let thread_ui = Arc::clone(&self.ui);
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = std::thread::Builder::new()
            .name("fusionplay-core".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        thread_ui.log(
                            LogLevel::Error,
                            "core",
                            &format!("无法创建核心运行时：{error}"),
                        );
                        return;
                    }
                };
                let result = runtime.block_on(async move {
                    run_receiver_with_arbiter(core_settings, events, command_rx, core_arbiter).await
                });
                if let Err(error) = result
                    && !thread_shutdown.load(Ordering::SeqCst)
                {
                    let message = format!("接收器退出：{error:#}");
                    thread_ui.log(LogLevel::Error, "core", &message);
                    thread_state.set_last_error(Some(message));
                }
                thread_state.set_core_running(false);
            })
            .context("无法启动核心接收线程")?;
        self.state.set_core_running(true);
        if let Ok(mut slot) = self.core.lock() {
            *slot = Some(CoreHandle {
                commands: command_tx,
                thread: Some(thread),
                shutdown,
            });
        }
        Ok(())
    }

    fn start_miplay(&self, settings: &SettingsView, receiver_name: &str) -> Result<()> {
        let data_dir = self.config.data_dir.clone();
        // Android/fnOS default: legacy fixed-key media compatibility for older
        // HyperOS senders. Marker file keeps an explicit opt-out durable.
        let legacy_compat = !data_dir.join("disable-legacy-fixed-media").is_file();
        if legacy_compat {
            fusionplay_miplay_sdk::media::enable_legacy_fixed_media_compatibility();
        }
        let interface = crate::windows_api::pick_lan_interface()
            .context("没有可用的局域网 IPv4 接口，无法启动小米妙播")?;
        let identity = load_miplay_identity(&data_dir)?;
        let retired = receiver_name_history(&data_dir, receiver_name);
        let state = Arc::clone(&self.state);
        let ui = Arc::clone(&self.ui);
        let weak = self.weak();
        let events: fusionplay_miplay_sdk::EventEmitter = Arc::new(move |value: Value| {
            state.ingest_miplay_event(&value);
            if let Some(hosts) = weak.upgrade() {
                hosts.handle_miplay_event_for_arbiter(&value);
            }
            if let Some(kind) = value.get("event").and_then(Value::as_str)
                && matches!(kind, "error" | "warning")
            {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("小米妙播异常");
                ui.log(
                    if kind == "error" {
                        LogLevel::Error
                    } else {
                        LogLevel::Warning
                    },
                    "miplay",
                    message,
                );
            }
        });
        fusionplay_miplay_sdk::set_process_pcm_output(Some(
            Arc::clone(&self.engine) as Arc<dyn MiPlayPcmOutput>
        ));
        let receiver_config = ReceiverConfig::new(
            receiver_name.to_owned(),
            identity,
            interface.ip,
            interface.name.clone(),
        )
        .with_device_type(miplay_device_type(&settings.mi_play_device_identity))
        .with_retired_receiver_names(retired)
        .with_initial_volume_percent(50)
        .with_hardware_address(interface.mac.clone());
        let receiver = MiPlayReceiver::start(receiver_config, events)
            .context("启动小米妙播接收器失败（发现/鉴权/端口）")?;
        let controller = receiver.controller();
        if let Ok(mut slot) = self.miplay.lock() {
            *slot = Some(MiPlayHandle {
                receiver,
                controller,
            });
        }
        Ok(())
    }

    fn stop_hosts(&self) {
        if let Ok(mut slot) = self.miplay.lock()
            && let Some(handle) = slot.take()
        {
            handle.controller.stop();
            drop(handle.receiver);
            std::thread::sleep(MIPLAY_RESTART_SETTLE);
        }
        fusionplay_miplay_sdk::set_process_pcm_output(None);
        if let Ok(mut slot) = self.core.lock()
            && let Some(mut handle) = slot.take()
        {
            handle.shutdown.store(true, Ordering::SeqCst);
            let _ = handle
                .commands
                .send(json!({"command": "shutdown"}).to_string());
            if let Some(thread) = handle.thread.take() {
                join_with_timeout(thread, CORE_JOIN_TIMEOUT);
            }
        }
        self.dlna.stop();
        self.release(Source::AirPlay);
        self.release(Source::Dlna);
        self.release(Source::MiPlay);
        self.engine.set_active_source(None);
    }

    /// Route core events that affect audio arbitration/playback.
    fn handle_core_event_for_arbiter(&self, value: &Value) {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
        let epoch = value.get("epoch").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "stream_started" => {
                if value.get("source").and_then(Value::as_str) == Some("airplay") {
                    self.claim(Source::AirPlay, "airplay_stream");
                }
            }
            "stream_stopped" => {
                if value.get("source").and_then(Value::as_str) == Some("airplay") {
                    self.release(Source::AirPlay);
                }
            }
            "source_takeover" => {
                let source = value
                    .get("source")
                    .and_then(Value::as_str)
                    .and_then(source_from_str);
                if let Some(source) = source {
                    self.claim(source, "core_takeover");
                }
            }
            "dlna_media" => {
                let track = DlnaTrack {
                    epoch,
                    url: value
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    media_kind: value
                        .get("media_kind")
                        .and_then(Value::as_str)
                        .unwrap_or("audio")
                        .to_owned(),
                    title: string_field(value, "title"),
                    artist: string_field(value, "artist"),
                    album: string_field(value, "album"),
                    artwork_url: string_field(value, "artwork_url"),
                    content_type: string_field(value, "content_type"),
                    sample_rate: value
                        .get("sample_rate")
                        .and_then(Value::as_u64)
                        .map(|rate| rate as u32),
                    channels: value
                        .get("channels")
                        .and_then(Value::as_u64)
                        .map(|channels| channels as u16),
                    duration_ms: value.get("duration_ms").and_then(Value::as_u64),
                    start_position_ms: value
                        .get("start_position_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                };
                if let Err(error) = self.dlna.play_track(track) {
                    self.state
                        .set_last_error(Some(format!("DLNA 播放失败：{error:#}")));
                }
            }
            "dlna_seek" => {
                if let Some(position) = value.get("position_ms").and_then(Value::as_u64) {
                    let _ = self.dlna.seek(position);
                }
            }
            "dlna_stop" => self.dlna.stop(),
            "dlna_volume" => {
                let percent = value.get("percent").and_then(Value::as_u64).unwrap_or(100) as u8;
                let muted = value
                    .get("muted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.dlna.set_volume(percent, muted);
            }
            _ => {}
        }
    }

    /// Route MiPlay events that affect audio arbitration.
    fn handle_miplay_event_for_arbiter(&self, value: &Value) {
        let kind = value.get("event").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "playback_state" => {
                let playing = value
                    .get("playing")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if playing {
                    self.claim_miplay();
                }
            }
            "stream_started" => self.claim_miplay(),
            "stream_stopped" | "media_channel_closed" => {
                self.release(Source::MiPlay);
            }
            _ => {}
        }
    }

    fn claim_miplay(&self) {
        let has_receiver = self
            .miplay
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        if !has_receiver {
            return;
        }
        if let Ok(guard) = self.miplay.lock()
            && let Some(handle) = guard.as_ref()
        {
            handle.controller.resume_output();
        }
        // Tell the core to suspend AirPlay/DLNA: the core arbiter owns the
        // AirPlay suspender and emits the canonical source_takeover event.
        let _ = self.send_core_command(
            json!({"command": "takeover", "source": "xiaomi_miplay"}),
        );
        self.claim(Source::MiPlay, "miplay_session");
    }

    /// Forward a transport command to the active source.
    ///
    /// The Flutter UI sends its Dart enum names (`playPause` / `previous` /
    /// `next`); the core and the MiPlay SDK use their own vocabularies, so the
    /// action is normalized here.
    pub fn playback(&self, action: &str) -> Result<()> {
        let action = normalize_playback_action(action)
            .ok_or_else(|| anyhow::anyhow!("未知播放命令：{action}"))?;
        if self.arbiter.current_source() == Some(Source::MiPlay) {
            let media_action = match action {
                "play" => MediaAction::Resume,
                "pause" => MediaAction::Pause,
                "play_pause" => MediaAction::Toggle,
                "previous_track" => MediaAction::Previous,
                "next_track" => MediaAction::Next,
                other => anyhow::bail!("未知播放命令：{other}"),
            };
            if let Ok(guard) = self.miplay.lock()
                && let Some(handle) = guard.as_ref()
            {
                handle.controller.send(media_action)?;
                return Ok(());
            }
            anyhow::bail!("小米妙播接收器未运行");
        }
        self.send_core_command(json!({"command": action}))
    }

    pub fn seek(&self, position_ms: u64) -> Result<()> {
        if self.arbiter.current_source() == Some(Source::MiPlay) {
            if let Ok(guard) = self.miplay.lock()
                && let Some(handle) = guard.as_ref()
            {
                handle.controller.send(MediaAction::Seek(position_ms))?;
                return Ok(());
            }
            anyhow::bail!("小米妙播接收器未运行");
        }
        self.send_core_command(json!({"command": "seek", "position_ms": position_ms}))
    }

    pub fn volume(&self, percent: u8) -> Result<()> {
        let percent = percent.min(100);
        if self.arbiter.current_source() == Some(Source::MiPlay) {
            if let Ok(guard) = self.miplay.lock()
                && let Some(handle) = guard.as_ref()
            {
                handle.controller.set_volume(percent)?;
                return Ok(());
            }
            anyhow::bail!("小米妙播接收器未运行");
        }
        // Core expects the target volume in `position_ms` (historical protocol).
        self.send_core_command(json!({"command": "set_volume", "position_ms": percent}))
    }

    fn send_core_command(&self, command: Value) -> Result<()> {
        let guard = self
            .core
            .lock()
            .map_err(|_| anyhow::anyhow!("核心状态锁损坏"))?;
        let Some(handle) = guard.as_ref() else {
            anyhow::bail!("接收器未运行");
        };
        handle
            .commands
            .send(command.to_string())
            .map_err(|_| anyhow::anyhow!("接收器未运行"))
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.stop_hosts();
        self.engine.stop();
    }

    pub fn engine_status(&self) -> crate::audio::EngineStatus {
        self.engine.status()
    }

    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }
}

fn join_with_timeout(thread: JoinHandle<()>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if thread.is_finished() {
        let _ = thread.join();
    }
}

/// Normalize the Flutter/Dart playback vocabulary to the core vocabulary.
pub fn normalize_playback_action(action: &str) -> Option<&'static str> {
    match action.trim() {
        "play" => Some("play"),
        "pause" => Some("pause"),
        "playPause" | "play_pause" | "toggle" => Some("play_pause"),
        "previous" | "previous_track" | "previousTrack" => Some("previous_track"),
        "next" | "next_track" | "nextTrack" => Some("next_track"),
        _ => None,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn source_from_str(source: &str) -> Option<Source> {
    match source {
        "airplay" => Some(Source::AirPlay),
        "dlna" => Some(Source::Dlna),
        "xiaomi_miplay" => Some(Source::MiPlay),
        _ => None,
    }
}

fn miplay_device_type(value: &str) -> MiPlayDeviceType {
    match value.trim().to_ascii_lowercase().as_str() {
        "vehicle" => MiPlayDeviceType::Vehicle,
        "tablet" => MiPlayDeviceType::Tablet,
        "speaker" | "display_speaker" => MiPlayDeviceType::Speaker,
        _ => MiPlayDeviceType::Television,
    }
}

/// Persisted MiPlay identity (`Identity/miplay-identity-v1.json`).
fn load_miplay_identity(data_dir: &Path) -> Result<ReceiverIdentity> {
    let path = data_dir.join("Identity").join("miplay-identity-v1.json");
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(stored) = serde_json::from_str::<StoredIdentity>(&text)
        && !stored.idm_instance_id.is_empty()
        && !stored.lyra_instance_id.is_empty()
    {
        return Ok(ReceiverIdentity::new(
            stored.idm_instance_id,
            stored.lyra_instance_id,
            stored.media_device_id,
        ));
    }
    let identity = StoredIdentity::generate();
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

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredIdentity {
    idm_instance_id: String,
    lyra_instance_id: String,
    media_device_id: String,
}

impl StoredIdentity {
    fn generate() -> Self {
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
            base & 0xffff_ffff_ffff
        );
        let lyra = format!("{:08X}", (base >> 8) as u32);
        let media = format!("{:08x}", base as u32);
        Self {
            idm_instance_id: idm,
            lyra_instance_id: lyra,
            media_device_id: media,
        }
    }
}

/// Persist receiver display names and return previous ones for MiPlay route
/// cleanup after a rename.
fn receiver_name_history(data_dir: &Path, current: &str) -> Vec<String> {
    let path = data_dir.join("miplay-receiver-names.json");
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
