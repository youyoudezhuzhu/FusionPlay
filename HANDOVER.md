# FusionPlay 交接文档

> 更新时间：2026-11-28
> 当前主线：`main` @ `aec10e2`（VERSION `1.7.2`）
> 本文面向接手后续开发/验收的人，说明当前状态、发布流程、未完成事项和坑。

---

## 1. 项目概览

FusionPlay 是局域网音乐投放接收器，支持：

- AirPlay 2
- DLNA
- 小米妙播（MiPlay）

三个宿主：

| 平台 | 产物 | 说明 |
|---|---|---|
| Android | APK | 最低 API 21；Android TV 遥控器适配 |
| fnOS | FPK | 原生服务 + Web 管理页 |
| Windows | 桌面应用 | Flutter UI + Rust 侧车 |

主目录：

```text
flutter/                 Flutter UI / Android 宿主 / Windows 宿主
src/AirPlayReceiver.Core/        AirPlay、DLNA、音频、JNI 核心
src/FusionPlay.MiPlaySdk/        MiPlay 协议
windows/backend/                 Windows 协议侧车
fnos/                            fnOS FPK
vendor/shairplay/                AirPlay 实现（带本地修复）
docs/                            协议、平台、UI 文档
fusionplay-ui-review*/           UI 验收材料（第一轮/第二轮）
```

技术栈：Flutter 3.32.8 / Dart 3.8.1；Kotlin + Rust 核心；Windows C++ 宿主；CI 用 GitHub Actions。

---

## 2. 当前状态

### 2.1 已发布版本

- Android 最新正式版：**1.7.2 / versionCode 10702**
- Release：<https://github.com/youyoudezhuzhu/FusionPlay/releases/tag/android-v1.7.2>
- 签名：正式 FusionPlay release key
- 证书 SHA-256：`48:36:FB:96:C2:EF:76:FD:81:AC:38:54:55:F1:B0:15:4A:F7:7F:AD:AE:1B:45:BB:B7:52:B1:92:49:9D:C1:2F`
- 更新清单：`updates/android/stable.json`，Pages 同步由 `android-pages.yml` 完成
- 1.7.2 的定位：**兼容修复版**，播放器玻璃降级为 shader-free，优先保证控件可见可点

### 2.2 主线 UI 现状（`main`）

- 21:9 双栏：左侧播放器（封面/信息/进度/控制/音量），右侧沉浸式歌词
- 封面：独立 `FloatingAlbumArt`，柔光扩散阴影 + 细边缘光
- 播放键：白色实心圆 + 深色图标；上一曲/下一曲为玻璃圆钮
- 顶部：只显示协议（如 `AirPlay 2`），不显示音质格式
- 歌曲名下方只保留歌手；已删除「作曲」、音源/专辑行、无歌词提示行
- 播放页玻璃：`liquid_glass_widgets`（vendored `0.18.4`），`GlassQuality.standard`
- 菜单/设置/弹窗：项目自研 `LiquidGlass` 的 flat fallback（无厚白边）
- 无 FittedBox 整页缩放；无底部 Dock
- 功能保留：AirPlay/MiPlay/DLNA、封面、进度/Seek 能力、音量/静音、歌词（同步/逐字/流式/AirPlay 标题）、菜单、焦点、键盘/遥控器/触摸、reduce motion

### 2.3 未合并的 UI 分支/PR

| PR | 分支 | HEAD | 状态 | 说明 |
|---|---|---|---|---|
| #2 | `ui/player-control-spacing-cover-20261118` | `e0d82b0` | Draft | 短横屏横向封面布局 + premium 玻璃；第二轮验收 **P0 未通过** |
| #3 | `audit/liquid-glass-pr2` | `f6364a30` | Draft | 基于 PR2；声称修复 P01、统一折射材质、封面 161.8/176.8px、213 测试通过；**尚未由我们复验，Android Impeller 未验证** |

结论：**PR #2 不要合并**。PR #3 需要先跑第二轮相同验收（含 P01–P20、玻璃专项、真机）再决定。

### 2.4 已知 P0/P1（来自 `fusionplay-ui-review-round2/`）

- **P0-1**：PR2 下 900×400 有歌词出现整块模糊脏层（P01），900×400 无歌词/980×420/840×360 正常
- **P0-2**：Android Impeller 动态折射未验证；Skia 下 `liquid_glass_widgets` 走 lightweight 降级
- **P1-1**：PR2 高度 349↔350、460↔461 布局突然切换（纵向↔横向）
- **P1-2**：PR2 封面 P01 150.7px / P03 159.1px，低于目标 160 / 170–180
- **P1-3**：进度条到时间文字约 10px，略高于建议 4–8px
- **P1-4**：顶部 AirPlay 标签/歌词按钮发虚、白色糊块（PR2）
- **P1-5**：玻璃在 Skia 下偏平偏棕，缺少真实折射

主线 `main` 不含 PR2 的横向布局/premium 改动，因此上述 P0 主要在 PR2/PR3 上。

---

## 3. 发布流程（Android）

### 3.1 版本与 tag

- 根目录 `VERSION` 必须与 tag 一致：例如 `1.7.2` ↔ `android-v1.7.2`
- `versionCode` 规则：`major*10000 + minor*100 + patch`（1.7.2 → 10702）
- `flutter/pubspec.yaml` 的 `version:` 也要同步更新
- 推送 tag `android-v*` 会触发 `.github/workflows/android.yml`：
  - 构建 arm64 release APK
  - 用 secrets 中的 release keystore 签名
  - 校验证书 SHA-256
  - 创建/更新 GitHub Release（APK、sha256、update-manifest.json）
  - 触发 `android-pages.yml` 同步 `updates/android/stable.json`

### 3.2 签名

keystore **不在仓库内**，本地备份在：

```text
/vol1/@appdata/deepseek.harness/home/github/artifacts/keys/fusionplay-release.jks
```

同目录有密码文件；不要把 keystore/密码提交到 git、issue 或聊天。

GitHub Secrets 需要：

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`（`fusionplay`）
- `ANDROID_KEY_PASSWORD`
- `ANDROID_CERT_SHA256`

未配置时本地构建会回退 debug 签名，**不能作为正式升级包**。

### 3.3 本地 Android 构建的坑（本机已验证）

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
- 仓库 `src/AirPlayReceiver.Core/rust-toolchain.toml` 指向 Windows host 工具链，Linux 必须用 `RUSTUP_TOOLCHAIN` 覆盖
- 需要 `cargo-ndk`（`cargo install cargo-ndk`）
- Gradle 文件监听在容器 FS 上会 NPE：用 `./gradlew assembleRelease --no-watch-fs`，或在 `gradle.properties` 临时加 `org.gradle.vfs.watch=false`
- 直接 `flutter build apk --release` 若失败，可用 `flutter/android/gradlew assembleRelease --no-watch-fs`
- 构建成功产物：`flutter/build/app/outputs/flutter-apk/app-release.apk`
- 1.7.2 正式 APK SHA-256：`1bbe809eb5ff631b0447b323ac6f910b1e1fd5045759e43041f6de93c8227554`

---

## 4. 测试与验收

### 4.1 常规检查

```bash
cd flutter
flutter pub get
flutter analyze
flutter test
```

- 当前 UI 清理后全量测试：**207 passed**（提交 `1abbe54` 时）
- PR2 分支全量：209 passed
- `flutter analyze` 会因 vendored `liquid_glass_widgets` 的 18–21 条 info 以退出码 1 结束；**无 error/warning**
- 指定 UI 测试：

```bash
flutter test test/player_preview_test.dart test/lyrics_player_test.dart
```

### 4.2 验收资产

- 第一轮：`fusionplay-ui-review/REVIEW.md` + 截图 + `measurements.csv`
- 第二轮：`fusionplay-ui-review-round2/REVIEW.md` + `comparison.md` + `issues.md` + P01–P20/G01–G10 + `measurements.csv`
- Release 附件：
  - <https://github.com/youyoudezhuzhu/FusionPlay/releases/tag/ui-review-pr2>
  - <https://github.com/youyoudezhuzhu/FusionPlay/releases/tag/ui-review-pr2-round2>
- 复现脚本：`fusionplay-ui-review-round2/tools/ui_review_round2_test.dart`（复制到 `flutter/test/` 后运行）

### 4.3 截图复现注意

- 必须先铺背景 `ArtworkBackground`，`GlassBackdropScope` 只负责采样、不负责显示背景
- 第一张截图前要 warmup 一帧，否则可能出现未解码/脏层
- 截图 DPR=2，输出物理像素 = 逻辑×2
- 环境是 Linux `flutter_test`（Skia），**不是 Android Impeller**；任何折射/性能结论都必须标「未验证」

---

## 5. 关键文件地图

| 文件 | 作用 |
|---|---|
| `flutter/lib/ui/player_view.dart` | 播放页全部布局/控制/玻璃/歌词接入 |
| `flutter/lib/ui/floating_album_art.dart` | 悬浮封面阴影与边缘光 |
| `flutter/lib/ui/lyrics_view.dart` | 完整时间轴歌词、逐字歌词、滚动 |
| `flutter/lib/ui/liquid_glass.dart` | 自研折射玻璃 + `GlassDialog`；当前菜单/设置/弹窗用 fallback |
| `flutter/lib/ui/glass_backdrop.dart` | 背景纹理捕获 `GlassBackdropScope` |
| `flutter/lib/ui/app_menu.dart` | 菜单/设置 morph overlay |
| `flutter/lib/ui/settings_view.dart` | 设置内容 |
| `flutter/lib/ui/update_dialogs.dart` | 更新/关于/错误弹窗 |
| `flutter/lib/main.dart` | 宿主、背景、全局浮层、菜单/设置/错误 toast |
| `flutter/test/player_preview_test.dart` | 21:9/平板/TV/流式歌词截图测试 |
| `flutter/test/lyrics_player_test.dart` | 布局/焦点/歌词/无歌词/21:9 回归 |
| `flutter/test/app_menu_test.dart` | 菜单/设置 morph/焦点 |
| `flutter/test/ui_gallery_test.dart` | 菜单/设置/弹窗 21:9 预览 |
| `.github/workflows/android.yml` | 正式签名构建 + Release |
| `.github/workflows/android-pages.yml` | 更新清单/Pages 同步 |
| `docs/ANDROID_RELEASE_SIGNING.md` | 签名密钥与 secrets 说明 |

第三方玻璃：

- `flutter/third_party/liquid_glass_widgets/`（MIT，0.18.4）
- 兼容性补丁：移除 4 处 `filterQuality:`，`vector_math` 固定 2.3.0
- 许可证说明：根目录 `THIRD_PARTY_NOTICES.md`

---

## 6. 已知限制与坑

1. **Android Impeller 未验证**：所有 UI 截图来自 Skia widget test；`GlassQuality.premium` 在 Skia 下不是真实折射。
2. **PR2 P0 未修复**：900×400 有歌词模糊脏层；不要在未复验前合并。
3. **PR3 未复验**：声称修复 P01/封面尺寸，但基于 PR2 且改动 133 个文件（含大量 audit 资产），需要独立验收。
4. **签名密钥**：keystore 路径不在仓库；CI secrets 必须保留，否则无法覆盖升级。
5. **Rust 工具链**：Linux 本地构建必须覆盖 `RUSTUP_TOOLCHAIN`，否则报 `stable-x86_64-pc-windows-gnullvm` 错误。
6. **Gradle 文件监听**：容器内需要 `--no-watch-fs`。
7. **UI Review zip**：`fusionplay-ui-review.zip`、`fusionplay-ui-review-round2.zip` 目前在仓库根目录是未跟踪文件；如不需要可删除，或移到 Release。
8. **更新弹窗**：关于/错误 toast 已截图；更新检查/进度/确认弹窗未单独截图，需要模拟更新状态。
9. **性能/录屏**：无真机/录屏能力，未测帧率、GPU、Shader 编译卡顿。

---

## 7. 建议的下一步

1. 对 PR3 跑与第二轮相同的验收：P01–P20、`flutter analyze/test`、P0 脏层、封面尺寸、349/350/460/461 跳变。
2. 在 Android Impeller 真机（至少 Pixel/中高端机 + Android TV 各一台）验证：
   - 播放器三按钮/音量胶囊折射
   - 菜单→设置 morph
   - 错误气泡
   - 动态背景/网格对比与录屏
3. 若 PR3 通过，更新 `VERSION` 到 1.7.3，走 tag `android-v1.7.3` 的 Actions 正式签名发布。
4. 修复/确认 PR2 的 349/350、460/461 硬阈值跳变，再谈合并。
5. 清理仓库根目录的 review zip，或统一放到 Release。

---

## 8. 快速命令

```bash
# 查看状态
cd /vol1/@appdata/deepseek.harness/home/FusionPlay
git status -sb
git log --oneline -10

# 测试
cd flutter
export PATH="$HOME/.cache/flutter-sdk/flutter/bin:$PATH"
flutter pub get
flutter analyze
flutter test

# 正式发布（只需 bump VERSION + pubspec，然后打 tag）
printf '1.7.3\n' > VERSION
# 改 flutter/pubspec.yaml version: 1.7.3+10703
git add VERSION flutter/pubspec.yaml
git commit -m 'release: FusionPlay Android 1.7.3'
git tag android-v1.7.3
git push origin main android-v1.7.3
```

GitHub 认证：token 在 `/root/.gh_token`，用 `GIT_ASKPASS` 临时脚本，不要把 token 写进 remote URL 或命令行。
