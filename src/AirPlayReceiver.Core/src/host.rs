use crate::audio::{PcmOutput, ReceiverAudioHandler, list_output_devices, start_audio};
#[cfg(not(target_os = "android"))]
use crate::audio::start_audio_external;
use crate::dlna::{DmrController, DmrService};
use crate::events::{CoreEvent, EventSink};
use crate::pairing::{FilePairingStore, PersistedState};
use crate::takeover::{MediaSource, PlaybackArbiter};
use crate::video::ReceiverVideoBridge;
use anyhow::{Context, Result, bail};
use rand::RngCore;
use serde::Deserialize;
use shairplay::{RaopServer, RemoteCommand};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// Default AirPlay 2 minimum output latency: 88,200 frames = 2 s at 44.1 kHz,
/// which is what genuine AirPlay receivers advertise. Hosts with a low-latency
/// mode pass a smaller value via [`Settings::airplay_latency_frames`].
pub const DEFAULT_AIRPLAY_LATENCY_FRAMES: u32 = 88_200;

#[derive(Clone)]
pub struct Settings {
    pub name: String,
    pub pin: Option<String>,
    pub state_path: PathBuf,
    pub output_device: Option<String>,
    pub airplay_enabled: bool,
    pub dlna_enabled: bool,
    /// Advertise AirPlay as an audio-only speaker (AirPort10,115 model, no
    /// video/HLS feature bits). fnOS sets this so iPhones stop offering video.
    /// Android/desktop keep the existing video-capable Apple TV profile.
    pub airplay_audio_only: bool,
    /// Minimum output latency advertised to AirPlay 2 senders, in frames at
    /// 44.1 kHz. Default: 88,200 (2 s). Lower values reduce end-to-end latency
    /// (Windows low-latency mode) at the cost of tolerance for Wi-Fi jitter.
    /// The AirPlay server clamps out-of-range values (valid: 11,025..=176,400
    /// frames, i.e. 250 ms..4 s) back to 88,200. Classic AirPlay ignores this.
    pub airplay_latency_frames: u32,
    /// Host-installed unified output (fnOS). `None` keeps cpal/Oboe output.
    pub audio_output: Option<std::sync::Arc<dyn PcmOutput>>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("name", &self.name)
            .field("pin", &self.pin)
            .field("state_path", &self.state_path)
            .field("output_device", &self.output_device)
            .field("airplay_enabled", &self.airplay_enabled)
            .field("dlna_enabled", &self.dlna_enabled)
            .field("airplay_audio_only", &self.airplay_audio_only)
            .field("airplay_latency_frames", &self.airplay_latency_frames)
            .field("audio_output", &self.audio_output.as_ref().map(|_| "external"))
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct UiCommand {
    command: String,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    media_kind: Option<String>,
    #[serde(default)]
    position_ms: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    rate: Option<f32>,
    #[serde(default)]
    ready: Option<bool>,
}

pub fn desktop_main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "shairplay=info,airplay_receiver_core=info".parse().unwrap()),
        )
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("unable to start FusionPlay runtime");
    runtime.block_on(async {
        let events = Arc::new(EventSink::new());
        if let Err(error) = run_from_cli(Arc::clone(&events)).await {
            let message = format!("{error:#}");
            events.emit(CoreEvent::Error { message: &message });
            eprintln!("{message}");
            std::process::exit(1);
        }
    });
}

async fn run_from_cli(events: Arc<EventSink>) -> Result<()> {
    let settings = parse_settings()?;
    let (tx, rx) = mpsc::unbounded_channel();
    let stdin_task = tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let result = tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("无法监听 Ctrl+C")?;
            Ok(())
        }
        result = run_receiver(settings, events, rx) => result,
    };
    stdin_task.abort();
    result
}

pub async fn run_receiver(
    settings: Settings,
    events: Arc<EventSink>,
    commands: mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    let arbiter = Arc::new(PlaybackArbiter::new(Arc::clone(&events)));
    run_receiver_with_arbiter(settings, events, commands, arbiter).await
}

pub async fn run_receiver_with_arbiter(
    settings: Settings,
    events: Arc<EventSink>,
    commands: mpsc::UnboundedReceiver<String>,
    arbiter: Arc<PlaybackArbiter>,
) -> Result<()> {
    if !settings.airplay_enabled && !settings.dlna_enabled {
        bail!("AirPlay 和 DLNA 不能同时在核心进程中禁用");
    }
    events.emit(CoreEvent::Status {
        state: "starting",
        message: "正在启动媒体接收器",
    });

    // DLNA renders through Android's media player, so initializing an Oboe
    // output stream here only wastes memory and can contend with real playback.
    let audio_runtime = if settings.airplay_enabled {
        let artwork_dir = settings
            .state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("artwork");
        let output_devices = list_output_devices().unwrap_or_else(|error| {
            let message = format!("无法读取完整音频输出列表：{error:#}");
            events.emit(CoreEvent::Log {
                level: "warning",
                message: &message,
            });
            Vec::new()
        });
        let runtime = match settings.audio_output.as_ref() {
            #[cfg(not(target_os = "android"))]
            Some(output) => start_audio_external(
                Arc::clone(&events),
                Arc::clone(&arbiter),
                artwork_dir,
                Arc::clone(output),
            )?,
            #[cfg(target_os = "android")]
            Some(_) => start_audio(
                Arc::clone(&events),
                Arc::clone(&arbiter),
                artwork_dir,
                settings.output_device.as_deref(),
            )?,
            None => start_audio(
                Arc::clone(&events),
                Arc::clone(&arbiter),
                artwork_dir,
                settings.output_device.as_deref(),
            )?,
        };
        let mut selected_device_reported = false;
        for device in &output_devices {
            let selected = device.id.eq_ignore_ascii_case(&runtime.device_id);
            selected_device_reported |= selected;
            events.emit(CoreEvent::OutputDevice {
                name: &device.name,
                id: &device.id,
                is_default: device.is_default,
                sample_rate: if selected {
                    runtime.sample_rate
                } else {
                    device.sample_rate
                },
                channels: if selected {
                    runtime.channels
                } else {
                    device.channels
                },
                sample_format: if selected {
                    runtime.sample_format
                } else {
                    device.sample_format
                },
                bits_per_sample: if selected {
                    runtime.bits_per_sample
                } else {
                    device.bits_per_sample
                },
            });
        }
        if !selected_device_reported {
            events.emit(CoreEvent::OutputDevice {
                name: &runtime.device_name,
                id: &runtime.device_id,
                is_default: settings.output_device.is_none(),
                sample_rate: runtime.sample_rate,
                channels: runtime.channels,
                sample_format: runtime.sample_format,
                bits_per_sample: runtime.bits_per_sample,
            });
        }
        Some(runtime)
    } else {
        None
    };

    static PERSISTED_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let (persisted, mac) = {
        let _guard = PERSISTED_STATE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("persisted state lock");
        let mut persisted = PersistedState::load(&settings.state_path);
        let mac = persisted.mac.unwrap_or_else(|| {
            let mut generated = [0_u8; 6];
            generated[0] = 0x02;
            rand::thread_rng().fill_bytes(&mut generated[1..]);
            generated
        });
        persisted.mac = Some(mac);
        persisted.save(&settings.state_path);
        (persisted, mac)
    };

    let mut server = if settings.airplay_enabled {
        let audio = audio_runtime
            .as_ref()
            .expect("AirPlay audio runtime must be initialized");
        let pairing_store = Arc::new(FilePairingStore::new(
            settings.state_path.clone(),
            persisted,
        ));
        let video_bridge = Arc::new(ReceiverVideoBridge::new(
            Arc::clone(&events),
            Arc::clone(&arbiter),
        ));
        let mut builder = RaopServer::builder()
            .name(settings.name.clone())
            .hwaddr(mac)
            .pairing_store(pairing_store)
            .output_sample_rate(audio.sample_rate)
            .output_max_channels(audio.channels.min(u8::MAX as u16) as u8)
            .audio_latency_frames(settings.airplay_latency_frames)
            .audio_only(settings.airplay_audio_only)
            .hls_handler(video_bridge.clone());
        if let Some(pin) = &settings.pin {
            builder = builder.pin(pin.clone());
        }

        let handler = Arc::clone(&audio.handler);
        let server_handler: Arc<dyn shairplay::AudioHandler> = handler.clone();
        let mut server = builder
            .build(server_handler)
            .context("无法创建 AirPlay 2 协议服务")?;
        let weak_audio_handler = Arc::downgrade(&handler);
        arbiter.register_suspender(MediaSource::AirPlayAudio, move |lease| {
            if let Some(handler) = weak_audio_handler.upgrade() {
                handler.suspend_for_takeover(lease);
            }
        });
        let weak_video_bridge = Arc::downgrade(&video_bridge);
        arbiter.register_suspender(MediaSource::AirPlayVideo, move |lease| {
            if let Some(bridge) = weak_video_bridge.upgrade() {
                bridge.suspend_for_takeover(lease);
            }
        });
        server
            .start()
            .await
            .context("无法启动 AirPlay 2 网络服务；请检查端口占用和防火墙权限")?;
        Some((server, video_bridge))
    } else {
        events.emit(CoreEvent::Log {
            level: "info",
            message: "AirPlay 接收服务已在设置中关闭",
        });
        None
    };
    events.emit(CoreEvent::RemoteControlUnavailable {
        source: None,
        epoch: None,
        reason: "连接投放设备后将自动检测媒体控制能力",
    });

    let dlna_service = if settings.dlna_enabled {
        match DmrService::start(
            settings.name.clone(),
            mac,
            Arc::clone(&events),
            Arc::clone(&arbiter),
        )
        .await
        {
            Ok(service) => {
                events.emit(CoreEvent::Log {
                    level: "info",
                    message: "DLNA DMR 已启动，可从同一局域网的投放菜单选择此电脑",
                });
                Some(service)
            }
            Err(error) => {
                let message = format!("DLNA DMR 启动失败：{error:#}");
                events.emit(CoreEvent::Log {
                    level: "error",
                    message: &message,
                });
                None
            }
        }
    } else {
        events.emit(CoreEvent::Log {
            level: "info",
            message: "DLNA 接收服务已在设置中关闭",
        });
        None
    };
    let dlna_controller = dlna_service.as_ref().map(DmrService::controller);

    let service_port = server
        .as_ref()
        .map(|(active_server, _)| active_server.service_info().port)
        .unwrap_or(0);
    let device_id = format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    events.emit(CoreEvent::ReceiverReady {
        name: &settings.name,
        pin: settings.pin.as_deref(),
        port: service_port,
        device_id: &device_id,
    });
    let ready_message = match (settings.airplay_enabled, settings.dlna_enabled) {
        (true, true) => "等待 AirPlay 2 或 DLNA 投放",
        (true, false) => "等待 AirPlay 2 投放",
        (false, true) => "等待 DLNA 投放",
        (false, false) => unreachable!(),
    };
    events.emit(CoreEvent::Status {
        state: "ready",
        message: ready_message,
    });
    events.emit(CoreEvent::Log {
        level: "info",
        message: ready_message,
    });

    wait_for_ui_commands(
        audio_runtime
            .as_ref()
            .map(|audio| Arc::clone(&audio.handler)),
        server
            .as_ref()
            .map(|(_, video_bridge)| Arc::clone(video_bridge)),
        dlna_controller,
        Arc::clone(&arbiter),
        Arc::clone(&events),
        commands,
    )
    .await?;

    events.emit(CoreEvent::Status {
        state: "stopping",
        message: "正在停止媒体接收器",
    });
    if let Some(service) = dlna_service {
        service.stop().await;
    }
    if let Some((active_server, _)) = server.as_mut() {
        active_server.stop().await;
    }
    events.emit(CoreEvent::Status {
        state: "stopped",
        message: "接收器已停止",
    });
    Ok(())
}

async fn wait_for_ui_commands(
    handler: Option<Arc<ReceiverAudioHandler>>,
    video_bridge: Option<Arc<ReceiverVideoBridge>>,
    dlna_controller: Option<Arc<DmrController>>,
    arbiter: Arc<PlaybackArbiter>,
    events: Arc<EventSink>,
    mut commands: mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    while let Some(line) = commands.recv().await {
        let Ok(command) = serde_json::from_str::<UiCommand>(&line) else {
            continue;
        };
        let command_name = command.command.trim().to_ascii_lowercase();
        if command_name == "shutdown" {
            return Ok(());
        }
        if command_name == "takeover" {
            let source = command.source.as_deref().map(str::trim).unwrap_or_default();
            let result = if source.eq_ignore_ascii_case("xiaomi_miplay") {
                let media_kind = match command.media_kind.as_deref() {
                    Some(value) if value.eq_ignore_ascii_case("audio") => "audio",
                    Some(value) if value.eq_ignore_ascii_case("video") => "video",
                    _ => "unknown",
                };
                arbiter.takeover(
                    MediaSource::XiaomiMiPlay,
                    media_kind,
                    "external_takeover",
                    false,
                    |_| (),
                );
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "unsupported external takeover source: {source}"
                ))
            };
            emit_command_result(
                &events,
                command.request_id.as_deref(),
                &command_name,
                &result,
            );
            continue;
        }
        if command_name == "video_state" {
            let source = command.source.as_deref();
            let update_airplay = source.is_some_and(|value| value.eq_ignore_ascii_case("airplay"))
                || (source.is_none()
                    && arbiter.current_source() == Some(MediaSource::AirPlayVideo));
            let update_dlna = source.is_some_and(|value| value.eq_ignore_ascii_case("dlna"))
                || (source.is_none() && arbiter.current_source() == Some(MediaSource::Dlna));
            if update_airplay && let Some(video_bridge) = &video_bridge {
                video_bridge.update_state(
                    command.position_ms,
                    command.duration_ms,
                    command.rate,
                    command.ready,
                );
            }
            if update_dlna && let Some(controller) = &dlna_controller {
                controller.update_playback_state(
                    command.position_ms,
                    command.duration_ms,
                    command.rate,
                    command.ready,
                );
            }
            continue;
        }

        if command
            .source
            .as_deref()
            .is_none_or(|value| value.eq_ignore_ascii_case("dlna"))
            && let Some(controller) = &dlna_controller
            && let Some(result) = controller.handle_ui_command(&command_name, command.position_ms)
        {
            emit_command_result(
                &events,
                command.request_id.as_deref(),
                &command_name,
                &result,
            );
            continue;
        }

        let remote_command = remote_command_from_name(&command_name, command.position_ms);

        let Some(remote_command) = remote_command else {
            events.emit(CoreEvent::CommandResult {
                request_id: command.request_id.as_deref(),
                command: &command_name,
                ok: false,
                message: Some("未知的媒体控制命令"),
            });
            continue;
        };

        let Some(command_handler) = handler.as_ref().map(Arc::clone) else {
            let result = Err(anyhow::anyhow!("AirPlay service is not running"));
            emit_command_result(
                &events,
                command.request_id.as_deref(),
                &command_name,
                &result,
            );
            continue;
        };
        let result = match tokio::task::spawn_blocking(move || {
            command_handler.send_remote_command(remote_command)
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(anyhow::anyhow!("媒体控制任务失败：{error}")),
        };
        emit_command_result(
            &events,
            command.request_id.as_deref(),
            &command_name,
            &result,
        );
    }

    // The UI process closed its pipe, so shutting down is safer than leaving an
    // invisible receiver running in the background.
    Ok(())
}

fn emit_command_result(
    events: &EventSink,
    request_id: Option<&str>,
    command: &str,
    result: &Result<()>,
) {
    let error_message = result.as_ref().err().map(ToString::to_string);
    events.emit(CoreEvent::CommandResult {
        request_id,
        command,
        ok: result.is_ok(),
        message: error_message.as_deref(),
    });
}

fn remote_command_from_name(command_name: &str, position_ms: Option<u64>) -> Option<RemoteCommand> {
    match command_name {
        "play" => Some(RemoteCommand::Play),
        "pause" => Some(RemoteCommand::Pause),
        "play_pause" => Some(RemoteCommand::PlayPause),
        "previous_track" => Some(RemoteCommand::PreviousTrack),
        "next_track" => Some(RemoteCommand::NextTrack),
        "seek" => position_ms.map(RemoteCommand::SeekToPosition),
        "set_volume" => position_ms.map(|value| RemoteCommand::SetVolume(value.min(100) as u8)),
        _ => None,
    }
}

fn parse_settings() -> Result<Settings> {
    parse_settings_from(std::env::args().skip(1))
}

pub fn parse_settings_from(arguments: impl IntoIterator<Item = String>) -> Result<Settings> {
    let mut name = None;
    let mut pin = None;
    let mut state_path = PathBuf::from("airplay-state.json");
    let mut output_device = None;
    let mut airplay_enabled = true;
    let mut dlna_enabled = true;
    let mut args = arguments.into_iter();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--name" => {
                name = Some(args.next().context("--name 缺少值")?);
            }
            "--pin" => {
                pin = Some(args.next().context("--pin 缺少值")?);
            }
            "--transient" => {
                pin = None;
            }
            "--state" => {
                state_path = PathBuf::from(args.next().context("--state 缺少值")?);
            }
            "--output-device" => {
                output_device = Some(args.next().context("--output-device 缺少值")?);
            }
            "--disable-airplay" => {
                airplay_enabled = false;
            }
            "--disable-dlna" => {
                dlna_enabled = false;
            }
            "--help" | "-h" => {
                println!(
                    "airplay-receiver-core --name <名称> [--pin <四位PIN>|--transient] --state <状态文件>"
                );
                std::process::exit(0);
            }
            unknown => bail!("未知参数：{unknown}"),
        }
    }

    let mut name =
        name.context("--name is required so AirPlay and DLNA advertise the Windows computer name")?;
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        bail!("接收器名称不能为空");
    }
    if trimmed_name.chars().count() > 64 {
        bail!("接收器名称不能超过 64 个字符");
    }
    name = trimmed_name.to_owned();

    if let Some(value) = &pin
        && (value.len() != 4 || !value.chars().all(|character| character.is_ascii_digit()))
    {
        bail!("PIN 必须是四位数字");
    }

    Ok(Settings {
        name,
        pin,
        state_path,
        output_device,
        airplay_enabled,
        dlna_enabled,
        airplay_audio_only: false,
        airplay_latency_frames: DEFAULT_AIRPLAY_LATENCY_FRAMES,
        audio_output: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_command_deserializes() {
        let command: UiCommand =
            serde_json::from_str(r#"{"command":"next_track","request_id":"test-1"}"#).unwrap();
        assert_eq!(command.command, "next_track");
        assert_eq!(command.request_id.as_deref(), Some("test-1"));
    }

    #[test]
    fn computer_name_is_required_instead_of_using_a_product_default() {
        let error = parse_settings_from(Vec::<String>::new()).unwrap_err();
        assert!(error.to_string().contains("--name is required"));
    }

    #[test]
    fn supplied_computer_name_is_preserved_for_discovery() {
        let settings = parse_settings_from([
            "--name".to_owned(),
            "  LivingRoom-PC  ".to_owned(),
            "--transient".to_owned(),
        ])
        .unwrap();
        assert_eq!(settings.name, "LivingRoom-PC");
        assert_eq!(settings.pin, None);
    }

    #[test]
    fn airplay_latency_defaults_to_the_two_second_protocol_value() {
        let settings =
            parse_settings_from(["--name".to_owned(), "LivingRoom-PC".to_owned()]).unwrap();
        assert_eq!(
            settings.airplay_latency_frames, DEFAULT_AIRPLAY_LATENCY_FRAMES,
            "hosts without a low-latency mode must keep advertising 88,200 frames"
        );
        assert_eq!(DEFAULT_AIRPLAY_LATENCY_FRAMES, 88_200);
    }

    #[test]
    fn shutdown_command_does_not_require_request_id() {
        let command: UiCommand = serde_json::from_str(r#"{"command":"shutdown"}"#).unwrap();
        assert_eq!(command.command, "shutdown");
        assert_eq!(command.request_id, None);
    }

    #[test]
    fn play_pause_ui_command_maps_to_toggle_semantics() {
        assert_eq!(
            remote_command_from_name("play_pause", None),
            Some(RemoteCommand::PlayPause)
        );
    }

    #[test]
    fn explicit_pause_and_seek_commands_preserve_their_semantics() {
        assert_eq!(
            remote_command_from_name("pause", None),
            Some(RemoteCommand::Pause)
        );
        assert_eq!(
            remote_command_from_name("play", None),
            Some(RemoteCommand::Play)
        );
        assert_eq!(
            remote_command_from_name("seek", Some(42_500)),
            Some(RemoteCommand::SeekToPosition(42_500))
        );
        assert_eq!(remote_command_from_name("seek", None), None);
        assert_eq!(
            remote_command_from_name("set_volume", Some(42)),
            Some(RemoteCommand::SetVolume(42))
        );
        assert_eq!(
            remote_command_from_name("set_volume", Some(142)),
            Some(RemoteCommand::SetVolume(100))
        );
    }

    #[test]
    fn command_result_reports_dlna_ui_success_and_failure_with_request_id() {
        let events = EventSink::new();
        emit_command_result(&events, Some("dlna-next-ok"), "next_track", &Ok(()));
        emit_command_result(
            &events,
            Some("dlna-next-missing"),
            "next_track",
            &Err(anyhow::anyhow!("no queued DLNA media")),
        );

        let captured = events.captured_events();
        assert_eq!(
            captured[0],
            serde_json::json!({
                "type": "command_result",
                "request_id": "dlna-next-ok",
                "command": "next_track",
                "ok": true,
                "message": null
            })
        );
        assert_eq!(captured[1]["request_id"], "dlna-next-missing");
        assert_eq!(captured[1]["command"], "next_track");
        assert_eq!(captured[1]["ok"], false);
        assert_eq!(captured[1]["message"], "no queued DLNA media");
    }
}
