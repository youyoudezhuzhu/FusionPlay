# FusionPlay Windows 版：架构与接口契约（1.3.0）

> 目标：在 `FusionPlay` 仓库内新增 Windows 桌面接收端，功能对齐 Android 版
> （AirPlay 2 / DLNA / 小米妙播 MiPlay 三个协议），UI 复用现有 Flutter 界面。
> 本文是 Windows 版本实现与验收的唯一接口契约；改接口必须同步改本文与两侧实现。

## 1. 进程模型

```text
┌────────────────────────────┐        NDJSON (stdin/stdout, UTF-8)        ┌─────────────────────────────┐
│ FusionPlay.exe             │  Dart → Rust: {"command": ...}             │ FusionPlayCore.exe          │
│  Flutter Windows 宿主/界面  │ ────────────────────────────────────────► │  Rust 侧车（唯一后端）        │
│  （纯视图 + 进程管理）       │ ◄──────────────────────────────────────── │  协议/音频/仲裁/设置/状态     │
└────────────────────────────┘   Rust → Dart: app_state / log / ...       └─────────────────────────────┘
```

- `FusionPlay.exe`：Flutter Windows 应用（本仓库 `flutter/`），**只做视图**：启动/守护侧车、
  读 JSON 事件、把 `app_state` 映射成 Dart `AppState`、把用户操作转成命令。
- `FusionPlayCore.exe`：本仓库 `windows/backend` 产出的 Rust 侧车进程（与 `FusionPlay.exe`
  同目录）。它拥有全部业务状态：设置持久化、三协议生命周期、音源仲裁、音频输出、
  Windows 开机自启注册、日志文件与诊断导出。
- 两者通过 **一行一个 JSON** 通信（UTF-8，无 BOM，`\n` 结尾）。侧车 `stderr` 仅用于
  tracing 原始日志（Dart 可透传为诊断日志），业务事件只走 stdout。
- Dart 侧在应用退出时向侧车发 `{"command":"shutdown"}` 并等待其退出（超时则 kill）。
- 侧车 stdin 被关闭（父进程消失）时必须在约 2 秒内自行退出，不能残留后台进程。
- 单实例：侧车用 Windows 命名互斥量 `Local\FusionPlay-Core` 检测；已有实例时输出
  `fatal` 事件（`FusionPlay 已在运行`）并以退出码 2 结束。

## 2. 侧车命令行

```text
FusionPlayCore.exe [--data-dir <dir>] [--null-audio] [--output-device <name-or-id>]
                   [--no-airplay] [--no-dlna] [--no-miplay]
                   [--log-level <trace|debug|info|warn|error>] [--version] [--help]
```

- `--data-dir`：状态目录，默认 `%LOCALAPPDATA%\FusionPlay`（Linux 测试时默认
  `$XDG_STATE_HOME/fusionplay` 或 `~/.local/state/fusionplay`）。
- `--null-audio`：不使用真实音频设备（CI / 无声卡机器 / 纯协议测试），PCM 直接丢弃。
- `--output-device`：cpal 设备名或 id；缺省用系统默认输出。找不到设备时自动回退
  `null` 模式并记录一条 warning 日志（接收器仍可被发现与控制）。
- `--no-airplay` / `--no-dlna` / `--no-miplay`：仅用于测试；**正常启动的协议开关来自
  settings.json**，命令行参数只作为一次性覆盖。
- `--version` 打印版本号并退出 0。

## 3. 数据目录

`%LOCALAPPDATA%\FusionPlay\`：

| 文件/目录 | 内容 |
|---|---|
| `settings.json` | `AppSettings`（schemaVersion 8，字段见 §5.1，camelCase） |
| `logs\fusionplay.log` | 主日志（5 MiB 上限，超出保留尾部；参考 fnOS `logging.rs`） |
| `artwork\` | 核心写入的封面缓存文件（`CoverArt.path`） |
| `Identity\miplay-identity-v1.json` | MiPlay 持久身份（`idm_instance_id` / `lyra_instance_id` / `media_device_id`） |
| `miplay-receiver-names.json` | 历史名称（改名后旧 mDNS 路由 goodbye） |
| `airplay_state.json` | 核心持久状态（AirPlay 配对/MAC） |
| `exports\` | `exportLogs` 生成的诊断包 |

## 4. 进程内音频链路（侧车）

- 统一主输出：**48 kHz / 2ch / f32**（`TARGET_RATE=48000`），cpal 打开系统默认输出设备。
  若设备不支持 48k，则在 cpal 输出回调前重采样（rubato），引擎内部始终 48k。
- 三个 PCM 来源：
  - AirPlay：`fusionplay_core` 的 `PcmOutput` 钩子（`start_audio_external`，48k/2）
  - DLNA：`dlna_player`（HTTP 拉流 + symphonia 解码 + 重采样到 48k）
  - MiPlay：`fusionplay_miplay_sdk::MiPlayPcmOutput` 进程级 sink
- **同一时刻只有一个活动音源**（`session::Arbiter`）：新音源 claim 后旧音源 flush 且其 PCM
  被丢弃；MiPlay 被接管时调用 `ReceiverController::suspend_output()`（恢复时 `resume_output()`）。
  AirPlay ↔ DLNA 的接管由核心内部 `PlaybackArbiter` 完成；MiPlay 通过向核心发送
  `{"command":"takeover","source":"xiaomi_miplay"}` 参与仲裁（核心会 suspend AirPlay）。
- 发送端音量：AirPlay dB / DLNA % / MiPlay % 换算为线性 `sender_gain`，写入引擎时应用；
  UI 拖动音量滑块通过 `volume` 命令回写发送端（AirPlay `set_volume` 远程命令 /
  MiPlay `ReceiverController::set_volume`）。
- `autoWakeEnabled`：播放期间调用 `SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED)`
  防休眠，停止播放后清除。`advancedEffectsEnabled` 在 Windows 上仅持久化、无实际 DSP。
- 无设备（`--null-audio` 或 cpal 失败）：引擎 `mode="null"`，丢弃 PCM，其它状态照常。

## 5. NDJSON 事件（侧车 → Dart，stdout）

每行一个对象，字段顺序无关。`type` 字段区分：

### 5.1 `ready`（启动后第一行）

```json
{"type":"ready","app":"FusionPlay","role":"core","version":"1.3.0",
 "dataDir":"C:\\Users\\me\\AppData\\Local\\FusionPlay",
 "logFile":"C:\\Users\\me\\AppData\\Local\\FusionPlay\\logs\\fusionplay.log",
 "nullAudio":false}
```

### 5.2 `app_state`（状态变化；10 Hz 合并，始终发最新值）

```json
{"type":"app_state","state":{ ...见下... }}
```

`state` 与 `flutter/lib/models/app_state.dart` 的 `AppState.fromMap` **逐字段对应**：

```jsonc
{
  "initialized": true,
  "busy": false,
  "coreRunning": true,
  "receiverReady": true,
  "receiverPort": 7000,
  "receiverDeviceId": "02:11:22:33:44:55",
  "connectedClient": "192.168.1.20",       // 无连接时 null
  "activeMediaSource": "airplay",           // airplay|dlna|xiaomi_miplay|null
  "selectedCoreMediaSource": "airplay",     // 同上
  "receiverName": "客厅电脑",
  "settings": {
    "schemaVersion": 8,
    "receiverName": null,                   // null 表示使用系统计算机名
    "startupEnabled": true,
    "autoWakeEnabled": true,
    "advancedEffectsEnabled": false,
    "miPlayEnabled": true,
    "miPlayDeviceIdentity": "television",   // vehicle|television|tablet|speaker
    "airPlayEnabled": true,
    "dlnaEnabled": true
  },
  "playback": {
    "title": null, "artist": null, "album": null,
    "coverArt": null,                       // 文件绝对路径或 http(s) URL
    "mediaUrl": null, "mediaKind": "audio",
    "protocol": "airplay",                  // 音源协议名
    "qualityText": null,                    // 自由文本，如 "AAC 44.1 kHz"
    "durationMs": null, "positionMs": 0, "volumePercent": null,
    "isPlaying": false, "streamActive": false,
    "sourceEpoch": null, "trackIdentity": null,
    "trackChangeDirection": null            // previous|next
  },
  "remoteControl": {
    "available": false, "commands": [],
    "transport": null, "experimental": false
  },
  "lastError": null
}
```

字段语义（与 Android `AppStateTransitions` 对齐）：

| 字段 | 来源 |
|---|---|
| `initialized` | 侧车启动完成后恒 true |
| `busy` | 协议重启/设置保存/导出进行中 |
| `coreRunning` | 核心接收线程存活 |
| `receiverReady` | 收到核心 `receiver_ready` 或 DLNA ready |
| `receiverPort` / `receiverDeviceId` | 核心 `receiver_ready` |
| `connectedClient` | `client_connected` / `client_disconnected` |
| `activeMediaSource` | 当前活动音源（`session::Arbiter` 的 lease 来源） |
| `selectedCoreMediaSource` | 核心侧最近一次发声的音源（用于设置页/播放页一致性） |
| `playback.*` | `now_playing`/`cover_art`/`progress`/`playback_state`/`volume`/`stream_started`/`stream_stopped`（AirPlay），`dlna_media`/`dlna_seek`/`dlna_stop`/`dlna_volume`（DLNA），MiPlay `media_info`/`playback_state`/`volume`/`stream_started`/`stream_stopped`（MiPlay，映射为相同字段） |
| `qualityText` | AirPlay：`stream_started` 的 codec/采样率/位深；DLNA：`content_type`/`sample_rate`；MiPlay：`audio_format` |
| `trackIdentity` | 新曲目标识（AirPlay 用 metadata 组合、DLNA 用 URL、MiPlay 用 media_info 标题+来源）；用于封面切换动画 |
| `trackChangeDirection` | 上一曲/下一曲命令后置 `previous`/`next`，新曲目稳定后清空 |
| `remoteControl` | 核心 `remote_control_available` / `remote_control_unavailable`（Windows 仅 AirPlay 会提供） |
| `lastError` | 核心 `error`、协议启动失败、用户可见提示（如日志导出成功，5 秒后自动清除） |

### 5.3 `log`

```json
{"type":"log","timestamp":"2026-11-10T17:00:00.123+08:00",
 "level":"info","source":"core","message":"..."}
```

`level ∈ info|warning|error`；`source ∈ core|miplay|dlna|audio|host`。也写入日志文件。

### 5.4 `response`（命令回执）

```json
{"type":"response","requestId":"c-42","command":"setReceiverName","ok":true,"message":null}
```

`requestId` 来自命令；命令可省略 `requestId`（则不回执，仅靠 `app_state` 体现结果）。

### 5.5 `fatal`（不可恢复错误，随后进程退出）

```json
{"type":"fatal","message":"FusionPlay 已在运行","exitCode":2}
```

### 5.6 `exiting`（正常退出前最后一行）

```json
{"type":"exiting","reason":"shutdown"}
```

## 6. NDJSON 命令（Dart → 侧车，stdin）

命令名与 Android `NativeAppController` 的 MethodChannel 方法名一致，参数键也一致；
`requestId` 可选。未知命令回 `response.ok=false`。

| 命令 | 参数 | 语义 |
|---|---|---|
| `setReceiverName` | `value: string` | 空字符串表示恢复系统计算机名；写 settings 并以新名称重启三协议 |
| `setStartupEnabled` | `value: bool` | 写/删 `HKCU\...\CurrentVersion\Run` 的 `FusionPlay` 项 |
| `setAutoWakeEnabled` | `value: bool` | 持久化 + 立即生效 |
| `setAdvancedEffectsEnabled` | `value: bool` | 仅持久化（Windows 无 DSP） |
| `setProtocolEnabled` | `protocol: "airplay"\|"dlna"\|"miplay"`, `value: bool` | 持久化并只重启受影响的协议 |
| `setMiPlayDeviceIdentity` | `value: "vehicle"\|"television"\|"tablet"\|"speaker"` | 重启 MiPlay 广播 |
| `playback` | `value: "playPause"\|"previous"\|"next"`（Dart 枚举名，兼容 `play`/`pause`/`play_pause`/`previous_track`/`next_track`） | 转发给当前音源；`command` 键已被信封占用，动作放在 `value` |
| `seek` | `positionMs: int` | 转发给当前音源 |
| `volume` | `percent: 0..100` | 回写发送端音量 |
| `exportLogs` | 无 | 生成 `exports\FusionPlay-diagnostics-<ts>.zip`，`lastError` 提示路径 5 秒后清除 |
| `clearError` | 无 | 清空 `lastError` |
| `shutdown` | 无 | 优雅退出（等同 stdin EOF） |

示例：

```json
{"command":"setReceiverName","value":"客厅电脑","requestId":"c-1"}
{"command":"setProtocolEnabled","protocol":"miplay","value":false}
{"command":"playback","value":"play_pause"}
```

## 7. 代码结构与 ownership

```text
windows/
  backend/                    # crate fusionplay-windows-host，bin FusionPlayCore
    Cargo.toml
    build.rs                  # VERSIONINFO 资源（复用 core build.rs 思路）
    src/
      main.rs                 # CLI、stdin 命令循环、stdout UiSink、生命周期
      ui.rs / ui_protocol.rs  # NDJSON 类型 + StdioUiSink（10Hz 合并）
      app.rs                  # AppRuntime：装配 settings/state/hosts/audio，命令分发
      settings.rs             # AppSettings v8 读写（原子落盘）、默认值
      windows_api.rs          # 计算机名、开机自启注册表、接口/MAC、单实例、防休眠
      state.rs                # RuntimeState：事件摄取 + AppStateView 投影 + lastError
      protocols.rs            # 三协议宿主：core run_receiver_with_arbiter + MiPlayReceiver + DlnaPlayer
      session.rs              # 单音源 Arbiter（claim/release/suspend hook）
      audio/                  # cpal WASAPI 引擎（null 后端、重采样、音量）
      dlna_player.rs          # DLNA 流解码播放（symphonia）
```

| 归属 | 文件 |
|---|---|
| audio-dlna（T1） | `src/audio/**`, `src/session.rs`, `src/dlna_player.rs` |
| host-state（T2） | `src/app.rs`, `src/settings.rs`, `src/windows_api.rs`, `src/state.rs`, `src/protocols.rs` |
| lead | `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/ui.rs`, `src/ui_protocol.rs`, `docs/**`, 顶层版本/README |
| dart-runtime（T3） | `flutter/lib/backend/app_controller.dart`、`windows_app_controller.dart`、`flutter/lib/main.dart`、`flutter/test/**` |
| win-shell（T4） | `flutter/windows/**`, `flutter/tool/build_windows.ps1`, `.github/workflows/windows.yml`, `installer/**`, `build.rs`、图标 |

## 8. 验收

1. `cargo test` / `cargo clippy`（Linux 主机，`windows/backend`）：settings、状态投影、
   声源仲裁、DKNA 解码（合成 WAV）、NDJSON 往返均有单测。
2. `cargo check --target x86_64-pc-windows-msvc`（Linux 需要 `llvm-rc`，或设置
   `FUSIONPLAY_SKIP_VERSION_RESOURCE=1`）：Windows 专有代码必须能编译。
3. `flutter analyze` / `flutter test`：Dart 控制器用 fake 侧车脚本做协议单测。
4. GitHub Actions `windows.yml`（windows-latest）：
   - 构建 `FusionPlayCore.exe`（`--target x86_64-pc-windows-msvc`，`RUSTUP_TOOLCHAIN=stable`）
   - PowerShell 冒烟测试：`FusionPlayCore.exe --null-audio --no-miplay --data-dir <tmp>`
     启动后必须输出 `ready` + `app_state`（`receiverReady=true`），发送 `shutdown` 后退出 0；
   - `flutter build windows --release`，把侧车 exe 拷进 `build/windows/x64/runner/Release/`；
   - 打包 `FusionPlay-Windows-<ver>-x64.zip`、可选 Inno Setup 安装包，上传 artifact。
5. 本地产物验收：下载 CI zip，校验结构（`FusionPlay.exe`、`FusionPlayCore.exe`、
   `data/`、`flutter_windows.dll`）、sha256，并发布到 GitHub Release。

## 9. 1.3.0 非目标

- SMTC（Windows 系统媒体控制/媒体键）与 Windows 通知中心（后续版本）。
- 歌词、视频投放（AirPlay 视频/DLNA 视频、HLS）、DLNA HTTPS、多音频设备 fan-out、
  音量曲线/校准（这些是 fnOS 特性）。
- MSIX/自动更新；1.3.0 分发 zip + Inno Setup 安装包。
