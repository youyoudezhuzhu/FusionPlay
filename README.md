# FusionPlay

FusionPlay 是一个面向全平台的局域网音乐投放接收器，支持 **AirPlay 2 / DLNA / 小米妙播（MiPlay）**。

同一套协议栈与界面已适配三个宿主平台：

| 平台 | 形态 | 说明 |
|---|---|---|
| Android | APK | 最低 Android 5.0（API 21），对 Android TV 做了遥控器适配 |
| fnOS（飞牛 OS） | FPK 应用 | 共享 Flutter 播放器 + 独立音箱管理，支持多音箱输出方案 |
| Windows 10/11 | 桌面应用 | Flutter Windows 界面 + Rust 协议侧车，支持开机自启、防火墙规则安装包 |

## 目录结构

```text
flutter/
  lib/ui/, lib/theme/   Android 与 Windows 共用的播放器、歌词、设置和视觉参数
  lib/models/           两端共用的界面状态
  lib/backend/android/  Kotlin MethodChannel 适配器
  lib/backend/windows/  Rust 侧车 NDJSON 适配器
  android/app/          Android 宿主、Kotlin 后端和 Rust 构建任务
  windows/              Windows 宿主工程（CMake/runner）
  test/                 Flutter 与界面交互测试
src/
  shared_audio/         Windows 和飞牛共用的 DLNA 声道混合、重采样与单测
  AirPlayReceiver.Core/ AirPlay、DLNA、音频和 JNI 核心
  FusionPlay.MiPlaySdk/ 小米妙播协议与媒体接收
  FusionPlay.AndroidIfAddrs/ Android 5 网络接口适配
windows/
  backend/              Windows 协议侧车 FusionPlayCore.exe（AirPlay 2 / DLNA / MiPlay）
fnos/                   fnOS FPK 宿主（Rust 后端 + Web 前端 + 打包脚本）
installer/              Windows Inno Setup 安装包脚本
vendor/shairplay/       带本地修复的上游 AirPlay 实现
tools/                  图标、抓包和诊断辅助工具
docs/                   平台与协议说明
```

三端在同一分支维护。播放协议改动集中在 `src/`。Android 和 Windows 通过各自的控制器向同一个 `FusionPlayHome` 提供状态。飞牛通过 Flutter Web 使用同一个 `PlayerView`、歌词布局、封面背景和菜单动效。NAS 专有的音箱分配、校准和接收管理集中在右上角的「设置」。飞牛播放器右下角的飞牛按钮提供「切换音箱」、「音箱音量曲线」和「新建实例」，每个实例的播放地址可单独收藏或在新标签页打开。播放器的 HTTP 适配器位于 `flutter/lib/backend/fnos/`。

调整共享颜色、圆角或模糊半径时，编辑 `flutter/lib/theme/fusion_theme.dart`，运行 `python3 tools/sync_web_theme.py` 并提交生成的样式。飞牛构建使用 `--check` 验证同步。播放器从当前媒体的封面生成背景，缺少封面时使用中性底色。

界面预览：[共享 Flutter 播放器](docs/platform-previews/flutter-player.png) · [飞牛默认播放器](docs/platform-previews/fnos-player.png) · [飞牛歌词布局](docs/platform-previews/fnos-player-lyrics.png) · [输出方案](docs/platform-previews/fnos-output-groups.png) · [飞牛输出设备](docs/platform-previews/fusionplay-fnos-output.png)。截图使用测试曲目信息和模拟 USB 设备。飞牛使用真实配置接口保存输出延迟，音频后端为 null。

Android 侧只有 `flutter/android/app` 一个应用模块：Rust 动态库由该模块的 `buildRustNative` 任务生成到 Gradle 构建目录，不会写入源码树。Windows 侧由 `windows/backend` 生成独立侧车可执行文件，与 Flutter 宿主通过 NDJSON 通信，接口见 [docs/WINDOWS_ARCHITECTURE.md](docs/WINDOWS_ARCHITECTURE.md)。

## 构建环境

- Windows 10/11 与 PowerShell 7（Android / Windows 桌面构建）；Linux 用于 fnOS 构建与 CI。
- Flutter 3.32.8、Dart 3.8.1。
- Android SDK 36、Android NDK 28.2.13676358、JDK 21（仅 Android）。
- Rust 工具链：Android 以 `src/AirPlayReceiver.Core/rust-toolchain.toml` 为准；Windows 侧车使用 stable MSVC 工具链。
- `cargo-ndk` 可执行文件位于 `CARGO_HOME/bin` 或 `PATH`（仅 Android）。

标准环境变量为 `FLUTTER_ROOT`、`ANDROID_HOME`（或 `ANDROID_SDK_ROOT`）、`JAVA_HOME` 与 `CARGO_HOME`。如使用集中工具目录，也可以设置 `FUSIONPLAY_TOOLS_ROOT`；构建脚本会将缓存放入该目录并自动处理包含非 ASCII 字符的工程路径。

## 检查与测试

```powershell
Set-Location .\flutter
flutter pub get --enforce-lockfile
flutter analyze --no-pub
flutter test --no-pub
Set-Location ..
```

Kotlin 单元测试可通过 Flutter Android 工程的 Gradle Wrapper 执行：

```powershell
.\flutter\android\gradlew.bat `
  -p .\flutter\android `
  :app:testReleaseUnitTest `
  --no-daemon
```

Rust 模块可分别运行 `cargo test` 和 `cargo clippy`。Android 原生库需要通过完整 APK 构建验证对应 NDK 目标；Windows 侧车在 `windows/backend` 下执行 `cargo test`，并用 `cargo check --target x86_64-pc-windows-msvc` 检查 Windows 专有代码。

## 构建 Android APK

```powershell
# 默认：只构建 arm64-v8a，安装包约 14 MB
.\flutter\tool\build_universal.ps1
# 需要兼容 32 位设备时：
.\flutter\tool\build_universal.ps1 -Abis armeabi-v7a
# 需要包含所有 ABI 的调试/兼容包：
.\flutter\tool\build_universal.ps1 -Abis all
```

脚本读取根目录 `VERSION`，计算 Android `versionCode`，按 `-Abis` 选择 ABI，并将 APK 与 Dart 混淆符号保存到 `artifacts/releases/`。该目录不进入版本控制。默认单 ABI 是为了避免把三份 Flutter engine 和 Rust core 打进安装包；32 位设备请用 `-Abis armeabi-v7a` 单独构建。

当前 Release 配置在没有私有签名配置时使用 Android 调试签名，公开发布商店版本前应替换为自己的安全签名流程，且不得提交密钥文件。

## 构建 Windows 桌面版

在 Windows 10/11 + PowerShell 7 上：

```powershell
.\flutter\tool\build_windows.ps1
```

脚本会编译 `windows/backend` 侧车、执行 `flutter build windows --release`、把 `FusionPlayCore.exe` 放进发布目录，输出 `dist/FusionPlay-Windows-<版本>-x64.zip` 与 SHA256；如本机安装了 Inno Setup，还会生成 `dist/FusionPlay-Setup-<版本>-x64.exe` 安装包（含私有/域网络防火墙放行规则）。

CI 上 `.github/workflows/windows.yml` 在 `windows-latest` 上完成同样的构建，并执行侧车 NDJSON 冒烟测试（启动 → `ready`/`receiverReady` → `shutdown` 退出 0）。发布流程：打 `windows-v*` tag 触发 Release。

Windows 版的安装、使用、端口与已知限制见 [docs/WINDOWS.md](docs/WINDOWS.md)，协议实现细节见 [docs/WINDOWS_ARCHITECTURE.md](docs/WINDOWS_ARCHITECTURE.md)。

Windows 版的数据目录为 `%LOCALAPPDATA%\FusionPlay`（设置、日志、封面缓存、MiPlay 身份），卸载应用不会删除这些用户数据。安装后首次运行会弹出 Windows 防火墙授权（安装包会预先添加放行规则）；建议只允许私有/域网络。

## 构建 fnOS FPK

见 [fnos/README.md](fnos/README.md) 与 [fnos/INSTALL.md](fnos/INSTALL.md)；CI 由 `.github/workflows/build-fnos.yml` 在 `fnos-*` tag 上出包。

## 协议与商标说明

本项目仅为网络互操作实现，不包含或调用小米、Apple 或其他厂商的私有运行库、账号服务或设备证书。AirPlay、Apple、小米、妙播、DLNA 等名称及商标归各自权利人所有，本项目与这些厂商没有隶属或官方认证关系。

## 许可证

FusionPlay 原创源码使用 [GNU Affero General Public License v3.0](LICENSE)，SPDX 标识为 `AGPL-3.0-only`。项目内第三方组件继续适用各自许可证，详情见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 与对应依赖源码或元数据。

## Android Release 签名

发布 APK 使用固定 release keystore，保证后续版本可覆盖安装；详见 [docs/ANDROID_RELEASE_SIGNING.md](docs/ANDROID_RELEASE_SIGNING.md)。

## 妙播改名后的旧路由

改名后若电视/设备中心仍显示两个实例，见 [docs/MIPLAY_DUPLICATE_ROUTES.md](docs/MIPLAY_DUPLICATE_ROUTES.md)（1.2.8/0.2.5 起自动清理历史名称路由）。
