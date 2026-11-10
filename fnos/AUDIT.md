# FusionPlay fnOS 版：源码审计报告

> 审计对象：`youyoudezhuzhu/FusionPlay` @ `8af8545`（含 Codex `7d3cf88` 的 MiPlay 兼容修复）
> 分支：`fnos/fpk`；审计时间：2026-10-28。所有结论均来自仓库源码实读，
> 并已在 x86_64 Linux 上执行 `cargo check --manifest-path src/AirPlayReceiver.Core/Cargo.toml --all-targets`（通过）。
> 审计边界：本机没有 iPhone/iPad/HyperOS 手机，也没有第二块声卡，因此"真机验证"项单独标注。

## 0. 一句话结论

协议核心是**跨平台 Rust**（AGPL-3.0-only），AirPlay 2 / MiPlay 都能在 Linux x86_64 编译；
DLNA 只有**控制面（MediaRenderer/AVTransport）**，媒体拉流解码在宿主侧（Android 用 Kotlin MediaPlayer），
Linux 端必须新增；三协议都各自内置 cpal 单设备输出，**统一多设备音频引擎不存在，需要新增**。
因此 fnOS 版 = 复用三个协议栈 + 新增统一音频引擎/宿主/Web UI/FPK。

## 1. 仓库与构建基线

| 项 | 结论 |
|---|---|
| origin | `youyoudezhuzhu/FusionPlay`（Fork），默认分支 `main` = `8af8545` |
| upstream | `rosienosiesie/FusionPlay-Android`（已配置 remote） |
| 顶层结构 | `src/`（Rust 核心）、`vendor/shairplay`、`flutter/`（Android UI）、`tools/`、`docs/` |
| Workspace | 无根 Cargo workspace；三个 crate 各自独立 `Cargo.toml` + `Cargo.lock` |
| Linux 编译 | ✅ `cargo check` 全通过（cpal 0.17 → ALSA，需 `libasound2-dev`，本机 1.2.8） |
| License | 核心 `AGPL-3.0-only`；`THIRD_PARTY_NOTICES.md` 列 Flutter/Kotlin/Rust 依赖；FPK 分发需附许可证与对应源码（本仓库公开，满足 AGPL 源码义务，仍需在包内放 LICENSE） |
| Android 构建 | `.github/workflows/android.yml`（Flutter 3.32.8 + NDK + Rust），**本任务不改动它** |

## 2. 状态矩阵（按任务要求分类）

### 2.1 已完整实现，可直接复用

| 模块 | 证据 | 说明 |
|---|---|---|
| 控制通道/配对/AirPlay 2 协议栈 | `vendor/shairplay/AP2-STATUS.md`、`handlers_ap2.rs`、`crypto/*` | SRP-6a 临时配对、HomeKit 配对、ChaCha20-Poly1305、FairPlay fp-setup、`_airplay._tcp`+`_raop._tcp` |
| AP2 buffered audio（103）解码 | `vendor/shairplay`、`features.rs`（`0x0001C340405D4A00`） | AAC 解码、多声道下混、重采样、定时播放缓冲、pause/flush |
| AP2 realtime audio（96） | `raop` 路径 | ALAC 解码、即时投递 |
| AP2 元数据/封面/进度 | `handlers_ap2.rs`、`dmap`、`events.rs` | 音量、封面、进度、DMAP |
| DLNA MediaRenderer 控制面 | `src/AirPlayReceiver.Core/src/dlna.rs`（`DEVICE_TYPE=MediaRenderer:1`、SSDP 1900、AVTransport） | 发现、描述、AVTransport、RenderingControl、事件回传、错误码 |
| MiPlay 设备发现/鉴权/媒体接收 | `src/FusionPlay.MiPlaySdk`（`MiPlayReceiver::start`/`controller`、`media.rs`） | 跨平台（非 Android 用 cpal），Linux 上 Codex 已跑通官方平板→Linux 接收 |
| MiPlay 旧版固定密钥兼容 | `7d3cf88` + `8af8545`（android_jni.rs `enable_legacy_fixed_media_compatibility`） | 本任务**必须保留**；fnOS 后端同样默认启用 |
| 三协议音源仲裁框架 | `src/AirPlayReceiver.Core/src/takeover.rs`（`MediaSource::{AirPlayAudio,AirPlayVideo,Dlna,XiaomiMiPlay}`、`PlaybackArbiter`、suspend hook） | 设计上已支持三协议，可复用 |
| 事件总线 | `events.rs`（`CoreEvent` 含 `DlnaMedia`/`Volume`/`NowPlaying`/`StreamStarted` 等） | 可作为状态上报来源 |
| 桌面 headless 入口 | `host.rs`（`desktop_main`/`run_receiver`/`run_receiver_with_arbiter`/`Settings`）、`main.rs` | 已有非 Android 主循环（stdin JSON 命令），LF 可复用为 fnOS 宿主骨架 |

### 2.2 已实现，但依赖 Android

| 模块 | 证据 | fnOS 处置 |
|---|---|---|
| JNI 桥 + 生命周期 | `android_jni.rs` | 不参与 Linux 构建；fnOS 宿主走 Rust 原生入口 |
| Oboe 音频输出 + 路由恢复 | `audio.rs`（`#[cfg(target_os="android")]`） | 不复用；Linux 用新 ALSA 引擎 |
| DLNA 媒体播放 | Kotlin `DlnaMediaProbe.kt`/`AppViewModel` + Android MediaPlayer | **Linux 需新实现拉流+解码** |
| Android 前台服务/通知 | `flutter/android/...` | 不需要；FPK 用 `cmd/main` + Web UI |

### 2.3 已实现，但只支持 Windows

| 模块 | 证据 | 说明 |
|---|---|---|
| 桌面 cpal 单设备输出 | `audio.rs` `#[cfg(not(target_os="android"))]` 注释写 WASAPI/Windows | 代码本身是 cpal 跨平台，Linux 能编译，可作为**单设备回退** |
| Windows 宿主 UI | 本仓库 `flutter/` 只有 `android/`，无 `windows/` | 说明 Windows 宿主不在本树；fnOS 宿主需新建 |
| 注册表相关能力 | `windows-sys` 依赖（`Win32_System_Registry`） | 仅 Windows 编译单元，Linux 不引用 |

### 2.4 存在基础代码，需要适配 Linux

| 模块 | 现状 | 适配工作 |
|---|---|---|
| 桌面入口 `host.rs` | 已有 `run_receiver*`/`Settings`/stdin 命令循环，非 Android 可编译 | 新增"外部 PCM 输出"注入；新增 fnOS 宿主进程 |
| 音频输出 | core 与 MiPlaySdk 各自 cpal 单设备 | 抽 `PcmOutput` trait，Linux 实现统一多设备 ALSA 引擎 |
| 设备枚举 | `list_output_devices()` 走 cpal | fnOS 需要 ALSA 稳定 ID（`hw:CARD=<id>,DEV=<n>`）+ 热插拔 |
| 配置 | 核心只有 `--name/--pin/--state/--output-device/--disable-*` | 新增 JSON 配置（协议开关/设备/校准/日志） |
| 日志 | `tracing` 到 stderr（桌面） | 新增文件日志 + 5MiB/1MiB 原地轮转 + 级别热更 |

### 2.5 尚未实现，需要新增

| 模块 | 说明 |
|---|---|
| 统一 PCM 引擎 | 主音量曲线 → 全局衰减 → 每设备 dB → 多设备分发；单设备独立缓冲 + 重采样 + 漂移补偿 + 手动延迟 |
| 多设备同步 | 统一主时间轴、每设备时钟漂移检测/补偿、状态管理、拔插恢复 |
| 跨协议仲裁 | 三协议"同一虚拟接收器同时只播一个主音源"，接管时 flush + 挂起旧协议（复用 `PlaybackArbiter` 思路） |
| DLNA 媒体播放 | HTTP(S) 拉流 + MP3/FLAC/WAV/AAC/MP4 音频解码 → PCM（HLS/视频首版明确不支持） |
| fnOS 宿主进程 | HTTP API（Unix socket + TCP）、静态 Web UI、状态机、诊断、配置导入导出 |
| fnOS Web UI | 概览/音频/接收/设置四页，每设备独立曲线，中文、响应式、浅深色 |
| FPK 打包与 CI | manifest/cmd 9 脚本/privilege（`join-groups: ["audio"]`）/resource/ui/icons、`build-fnos.yml` |

### 2.6 实现存在，但必须真机验证

| 能力 | 为何必须真机 |
|---|---|
| AirPlay 2 普通 iPhone 投放/暂停恢复/切歌/发送端音量 | 需要真实 iPhone + 网络；CI 只能做单元/结构测试 |
| AirPlay 2 与 HomePod/其他 AP2 音箱共同组多房间 | 上游 PTP 只做到"映射到本地时间轴"，**未做多房间同步**；本项目不宣称支持，需真机验证边界 |
| DLNA 控制端（手机/DLNA App）发现与播放 | 需要真实控制点 + 真实媒体 URL |
| MiPlay 真实 HyperOS 发送端 | 需要小米手机/平板；且存在厂商准入限制，不能宣称官方认证 |
| 多声卡真实播放/同步/拔插 | 需要至少两块物理输出设备；CI 用 mock/null 引擎 |
| Intel N150 性能 | 需目标机实测（本项目按低占用设计，但不做无依据的性能断言） |

## 3. 关键风险与对策

1. **/dev/snd 权限**：fnOS 应用默认以 `run-as: package` 运行且不在 `audio` 组 →
   `aplay -l` 空、ALSA 打开失败（本机实测：`app 用户 → no soundcards found`）。
   对策：FPK `config/privilege` 增加 `"join-groups": ["audio"]`（官方规范支持），仍保持非 root。
2. **DLNA 媒体解码**：核心只做控制面。首版支持 MP3/FLAC/WAV/AAC（symphonia），
   HLS/视频/DLNA 转码不支持并在 UI 明确报错，不伪造成功。
3. **MiPlay 厂商准入**：保留诊断日志，失败如实上报；不伪造成"已认证"。
4. **上游合并**：所有改动放在 `fnos/` 新目录 + 少量平台抽象钩子（`PcmOutput`），
   不复制协议核心、不改 Android 行为。
5. **AGPL**：FPK 内含 LICENSE 与源码地址；对外分发时遵守 AGPL。

## 4. 审计结论

- 可复用性最高的路径：**保留 core/MiPlaySdk 协议栈，只加"外部 PCM 输出"钩子**，
  fnOS 宿主进程实现统一音频引擎、DLNA 拉流解码、Web 管理与 FPK。
- 不采用：重写协议、复制一套 Rust 核心、Docker、依赖 NAS 额外安装运行组件。
