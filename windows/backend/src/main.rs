//! `FusionPlayCore.exe` entry point.
//!
//! Owns the process lifecycle only: parse the CLI, install logging, start the
//! runtime, pump NDJSON commands from stdin and emit NDJSON events through a
//! [`StdioUiSink`]. All business logic lives in [`fusionplay_windows::app`].

use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::sync::Arc;

use fusionplay_windows::app::StartError;
use fusionplay_windows::logging;
use fusionplay_windows::ui::{StdioUiSink, UiSink};
use fusionplay_windows::ui_protocol::{LogLevel, UiCommand};
use fusionplay_windows::{AppRuntime, RuntimeConfig, VERSION};

struct CliArgs {
    data_dir: Option<PathBuf>,
    null_audio: bool,
    output_device: Option<String>,
    force_disable_airplay: bool,
    force_disable_dlna: bool,
    force_disable_miplay: bool,
    log_level: String,
    show_version: bool,
    show_help: bool,
}

fn parse_args() -> Result<CliArgs, String> {
    let mut args = CliArgs {
        data_dir: None,
        null_audio: false,
        output_device: None,
        force_disable_airplay: false,
        force_disable_dlna: false,
        force_disable_miplay: false,
        log_level: "info".to_owned(),
        show_version: false,
        show_help: false,
    };
    let mut iterator = std::env::args().skip(1);
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--data-dir" => {
                args.data_dir = Some(PathBuf::from(
                    iterator.next().ok_or("--data-dir 缺少值")?,
                ));
            }
            "--null-audio" => args.null_audio = true,
            "--output-device" => {
                args.output_device = Some(iterator.next().ok_or("--output-device 缺少值")?);
            }
            "--no-airplay" => args.force_disable_airplay = true,
            "--no-dlna" => args.force_disable_dlna = true,
            "--no-miplay" => args.force_disable_miplay = true,
            "--log-level" => {
                args.log_level = iterator.next().ok_or("--log-level 缺少值")?;
            }
            "--version" | "-V" => args.show_version = true,
            "--help" | "-h" => args.show_help = true,
            other => return Err(format!("未知参数：{other}")),
        }
    }
    Ok(args)
}

fn print_help() {
    println!(
        "FusionPlayCore {VERSION}\n\
         FusionPlay Windows 接收端侧车（AirPlay 2 / DLNA / 小米妙播）\n\n\
         用法：FusionPlayCore.exe [选项]\n\n\
           --data-dir <dir>          状态目录（默认 %LOCALAPPDATA%\\FusionPlay）\n\
           --null-audio              不使用真实音频设备（测试/无声卡）\n\
           --output-device <name>    指定音频输出设备（默认系统默认设备）\n\
           --no-airplay              本次启动禁用 AirPlay\n\
           --no-dlna                 本次启动禁用 DLNA\n\
           --no-miplay               本次启动禁用小米妙播\n\
           --log-level <level>       trace|debug|info|warn|error（默认 info）\n\
           --version                 打印版本号\n\
           --help                    显示本帮助\n\n\
         协议通过 stdin/stdout 的 NDJSON 与 UI 通信，详见 docs/WINDOWS_ARCHITECTURE.md。"
    );
}


fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            // stdout is the event channel; argument errors still surface as a
            // fatal event so a graphical launcher can show them.
            let ui = StdioUiSink::new();
            ui.fatal(&message, 1);
            std::process::exit(1);
        }
    };
    if args.show_help {
        print_help();
        return;
    }
    if args.show_version {
        println!("FusionPlayCore {VERSION}");
        return;
    }

    let data_dir = args
        .data_dir
        .clone()
        .unwrap_or_else(fusionplay_windows::windows_api::default_data_dir);
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        eprintln!("无法创建状态目录 {}：{error}", data_dir.display());
    }
    let log_file = data_dir.join("logs").join("fusionplay.log");
    let _logging_guard = match logging::init(&args.log_level, &log_file) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("无法初始化日志：{error:#}");
            None
        }
    };

    let ui = StdioUiSink::new();
    let interactive = std::io::stdin().is_terminal();
    let config = RuntimeConfig {
        data_dir,
        log_file,
        null_audio: args.null_audio,
        output_device: args.output_device.clone(),
        force_disable_airplay: args.force_disable_airplay,
        force_disable_dlna: args.force_disable_dlna,
        force_disable_miplay: args.force_disable_miplay,
        log_level: args.log_level.clone(),
    };

    let runtime = match AppRuntime::start(config, Arc::clone(&ui) as Arc<dyn UiSink>) {
        Ok(runtime) => runtime,
        Err(StartError::AlreadyRunning) => {
            ui.fatal("FusionPlay 已在运行", 2);
            std::process::exit(2);
        }
        Err(StartError::Other(error)) => {
            let message = format!("{error:#}");
            ui.fatal(&message, 1);
            std::process::exit(1);
        }
    };

    if interactive {
        eprintln!("FusionPlayCore {VERSION} 已启动；从 stdin 读取 NDJSON 命令。");
    }
    ui.ready(runtime.ready_info());

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        match lines.next() {
            Some(Ok(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<UiCommand>(trimmed) {
                    Ok(command) => {
                        let request_id = command.request_id.clone();
                        let name = command.command.clone();
                        let is_shutdown = name == "shutdown";
                        let outcome = runtime.handle_command(command);
                        if let Some(request_id) = request_id {
                            ui.response(
                                Some(request_id.as_str()),
                                &name,
                                outcome.ok,
                                outcome.message.as_deref(),
                            );
                        }
                        if is_shutdown {
                            break;
                        }
                    }
                    Err(error) => {
                        ui.log(
                            LogLevel::Warning,
                            "host",
                            &format!("无法解析 UI 命令：{error}（{trimmed}）"),
                        );
                    }
                }
            }
            Some(Err(error)) => {
                ui.log(LogLevel::Warning, "host", &format!("读取 stdin 失败：{error}"));
                break;
            }
            None => break,
        }
    }

    runtime.shutdown();
    ui.exiting("shutdown");
}
