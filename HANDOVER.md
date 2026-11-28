# FusionPlay 交接文档

> 更新：2026-11-28
> 活跃开发分支：`integration/unified-platforms` @ `195284c`（`Prepare FusionPlay 1.7.15`）
> 最新正式版：`v1.7.15`（2026-11-22）
> 先读 `AGENTS.md` 与 `docs/CROSS_PLATFORM_MAINTENANCE.md`，它们是维护流程的权威说明。

---

## 1. 项目是什么

FusionPlay 是局域网音乐投放接收器，支持 **AirPlay 2 / DLNA / 小米妙播（MiPlay）**，
一套共享协议与界面适配三个宿主：

| 平台 | 产物 | 入口 |
|---|---|---|
| Android | APK（arm64，最低 API 21） | `flutter/android/app` |
| Windows 10/11 | Setup EXE + 便携 ZIP（x64） | `installer/`、`windows/backend/` |
| 飞牛 fnOS | FPK（x64 / ARM64） | `fnos/` |

共享核心：

- `src/AirPlayReceiver.Core`：AirPlay、DLNA 控制面、音频与 JNI
- `src/FusionPlay.MiPlaySdk`：MiPlay 协议与媒体接收
- `vendor/shairplay`：带本地修复的 AirPlay 实现
- `src/shared_audio`、`src/shared_update`、`src/shared_logging`：跨宿主共享逻辑
- `flutter/lib`：共享 Flutter UI / 状态模型 / 宿主适配

---

## 2. 当前状态（重要）

### 2.1 分支关系

- **`integration/unified-platforms`**：应用与统一发版的活跃分支，当前 `195284c` / `VERSION 1.7.15`。
- **`main`**：主要承载 Android Pages 的更新清单同步提交（`android: sync update manifest ... [skip ci]`）；
  它可能落后于 integration，**不要以 main 的 VERSION 作为最新版本依据**。
- 远端仍有 `audit/liquid-glass-pr2`、`ui/ios18-frosted-glass`、`ui/player-control-spacing-cover-20261118` 等实验分支。
- PR #4（统一 Android/Windows/飞牛的共享实现与视觉风格）已合并。
- PR #2（横向封面 + premium 玻璃）已合并到其目标分支，但当前 integration 的实现已被 PR #4 的 Frosted Glass 方案取代。
- **PR #3 `audit/liquid-glass-pr2` 仍开着**：试图统一折射材质并修复 P01 合成污染；**未合并、未由本流程复验**。

### 2.2 最新正式版

- Tag：`v1.7.15`，commit `195284cef00e0caf385765d8efb49644704d8b3e`
- Release：<https://github.com/youyoudezhuzhu/FusionPlay/releases/tag/v1.7.15>
- 资产（9 个）：
  - `FusionPlay-Android-1.7.15.apk` + `.sha256`
  - `fusionplay-fnos.fpk`（x64）
  - `fusionplay-fnos-arm64.fpk`
  - `FusionPlay-Setup-1.7.15-x64.exe` + `.sha256`
  - `FusionPlay-Windows-1.7.15-x64.zip` + `.sha256`
  - `update-manifest.json`
- Android `versionCode`：`10715`
- 1.7.15 内容：修复 DLNA 播放中因控制请求切换网络地址而被拒绝的进度跳转；三端共用；菜单项收敛。

### 2.3 当前 UI 实现

- 播放页仍是 21:9 双栏：左侧播放器（封面/标题/进度/控制/音量），右侧沉浸式歌词；无歌词时左侧大封面 + 右侧控制。
- 顶部只显示协议（`AirPlay 2` / `DLNA` / `小米妙播`），不显示音质/码率。
- 歌曲名下方只保留歌手；「作曲」、音源/专辑行、无歌词提示行已移除。
- 封面使用 `FloatingAlbumArt`：宽范围柔光扩散 + 细边缘光，无黑色厚底座。
- 播放键为白色实心圆 + 深色图标；上一曲/下一曲为玻璃圆钮。
- **当前玻璃实现是 `flutter/lib/ui/frosted_glass.dart`（iOS 18 风格磨砂）**，配合 `glass_appearance.dart` 和 `FrostedScene`；
  integration 的 `pubspec.yaml` **不依赖 `liquid_glass_widgets`**，`flutter/lib/ui/liquid_glass.dart` 已删除。
  `flutter/third_party/liquid_glass_widgets` 目录可能仍有残留；不要在没有验证的情况下重新引入。
- 有歌词/无歌词、菜单、设置、弹窗、错误 toast 的截图与验收材料在 `fusionplay-ui-review/`、`fusionplay-ui-review-round2/`。

### 2.4 已知问题与验证缺口

| 项 | 状态 |
|---|---|
| Android Impeller 真机玻璃/性能 | 未验证；所有 UI 截图来自 `flutter_test`（Linux/Skia）或桌面 |
| 折射液态玻璃 | 当前正式版用磨砂，不是折射；PR #3 的折射方案未合并 |
| 飞牛 ARM64 DLNA 快放/封面/暂停 | Linux 测试覆盖 8 实例、暂停隔离、同曲重播、MP3 断流续传；真机待复测 |
| 妙播声音发颤 | v1.7.10 已修（锁竞争插入静音）；实体音箱待复测 |
| AirPlay RTSP type 96 缺共享密钥 | 历史握手失败，保留发送端/会话日志待复现 |
| 仓库 Fork 状态 | GitHub 仍显示 `fork: true`；README 已标注来源 |
| UI review zip | 仓库根目录的 `fusionplay-ui-review*.zip` 是未跟踪产物，可删除或移到 Release |

更完整的缺口与复测方法见 `docs/CROSS_PLATFORM_MAINTENANCE.md` 的“已知验证缺口”。

---

## 3. 目录与关键文件

| 路径 | 职责 |
|---|---|
| `flutter/lib/ui/player_view.dart` | 播放页布局、控制、歌词接入、玻璃控件 |
| `flutter/lib/ui/floating_album_art.dart` | 悬浮封面阴影/边缘光 |
| `flutter/lib/ui/frosted_glass.dart` | 当前正式版磨砂玻璃（`GlassLevel` / `FrostedGlass`） |
| `flutter/lib/ui/glass_appearance.dart` | 玻璃/不透明外观偏好与 `FrostedScene` |
| `flutter/lib/ui/lyrics_view.dart` | 完整时间轴歌词、逐字、滚动 |
| `flutter/lib/ui/app_menu.dart` | 三点菜单 / 设置 morph |
| `flutter/lib/ui/settings_view.dart` | 设置内容 |
| `flutter/lib/ui/update_dialogs.dart` | 更新/关于/错误弹窗 |
| `flutter/lib/main.dart` | 宿主、场景、全局浮层 |
| `flutter/lib/backend/` | Android / Windows / fnOS 控制器 |
| `windows/backend/` | Windows Rust 侧车（NDJSON） |
| `fnos/backend/` | 飞牛 Rust HTTP 宿主 |
| `fnos/package/` | FPK 清单与生命周期脚本 |
| `.github/workflows/release.yml` | 统一 `vX.Y.Z` 发版（四安装包） |
| `.github/workflows/android.yml` | Android APK 构建/签名 |
| `.github/workflows/android-pages.yml` | Android 更新清单与 Pages |
| `.github/workflows/shared-platform-checks.yml` | 跨平台 PR 检查 |
| `docs/ANDROID_RELEASE_SIGNING.md` | Android 签名密钥与 secrets |
| `docs/DIAGNOSTICS.md` | 日志与诊断 |

---

## 4. 本地环境与常规验证

CI 使用 Flutter 3.32.8；Linux Rust 音频构建需要 ALSA 开发库与 `pkg-config`。

```sh
# 运行时/UI
cd flutter
flutter pub get --enforce-lockfile
flutter analyze --no-pub
flutter test --no-pub

# Rust
cargo test --locked --manifest-path src/AirPlayReceiver.Core/Cargo.toml --all-targets
cargo test --locked --manifest-path src/FusionPlay.MiPlaySdk/Cargo.toml --lib
cargo test --locked --manifest-path fnos/backend/Cargo.toml --all-targets
cargo test --locked --manifest-path windows/backend/Cargo.toml --all-targets

# Web 主题同步
python3 tools/sync_web_theme.py --check

# 飞牛 Web 交互（需 Playwright）
CHROMIUM=/usr/bin/chromium python3 fnos/tools/test-curve-page.py
```

Flutter Web（飞牛播放器）：

```sh
cd flutter
flutter build web --release --no-pub --no-web-resources-cdn --pwa-strategy=none -t lib/backend/fnos/player.dart
```

---

## 5. 发版流程（统一 `vX.Y.Z`）

### 5.1 版本号同步清单

1. 根 `VERSION`
2. `INSTALLER_VERSION`
3. `flutter/pubspec.yaml`（`version: X.Y.Z+<versionCode>`）
4. `flutter/lib/ui/settings_view.dart` 默认版本
5. 飞牛 `manifest`、飞牛与 Windows 的 `Cargo.toml` / `Cargo.lock`
6. `docs/update/android/notes.md`
7. Android versionCode = `major*10000 + minor*100 + patch`；Windows 文件版本为四段
8. 提交并推送到 `integration/unified-platforms`
9. 从同一提交打单个 `vX.Y.Z` 标签并推送

```sh
git tag v1.7.16
git push origin v1.7.16
```

`release.yml` 会构建：Android ARM64 APK、Windows x64 EXE、Windows 便携 ZIP、飞牛 x64 FPK、飞牛 ARM64 FPK；
全部齐全后发布到同一个 Release。

### 5.2 发布后核对

1. Android Pages 工作流成功；
2. 对比 Release 的 `update-manifest.json`、Pages `stable.json`、raw `updates/android/stable.json`；
3. 核对版本码、`source_commit`、APK 大小、SHA-256、签名指纹；
4. 实际下载一次 Pages APK；
5. 核对两个 FPK 的架构/版本、Windows 资产完整。

Pages 失败可重试：

```sh
gh workflow run android-pages.yml --ref main -f tag=vX.Y.Z
```

### 5.3 Android 签名

- keystore **不在仓库内**，本机备份：
  `/vol1/@appdata/deepseek.harness/home/github/artifacts/keys/fusionplay-release.jks`
  （同目录有密码文件；不要提交/外传）
- 别名 `fusionplay`，RSA 4096，证书 SHA-256：
  `48:36:FB:96:C2:EF:76:FD:81:AC:38:54:55:F1:B0:15:4A:F7:7F:AD:AE:1B:45:BB:B7:52:B1:92:49:9D:C1:2F`
- GitHub Secrets：`ANDROID_KEYSTORE_BASE64`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、
  `ANDROID_KEY_PASSWORD`、`ANDROID_CERT_SHA256`
- 未配置 secrets 时会回退 debug 签名，**不能作为正式升级包**

---

## 6. 本地 Android 构建踩坑（Linux）

```bash
export ANDROID_HOME=/opt/android-sdk
export ANDROID_SDK_ROOT=/opt/android-sdk
export ANDROID_NDK_HOME=/opt/android-sdk/ndk/28.2.13676358
export CARGO_HOME="$HOME/.cargo"
export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu
export PATH="$HOME/.cargo/bin:$PATH"
export FUSIONPLAY_ABIS=arm64-v8a
```

- `flutter/android/local.properties` 需要 `sdk.dir=/opt/android-sdk`
- `src/AirPlayReceiver.Core/rust-toolchain.toml` 指向 Windows host，Linux 必须用 `RUSTUP_TOOLCHAIN` 覆盖，
  否则报 `stable-x86_64-pc-windows-gnullvm`
- 需要 `cargo-ndk`（`cargo install cargo-ndk`）
- 容器 FS 上 Gradle 文件监听会 NPE：`./gradlew assembleRelease --no-watch-fs`，
  或临时 `org.gradle.vfs.watch=false`
- 直接 `flutter build apk --release` 失败时，可进 `flutter/android` 用 `./gradlew assembleRelease --no-watch-fs`
- 产物：`flutter/build/app/outputs/flutter-apk/app-release.apk`

---

## 7. 飞牛 / Windows 特别注意

### 飞牛

- 桌面入口进入共享播放器；右上三点菜单为共享设置/诊断/更新，右下角飞牛菜单进入音箱切换/新建实例。
- `fnos/frontend/index.html` 只兼容旧书签；FPK 构建有白名单，`check-fpk.sh` 会拒绝旧后台资源。
- 主题值改 `flutter/lib/theme/fusion_theme.dart` 后运行 `python3 tools/sync_web_theme.py`。
- 排查升级：先看包架构、`/healthz`、`app.log`、`fusionplay.log`，用同架构覆盖安装测试。
- 两个 `backend/src/dlna_player.rs`（Windows/飞牛）需分别回归。

### Windows

- 原生输出/安装器/COM/WASAPI 改动必须在 Windows runner 验证，Linux 只能做静态检查。
- 侧车接口见 `docs/WINDOWS_ARCHITECTURE.md`。

---

## 8. 交叉验证清单（改共享协议/播放器时）

1. 共享事件定义 → 三宿主状态投影 → Flutter 解析 → 界面逐层核查；
2. 同曲歌词更新、真实换曲、暂停恢复、新推送抢占旧音源；
3. 歌词更新保留进度；换曲清理旧封面/歌词；旧音源迟到事件不能覆盖新音源；
4. 先核对发送端真实字段（AirPlay 可能把歌词放标题；DLNA 取决于 DIDL/资源）；
5. Android 结构化歌词/歌词 URI 支持范围比桌面宿主更广。

---

## 9. 已知陷阱

1. **不要以 main 的 VERSION 判断最新版**：main 可能只同步了 manifest；以 tag/Release 和 integration 分支为准。
2. **版本 tag 必须与 `VERSION` 完全一致**，`android_check_tag.py` 会拒绝不匹配。
3. **`main` 上的 Pages 清单提交会让 main 与 integration 分叉**；推送 release tag 后先核对。
4. **PR #3 未合并**：它基于 PR #2、改动大（含 audit 资产）；引用其结论前必须独立复验。
5. **UI 验收必须在真机 Impeller 上做玻璃/性能结论**；`flutter_test` 是 Skia。
6. **不要提交 keystore/密码/日志中的签名媒体 URL**。
7. `flutter/third_party/liquid_glass_widgets` 是历史残留；当前 pubspec 不依赖，重新启用需完整验证。
8. 仓库 `fork: true` 状态未处理；改 README/来源说明前先确认上游关系。

---

## 10. 建议下一步

1. 用真机（Android 手机 + Android TV）复验 v1.7.15 播放页/菜单/设置/弹窗，记录设备、GPU、Impeller 状态。
2. 评审 PR #3：跑与 `fusionplay-ui-review-round2` 相同的 P01–P20、玻璃动态、349/350/460/461 跳变；
   通过后再决定是否替换当前 Frosted Glass。
3. 规划 v1.7.16 时走统一 `vX.Y.Z` 标签，避免再出现分平台 `android-v*` 标签。
4. 清理根目录 review zip，或统一归档到 Release。
5. 复核飞牛 ARM64 与妙播实体设备两项已知缺口。

---

## 11. 快速命令

```bash
cd /vol1/@appdata/deepseek.harness/home/FusionPlay
git fetch --tags origin
git status -sb
git log --oneline -10

# 测试
cd flutter
flutter pub get --enforce-lockfile
flutter analyze --no-pub
flutter test --no-pub

# 发版后检查
git rev-parse v1.7.15
```

GitHub 认证：token 位于 `/root/.gh_token`；使用 `GIT_ASKPASS` 临时脚本，不要写进 remote URL 或命令行。
