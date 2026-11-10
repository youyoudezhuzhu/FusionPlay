//! Runtime assembly and UI command dispatch.
//!
//! `main.rs` only creates an [`AppRuntime`], prints the `ready` event, pumps
//! NDJSON commands and finally calls [`AppRuntime::shutdown`]. Everything the
//! UI can observe lives in [`RuntimeState`] and is published as `app_state`
//! events by a dedicated publisher thread (100 ms cadence, only on change).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;

use crate::protocols::ProtocolHosts;
use crate::settings::SettingsStore;
use crate::state::RuntimeState;
use crate::ui::UiSink;
use crate::ui_protocol::{
    AppStateView, LogLevel, ReadyInfo, SettingsView, UiCommand,
};
use crate::windows_api;
use crate::{VERSION, logging};

/// Everything `main.rs` resolves from the CLI before the runtime starts.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub log_file: PathBuf,
    pub null_audio: bool,
    pub output_device: Option<String>,
    pub force_disable_airplay: bool,
    pub force_disable_dlna: bool,
    pub force_disable_miplay: bool,
    pub log_level: String,
    /// One-shot AirPlay 2 latency override (`--airplay-latency-ms` / env).
    /// `None` keeps the persisted setting (default 2000 ms).
    pub airplay_latency_ms: Option<u32>,
}

/// Why the runtime could not start. `AlreadyRunning` maps to exit code 2.
#[derive(Debug)]
pub enum StartError {
    AlreadyRunning,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for StartError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

impl From<std::io::Error> for StartError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}

/// Result of one UI command.
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub ok: bool,
    pub message: Option<String>,
}

impl CommandOutcome {
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
        }
    }
}

/// Owns settings, protocol hosts, audio engine and the projected UI state.
pub struct AppRuntime {
    config: RuntimeConfig,
    ui: Arc<dyn UiSink>,
    store: SettingsStore,
    state: Arc<RuntimeState>,
    hosts: Mutex<Option<Arc<ProtocolHosts>>>,
    publisher: Mutex<Option<JoinHandle<()>>>,
    publisher_stop: Arc<AtomicBool>,
    keep_awake: Arc<AtomicBool>,
    _single_instance: Option<windows_api::SingleInstance>,
}

impl AppRuntime {
    pub fn start(config: RuntimeConfig, ui: Arc<dyn UiSink>) -> Result<Arc<Self>, StartError> {
        // Single instance first: a second FusionPlayCore must not steal the
        // mDNS/AirPlay ports from the running receiver.
        let single_instance =
            windows_api::acquire_single_instance().ok_or(StartError::AlreadyRunning)?;
        std::fs::create_dir_all(&config.data_dir).with_context(|| {
            format!("无法创建状态目录 {}", config.data_dir.display())
        })?;
        let store = SettingsStore::new(config.data_dir.clone());
        let mut settings = store.load();
        if config.force_disable_airplay {
            settings.air_play_enabled = false;
        }
        if config.force_disable_dlna {
            settings.dlna_enabled = false;
        }
        if config.force_disable_miplay {
            settings.mi_play_enabled = false;
        }
        let receiver_name = resolve_receiver_name(&settings);
        let state = Arc::new(RuntimeState::new(settings.clone(), receiver_name.clone()));

        // First run: honour the default "start with Windows" preference once the
        // settings file exists (Windows only; a no-op elsewhere).
        if settings.startup_enabled {
            if let Err(error) = windows_api::set_startup_enabled(true) {
                ui.log(
                    LogLevel::Warning,
                    "host",
                    &format!("无法注册开机自启：{error:#}"),
                );
            }
        } else if let Err(error) = windows_api::set_startup_enabled(false) {
            logging::append_line(&format!("清除开机自启失败：{error:#}"));
        }

        ui.log(
            LogLevel::Info,
            "host",
            &format!("FusionPlay {VERSION} 正在启动，接收器名称：{receiver_name}"),
        );
        let hosts = ProtocolHosts::start(&config, &settings, &receiver_name, Arc::clone(&state), Arc::clone(&ui))
            .map_err(StartError::Other)?;
        state.set_initialized(true);
        state.set_busy(false);
        ui.publish(state.view());

        let keep_awake = Arc::new(AtomicBool::new(false));
        let publisher_stop = Arc::new(AtomicBool::new(false));
        let publisher_stop_thread = Arc::clone(&publisher_stop);
        let publisher_state = Arc::clone(&state);
        let publisher_ui = Arc::clone(&ui);
        let publisher_keep_awake = Arc::clone(&keep_awake);
        let publisher = std::thread::Builder::new()
            .name("fusionplay-publisher".to_owned())
            .spawn(move || {
                let mut last: Option<AppStateView> = None;
                let mut awake = false;
                while !publisher_stop_thread.load(Ordering::SeqCst) {
                    let view = publisher_state.view();
                    if last.as_ref() != Some(&view) {
                        publisher_ui.publish(view.clone());
                        last = Some(view.clone());
                    }
                    let desired_awake = view.settings.auto_wake_enabled
                        && (view.playback.is_playing || view.playback.stream_active || view.connected_client.is_some());
                    if desired_awake != awake {
                        windows_api::set_keep_awake(desired_awake);
                        publisher_keep_awake.store(desired_awake, Ordering::SeqCst);
                        awake = desired_awake;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .context("无法启动状态发布线程")?;

        Ok(Arc::new(Self {
            config,
            ui,
            store,
            state,
            hosts: Mutex::new(Some(hosts)),
            publisher: Mutex::new(Some(publisher)),
            publisher_stop,
            keep_awake,
            _single_instance: Some(single_instance),
        }))
    }

    pub fn ready_info(&self) -> ReadyInfo {
        ReadyInfo {
            app: "FusionPlay".to_owned(),
            role: "core".to_owned(),
            version: VERSION.to_owned(),
            data_dir: self.config.data_dir.display().to_string(),
            log_file: self.config.log_file.display().to_string(),
            null_audio: self.config.null_audio,
        }
    }

    pub fn snapshot(&self) -> AppStateView {
        self.state.view()
    }

    pub fn handle_command(&self, command: UiCommand) -> CommandOutcome {
        match command.command.as_str() {
            "setReceiverName" => self.set_receiver_name(&command.value_string()),
            "setStartupEnabled" => {
                let Some(enabled) = command.value_bool() else {
                    return CommandOutcome::error("setStartupEnabled 缺少布尔值");
                };
                let outcome = self.update_settings(|settings| settings.startup_enabled = enabled);
                if !outcome.ok {
                    return outcome;
                }
                match windows_api::set_startup_enabled(enabled) {
                    Ok(()) => CommandOutcome::ok(),
                    Err(error) => CommandOutcome::error(format!("更新开机自启失败：{error:#}")),
                }
            }
            "setAutoWakeEnabled" => {
                let Some(enabled) = command.value_bool() else {
                    return CommandOutcome::error("setAutoWakeEnabled 缺少布尔值");
                };
                self.update_settings(|settings| settings.auto_wake_enabled = enabled)
            }
            "setAdvancedEffectsEnabled" => {
                let Some(enabled) = command.value_bool() else {
                    return CommandOutcome::error("setAdvancedEffectsEnabled 缺少布尔值");
                };
                self.update_settings(|settings| settings.advanced_effects_enabled = enabled)
            }
            "setProtocolEnabled" => {
                let Some(protocol) = command.protocol.as_deref() else {
                    return CommandOutcome::error("setProtocolEnabled 缺少 protocol");
                };
                let Some(enabled) = command.value_bool() else {
                    return CommandOutcome::error("setProtocolEnabled 缺少布尔值");
                };
                self.set_protocol_enabled(protocol, enabled)
            }
            "setMiPlayDeviceIdentity" => {
                self.set_miplay_identity(&command.value_string())
            }
            "setAirPlayLatencyMs" => {
                let Some(value) = command.value.as_ref().and_then(serde_json::Value::as_u64) else {
                    return CommandOutcome::error("setAirPlayLatencyMs 缺少整数毫秒值");
                };
                self.set_air_play_latency_ms(value.min(u64::from(u32::MAX)) as u32)
            }
            "playback" => {
                let action = command.value_string();
                let outcome = self.with_hosts(|hosts| hosts.playback(&action));
                match crate::protocols::normalize_playback_action(&action) {
                    Some("previous_track") => self.state.note_track_change("previous"),
                    Some("next_track") => self.state.note_track_change("next"),
                    _ => {}
                }
                outcome
            }
            "seek" => {
                let Some(position) = command.position_ms else {
                    return CommandOutcome::error("seek 缺少 positionMs");
                };
                self.with_hosts(|hosts| hosts.seek(position))
            }
            "volume" => {
                let Some(percent) = command.percent else {
                    return CommandOutcome::error("volume 缺少 percent");
                };
                self.with_hosts(|hosts| hosts.volume(percent))
            }
            "exportLogs" => self.export_logs(),
            "clearError" => {
                self.state.clear_error();
                CommandOutcome::ok()
            }
            "shutdown" => CommandOutcome::ok(),
            other => CommandOutcome::error(format!("未知命令：{other}")),
        }
    }

    fn with_hosts(&self, action: impl FnOnce(&ProtocolHosts) -> Result<()>) -> CommandOutcome {
        let guard = match self.hosts.lock() {
            Ok(guard) => guard,
            Err(_) => return CommandOutcome::error("接收器状态锁损坏"),
        };
        let Some(hosts) = guard.as_ref() else {
            return CommandOutcome::error("接收器未运行");
        };
        match action(hosts) {
            Ok(()) => CommandOutcome::ok(),
            Err(error) => {
                let message = format!("{error:#}");
                self.ui.log(LogLevel::Warning, "host", &message);
                CommandOutcome::error(message)
            }
        }
    }

    fn settings(&self) -> SettingsView {
        self.state.settings()
    }

    fn update_settings(
        &self,
        update: impl FnOnce(&mut SettingsView),
    ) -> CommandOutcome {
        let mut settings = self.settings();
        update(&mut settings);
        settings.schema_version = 8;
        if let Err(error) = self.store.save(&settings) {
            let message = format!("保存设置失败：{error:#}");
            self.ui.log(LogLevel::Error, "host", &message);
            return CommandOutcome::error(message);
        }
        self.state.apply_settings(settings);
        CommandOutcome::ok()
    }

    fn set_receiver_name(&self, raw: &str) -> CommandOutcome {
        let trimmed = raw.trim();
        if trimmed.chars().count() > 63 {
            return CommandOutcome::error("接收器名称不能超过 63 个字符");
        }
        let explicit = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        let mut settings = self.settings();
        settings.receiver_name = explicit.clone();
        if let Err(error) = self.store.save(&settings) {
            let message = format!("保存设置失败：{error:#}");
            self.ui.log(LogLevel::Error, "host", &message);
            return CommandOutcome::error(message);
        }
        let receiver_name = explicit
            .or_else(windows_api::computer_name)
            .unwrap_or_else(|| "FusionPlay".to_owned());
        self.state.apply_settings(settings.clone());
        self.state.set_receiver_name(receiver_name.clone());
        self.restart_hosts(&settings, &receiver_name)
    }

    fn set_protocol_enabled(&self, protocol: &str, enabled: bool) -> CommandOutcome {
        let mut settings = self.settings();
        match protocol.trim().to_ascii_lowercase().as_str() {
            "airplay" => settings.air_play_enabled = enabled,
            "dlna" => settings.dlna_enabled = enabled,
            "miplay" | "xiaomi_miplay" => settings.mi_play_enabled = enabled,
            other => return CommandOutcome::error(format!("未知协议：{other}")),
        }
        if !settings.air_play_enabled && !settings.dlna_enabled && !settings.mi_play_enabled {
            return CommandOutcome::error("至少需要启用一个接收协议");
        }
        if let Err(error) = self.store.save(&settings) {
            let message = format!("保存设置失败：{error:#}");
            self.ui.log(LogLevel::Error, "host", &message);
            return CommandOutcome::error(message);
        }
        self.state.apply_settings(settings.clone());
        let receiver_name = resolve_receiver_name(&settings);
        self.ui.log(
            LogLevel::Info,
            "host",
            &format!(
                "协议 {} 已{}",
                protocol,
                if enabled { "启用" } else { "关闭" }
            ),
        );
        self.restart_hosts(&settings, &receiver_name)
    }

    fn set_miplay_identity(&self, raw: &str) -> CommandOutcome {
        let normalized = raw.trim().to_ascii_lowercase();
        let identity = match normalized.as_str() {
            "vehicle" => "vehicle",
            "television" => "television",
            "tablet" => "tablet",
            "speaker" | "display_speaker" => "speaker",
            other => return CommandOutcome::error(format!("未知的妙播设备身份：{other}")),
        };
        let mut settings = self.settings();
        settings.mi_play_device_identity = identity.to_owned();
        if let Err(error) = self.store.save(&settings) {
            let message = format!("保存设置失败：{error:#}");
            self.ui.log(LogLevel::Error, "host", &message);
            return CommandOutcome::error(message);
        }
        self.state.apply_settings(settings.clone());
        let receiver_name = resolve_receiver_name(&settings);
        self.restart_hosts(&settings, &receiver_name)
    }

    fn set_air_play_latency_ms(&self, milliseconds: u32) -> CommandOutcome {
        let mut settings = self.settings();
        settings.air_play_latency_ms = milliseconds;
        if let Err(error) = self.store.save(&settings) {
            let message = format!("保存设置失败：{error:#}");
            self.ui.log(LogLevel::Error, "host", &message);
            return CommandOutcome::error(message);
        }
        self.state.apply_settings(settings.clone());
        let receiver_name = resolve_receiver_name(&settings);
        self.ui.log(
            LogLevel::Info,
            "host",
            &format!("AirPlay 2 广告延迟已设为 {milliseconds} ms"),
        );
        self.restart_hosts(&settings, &receiver_name)
    }

    fn restart_hosts(&self, settings: &SettingsView, receiver_name: &str) -> CommandOutcome {
        self.state.set_busy(true);
        let result = {
            let guard = match self.hosts.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    self.state.set_busy(false);
                    return CommandOutcome::error("接收器状态锁损坏");
                }
            };
            match guard.as_ref() {
                Some(hosts) => hosts.restart(settings, receiver_name),
                None => Err(anyhow::anyhow!("接收器未运行")),
            }
        };
        self.state.set_busy(false);
        match result {
            Ok(()) => {
                self.ui.publish(self.state.view());
                CommandOutcome::ok()
            }
            Err(error) => {
                let message = format!("重启接收器失败：{error:#}");
                self.ui.log(LogLevel::Error, "host", &message);
                self.state.set_last_error(Some(message.clone()));
                CommandOutcome::error(message)
            }
        }
    }

    fn export_logs(&self) -> CommandOutcome {
        self.state.set_busy(true);
        let result = windows_api::export_diagnostics(&self.config.data_dir, &self.config.log_file, &self.state.view());
        self.state.set_busy(false);
        match result {
            Ok(path) => {
                let message = format!("诊断日志已导出到 {}", path.display());
                self.ui.log(LogLevel::Info, "host", &message);
                self.state.set_export_hint(message);
                CommandOutcome::ok()
            }
            Err(error) => {
                let message = format!("导出诊断日志失败：{error:#}");
                self.ui.log(LogLevel::Error, "host", &message);
                self.state.set_last_error(Some(message.clone()));
                CommandOutcome::error(message)
            }
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut slot) = self.publisher.lock()
            && let Some(handle) = slot.take()
        {
            self.publisher_stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        if self.keep_awake.load(Ordering::SeqCst) {
            windows_api::set_keep_awake(false);
        }
        if let Ok(mut slot) = self.hosts.lock()
            && let Some(hosts) = slot.take()
        {
            hosts.stop();
        }
        self.ui.log(LogLevel::Info, "host", "FusionPlay 接收器已停止");
        let _ = json!({ "stopped": true });
    }
}

fn resolve_receiver_name(settings: &SettingsView) -> String {
    settings
        .receiver_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(windows_api::computer_name)
        .unwrap_or_else(|| "FusionPlay".to_owned())
}
