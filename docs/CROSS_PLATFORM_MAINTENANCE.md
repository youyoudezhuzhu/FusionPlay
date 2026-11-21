# FusionPlay 三端共用范围与维护约定

2026-11-21 对照了 `integration/unified-platforms` 的三个应用入口、宿主事件处理、安装包构建脚本和线上发布记录。协议与播放器已有共同源码。能否同步生效，取决于修复所在的层、每个宿主是否传递相应字段，以及用户安装的包是否包含该提交。

| 改动位置 | 自动覆盖的范围 | 平台边界 |
| --- | --- | --- |
| `src/AirPlayReceiver.Core`、`vendor/shairplay`、`src/FusionPlay.MiPlaySdk` | Android、Windows、飞牛重新构建时都会编入同一份 AirPlay、DLNA 控制面和妙播协议实现。例子是 `11fdc62` 对首包音频之前到达的 AirPlay 元数据的修复。 | Android 通过 Gradle 构建 JNI 库。Windows 和飞牛通过各自的 Cargo 工程引用同一源码。DLNA 媒体下载、解码和播放控制仍经过宿主。 |
| `flutter/lib/ui/player_view.dart`、`flutter/lib/models`、`flutter/lib/theme`、`flutter/shaders` | 三端编译同一个播放组件，复用封面、布局、进度条、歌词和玻璃视觉逻辑。例如 `ea5f03d` 的玻璃边缘修改。 | Android、Windows 从 `flutter/lib/main.dart` 进入。飞牛从 `flutter/lib/backend/fnos/player.dart` 进入，按音箱实例经 HTTP 读取状态。Android 图形后端、Windows 桌面和浏览器的着色器能力、实机效果仍需分别验证。 |
| 宿主事件 → `PlaybackSnapshot` | 共用 Flutter 状态模型、歌词显示和解析入口。Windows 与飞牛还共用妙播分包的曲目判别规则。 | Android 用 Kotlin/MethodChannel，Windows 用 Rust/NDJSON，飞牛用每实例状态/HTTP。新增协议字段时，要检查三条传递路径。Android 另有结构化歌词、歌词 URI 与同名 LRC 读取路径，桌面与飞牛这里验证的是内嵌歌词。 |
| 音频设备与会话 | Windows 和飞牛共用 `src/shared_audio/stereo.rs` 的 DLNA 混音和重采样。共享内核处理 AirPlay 的音量命令和事件。 | Android 把音量映射到系统媒体音量，Windows 输出到 WASAPI，飞牛输出到绑定的 ALSA 设备并应用该音箱的曲线。抢占、输出生命周期及各宿主的元数据缓存仍分开维护。 |
| 飞牛管理界面 | 默认播放页复用共享组件。独立管理界面的一部分颜色、圆角和模糊参数来自 `tools/sync_web_theme.py`。 | 实例增删改名、音箱绑定和校准在飞牛目录维护。播放器附加入口可打开这些管理功能。独立管理页使用 HTML/JS，播放页的 Flutter 组件改动不会自动重写这些页面。 |

**最容易漏同步的是桌面 DLNA。** 在 `62e7a5b` 比较 `windows/backend/src/dlna_player.rs` 和 `fnos/backend/src/dlna_player.rs`，按原始文本逐行查找连续至少 10 行的相同片段，合计 1,683 行。两端分别实现了 HTTP 打开、Range 响应解析和媒体解码。以后修复这几个环节，须检查另一宿主并运行两端的 DLNA 回归。可优先将 HTTP 读取和范围解析收进共享 Rust 模块，保留各自的设备、会话与输出回调。

## 更新与分发

| 平台 | 版本来源 | 用户取得更新的实际路径 |
| --- | --- | --- |
| Android | 根目录 `VERSION` 推导显示版本与 `versionCode` | `android-v*` 标签发布签名 APK。Pages 工作流验证哈希及大小，部署 `update/android/stable.json` 和 APK，并维护 raw 备用清单。应用内检查、下载进度、安装权限和安装逻辑在 Android 适配器。 |
| Windows | 根目录 `VERSION` 和 `INSTALLER_VERSION` | `windows-v*` 标签发布 ZIP 和 Setup.exe。Flutter 运行时入口没有启用 Android 的更新页。 |
| 飞牛 | `fnos/package/manifest` 与 `fnos/backend/Cargo.toml`，打包检查会对照两者 | `fnos-*` 标签发布 FPK，经飞牛应用管理安装或升级。 |

核对时三个平台各自最新的非预发行 Release 为 Android `1.7.8`、Windows `1.3.2` 和飞牛 `0.2.5`。Android 线上清单的源码提交为 `f6364a30f2342b5a500bd23111b03e45e6b0a5ba`。合入共享修复后，各平台仍须从含有该修复的提交构建并发布。平台标签和用户端更新渠道目前分别管理。[发布记录](https://github.com/youyoudezhuzhu/FusionPlay/releases) · [Android 线上清单](https://youyoudezhuzhu.github.io/FusionPlay/update/android/stable.json)

## 审查中修复的问题

- **Windows 歌词中断。** 共享内核和 SDK 已发出 AirPlay 的 `lyric_line`、DLNA 的 `lyrics_text`、妙播的 `lyrics`，Windows 侧车原先没有对应的输出字段。`5a9f777` 补齐三种事件到 Flutter 的路径。原飞牛的 LRC/妙播逐字解析移入共享模型，保留 Android 原有结构化歌词。回归覆盖同曲补发、同名不同 ID 换曲、歌词更新保持进度及清理旧歌词。
- **AirPlay 手动音量与发送端音量。** 共享内核曾将 Android 由系统控制音量的处理套用于桌面 PCM。已有无声卡集成测试复现：发送端给出 −6.02 dB，输出接口收到 1.0。`e03bccc` 让 Windows 和飞牛的输出收到约 0.5，应用内静音输出 0，全音量恢复 1。Android 继续由系统执行衰减。飞牛的滑块使用协议报告的百分比，独立于输入音箱曲线的线性增益。
- **Windows 版本不一致。** 安装包与文件属性读取根目录版本，侧车的命令行、启动握手及诊断原先使用自身 Cargo 包版本。`62e7a5b` 改为读取安装包使用的产品版本，回归断言直接对照根目录文件。

## 如何验证一次共享修改

拉取请求的 `.github/workflows/shared-platform-checks.yml` 针对相关路径运行只读检查。Linux 运行共用协议、飞牛宿主测试、Flutter 分析与测试，并编译飞牛实际使用的 Web 入口。Windows 运行侧车测试并编译 Flutter 桌面入口。Android 编译 arm64 调试 APK 并运行 Kotlin 单测。三端发版与 PR 检查统一通过 `.github/actions/setup-flutter/action.yml` 选择 Flutter SDK 版本。发版仍由各平台已有工作流负责。

本次在 Linux 本地复核了共享内核的外部 PCM 测试、Windows 侧车测试与命令行版本、飞牛宿主测试、Android 后端及更新模块的 JVM 单测，以及 Flutter 全量测试与飞牛 Web release 构建。飞牛真实 ALSA 设备测试需要连接声卡，桌面音量链路通过 PCM 接口测量。CI 每个宿主的具体结果以 PR 检查记录为准。

协议变更的最短回归路径是重放同一组事件，检查宿主面向 Flutter 输出的快照，再检查共用播放器。例如一条仅含歌词的妙播更新，应保留上一条事件给出的曲名和进度，只有真实换曲才清理旧字段。发布时对照每个平台标签对应的提交，Android 另核对 Pages 清单里的 `source_commit`。
