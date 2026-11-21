# FusionPlay 维护与交接

## 从哪里接手

截至 2026-11-21，应用开发和统一发版使用 `integration/unified-platforms`。[PR #4](https://github.com/youyoudezhuzhu/FusionPlay/pull/4) 仍开着。最新正式版 `v1.7.10` 对应 `faa721bb01b1710d3240642aac085a4db5f5f114`，Android `versionCode` 为 `10710`。开始工作时先执行 `git status -sb` 和 `git fetch --tags origin`，对照远端分支、当前 Release 和 PR，保留工作区已有改动。应用变更以统一分支为基础。Android 的 Pages 工作流从 `main` 同步公开更新清单和 raw 备用清单。[正式发布](https://github.com/youyoudezhuzhu/FusionPlay/releases/tag/v1.7.10) · [发布构建](https://github.com/youyoudezhuzhu/FusionPlay/actions/runs/44880186363) · [Pages 部署](https://github.com/youyoudezhuzhu/FusionPlay/actions/runs/44880891363)

## 源码地图

| 层 | 入口与职责 |
| --- | --- |
| AirPlay / DLNA 控制面 | [AirPlayReceiver.Core](../src/AirPlayReceiver.Core)、[shairplay](../vendor/shairplay)。三端共享事件定义、AirPlay 接收和 DLNA AVTransport 控制面。 |
| 妙播 | [FusionPlay.MiPlaySdk](../src/FusionPlay.MiPlaySdk)。Windows 和飞牛通过同一个宿主 PCM 输出线程接收数据，Android 使用专用音频回调。 |
| 播放及设置界面 | [PlayerView](../flutter/lib/ui/player_view.dart)、[SettingsView](../flutter/lib/ui/settings_view.dart)、[状态模型](../flutter/lib/models)、[主题](../flutter/lib/theme)、[着色器](../flutter/shaders)。播放器布局按可用空间决定，音源能力决定控件能否操作。有歌词的横屏布局把封面、进度和控制归在同一列。 |
| Android | [Flutter 入口](../flutter/lib/main.dart) → [MethodChannel 控制器](../flutter/lib/backend/android) → [Android 宿主](../flutter/android/app)。Gradle 从 Rust 源码构建 JNI 库。音频走系统输出，安装权限、下载及安装由 Android 宿主管理。 |
| Windows | 同一 Flutter 入口 → [Windows 控制器](../flutter/lib/backend/windows) → [Rust 侧车](../windows/backend) 的 NDJSON 接口 → WASAPI。安装器在 [installer](../installer)。 |
| 飞牛 | [Flutter Web 入口](../flutter/lib/backend/fnos/player.dart) → [Rust HTTP 宿主](../fnos/backend/src/http.rs) → 各实例状态或命令接口。音频走 ALSA，每个音箱独立绑定、应用曲线和校准。安装生命周期在 [FPK 脚本](../fnos/package/cmd)。 |
| 共享工具逻辑 | [双声道转换](../src/shared_audio/stereo.rs)、[更新资产选择](../src/shared_update/mod.rs)、[日志轮转及脱敏](../src/shared_logging/mod.rs)。 |

协议新增字段后，沿共享事件定义、三个宿主的状态投影、Flutter 解析和界面逐层核查。重放同曲歌词更新、真实换曲、暂停恢复、新推送抢占旧音源这几种情况。验证歌词更新保留进度，换曲清理旧封面和歌词，旧音源的迟到事件无法覆盖当前音源。先核对发送端真实提供的字段。AirPlay 有发送端把歌词放在标题字段，DLNA 歌词取决于 DIDL 元数据或发送端提供的歌词资源。当前 Android 对结构化歌词和歌词 URI 的处理范围比桌面宿主更广。[AirPlay 歌词](ANDROID_LYRICS.md) · [DLNA 兼容性](DLNA_COMPAT.md) · [Windows 接口](WINDOWS_ARCHITECTURE.md)

Windows 与飞牛各有一个 `backend/src/dlna_player.rs`。媒体下载、HTTP Range 和解码修复需要检查两份，分别回归。飞牛已实现 PCM 节拍、提前断开后一次受限续传和曲末状态回传。妙播的宿主队列锁及节拍修复位于共享 SDK，覆盖 Windows 和飞牛两种架构。

## 飞牛专项

- 桌面入口进入共享播放器。右上角三点菜单打开共享设置、诊断和更新，右下角飞牛菜单进入音箱切换、新建实例。音箱页负责绑定、校准和曲线。独立实例使用固定 ID，改名或重绑后仍可从原播放地址进入。检查实例隔离时应同时比对音频、封面、歌词和控制请求。
- `fnos/frontend/index.html` 仅负责兼容旧书签。FPK 构建列出允许复制的 Web 文件，`check-fpk.sh` 会拒绝旧后台资源。共享主题值在 `flutter/lib/theme/fusion_theme.dart` 修改，运行 `python3 tools/sync_web_theme.py` 更新 CSS。
- 排查启动或升级时，先看包架构、`/healthz`、`app.log` 和 `fusionplay.log`，比对覆盖安装前后的配置和实例。使用同架构升级测试。FPK 安装、服务状态、音频组权限、真实声卡和数据目录详见安装文档。[界面入口](../fnos/UI-STRUCTURE.md) · [飞牛安装与升级](../fnos/INSTALL.md) · [验证记录](../fnos/VERIFICATION.md)

## 本地验证

CI 使用 Flutter 3.32.8。Linux Rust 音频构建需要 ALSA 开发库及 `pkg-config`。Android 发布以 Android 工作流中的 SDK、NDK、JDK 和签名设置为准。Windows 原生输出、安装器与 COM/WASAPI 相关改动应在 Windows runner 上验证。

从仓库根目录按改动范围执行以下命令。

```sh
cargo test --locked --manifest-path src/AirPlayReceiver.Core/Cargo.toml --all-targets
cargo test --locked --manifest-path src/FusionPlay.MiPlaySdk/Cargo.toml --lib
cargo test --locked --manifest-path fnos/backend/Cargo.toml --all-targets
cargo test --locked --manifest-path windows/backend/Cargo.toml --all-targets
python3 tools/sync_web_theme.py --check
(cd flutter && flutter pub get --enforce-lockfile && flutter analyze --no-pub && flutter test --no-pub)
```

- 飞牛浏览器交互回归用 `CHROMIUM=/usr/bin/chromium python3 fnos/tools/test-curve-page.py`，需要 Python Playwright。并发投送使用 `fnos/tools/test-speaker-instances.py`，支持指定接收器数量及 MP3 断流对照。完整命令见飞牛验证记录。
- Flutter Web 验证从 `flutter/` 运行 `flutter build web --release --no-pub --no-web-resources-cdn --pwa-strategy=none -t lib/backend/fnos/player.dart`。Android 渲染验收使用 Android 实机，浏览器截图用于核对 Web 端。
- PR 检查见 [shared-platform-checks.yml](../.github/workflows/shared-platform-checks.yml)。飞牛 CI 还会检查 FPK 两种架构，并模拟安装、启动、停止及同架构原地升级。使用 null 音频后端的结果只覆盖软件链路，实体声卡和真实客户端需要另测。

## 发版与在线更新

1. 发布前核对当前分支、目标提交、最近正式版和已有标签。同步修改根 `VERSION`、`INSTALLER_VERSION`、`flutter/pubspec.yaml`、`flutter/lib/ui/settings_view.dart` 的默认版本、飞牛 `manifest`、飞牛与 Windows 的 `Cargo.toml` 和 `Cargo.lock`。更新 `docs/update/android/notes.md`。Android 版本码按 `major × 10000 + minor × 100 + patch` 计算，Windows 文件版本为四段。
2. 从确认的统一分支提交推送单个 `vX.Y.Z` 标签。[统一发布工作流](../.github/workflows/release.yml) 构建 Android ARM64 APK、Windows x64 EXE 和便携 ZIP、飞牛 x64 FPK 和 ARM64 FPK。四个安装包齐全后发布同一份 Release。Android 正式包使用已有签名，签名密钥由 GitHub Actions Secrets 管理。
3. Release 发布后确认 [Android Pages 工作流](../.github/workflows/android-pages.yml) 成功。比较 Release 的 `update-manifest.json`、[Pages 清单](https://youyoudezhuzhu.github.io/FusionPlay/update/android/stable.json) 与 [raw 清单](https://raw.githubusercontent.com/youyoudezhuzhu/FusionPlay/main/updates/android/stable.json)。核对版本码、`source_commit`、APK 大小、SHA-256 和签名指纹，实际下载一次 Pages APK。Pages 失败可使用 `gh workflow run android-pages.yml --ref main -f tag=vX.Y.Z` 重试同步。
4. 核对公开 Release 的资产完整性、两个 FPK 的版本和架构。Windows 让用户选择目标目录下载，飞牛通过 NAS 文件夹授权保存对应架构 FPK，之后由用户安装。旧版 Windows 或飞牛如果只识别旧的分平台标签，应从统一 Release 手动覆盖升级一次。发版记录包含版本标签、提交、Release、Pages 和真机验收范围。

## 已知验证缺口

| 待确认 | 当前证据与复测目标 |
| --- | --- |
| 飞牛 ARM64 DLNA 快放、封面、暂停 | Linux 测试已覆盖 8 实例、暂停隔离、同曲重播和 MP3 断流续传。真机用 QQ 音乐、网易云和 BubbleUPnP 复测，从 AirPlay 切到 DLNA 也要核对曲名、封面和音频归属。 |
| 妙播声音发颤 | 旧版在锁竞争时插入 10 ms 静音，回归能复现这一点，修复已进入 `v1.7.10`。实体音箱上复测持续音和切歌，记录发生时间并核对 `妙播输出统计` 中的缓冲不足、节拍延迟和丢帧。Windows 与飞牛使用同一个 SDK 修复。 |
| AirPlay RTSP type 96 缺共享密钥 | 旧日志中出现过该握手失败。保留发送端型号、应用、复现时间及对应会话日志，便于和 DLNA HTTP 错误分开诊断。 |
| 仓库 Fork 状态 | GitHub 查询仍为 `fork: true`。README 已按要求标注来源 `rosienosiesie/FusionPlay-Android`。如重新处理，先核对 GitHub 的脱离流程、可保留的设置和 PR。 |

此前的 UI 验收资料仍在 [第一轮](../fusionplay-ui-review)、[第二轮](../fusionplay-ui-review-round2) 和 [专项审查](../fusionplay-ui-audit-codex)。排查外部反馈时记录安装版本、平台架构、发送端应用、音频输出和事件时间。分享日志前清除签名媒体 URL、令牌等敏感字段。[诊断文档](DIAGNOSTICS.md)
