# Android APK 体积优化

## 1.4.1 的 31.8 MB 从哪来

检查 release APK（未压缩总大小约 68 MB）：

| 内容 | 未压缩大小 |
|---|---:|
| 3 份 `libflutter.so`（arm64-v8a / armeabi-v7a / x86_64） | 31.7 MB |
| 3 份 `libapp.so`（Dart AOT） | 13.4 MB |
| 3 份 `libfusionplay_core.so`（Rust） | 10.3 MB |
| 3 份 `libc++_shared.so` | 3.4 MB |
| `MiSansVF.ttf` 变量字体 | 4.5 MB |
| dex / 资源 / 其他 | 约 4 MB |

APK 里的 native 库已被 deflate 压缩（`useLegacyPackaging = true`），但“三个 CPU 架构各带一套 Flutter engine + Rust core”占了绝大部分。功能并不多，体积主要来自通用 APK 的多架构重复。

## 2. 本版优化

1. **Release 只出 arm64-v8a**（`FUSIONPLAY_ABIS=arm64-v8a`，CI 用 `--target-platform android-arm64`）。
   - 去掉 x86_64 和 armeabi-v7a 两套 Flutter/Rust/C++ 库。
   - 预计 APK 从 31.8 MB 降到约 14 MB（arm64 单套 native 约 9.3 MB 压缩后 + 字体/资源/dex）。
   - 这是体积最大的单一优化项。
2. **字体子集化**：MiSans 从 4.3 MB 全量变量字体裁剪为 `MiSansUI.ttf`（约 2.1 MB，保留 4121 个字形：UI 固定文案 + GB2312 一级常用汉字 + ASCII）。动态歌曲名/歌词里的生僻字由 Android 系统字体 fallback 渲染，避免全量 CJK 字体占用。
3. **移除未使用的 `media3-exoplayer-hls` 依赖**（应用只使用 ExoPlayer 播放网络音频/普通媒体，不构建 HLS source）。
4. **限定资源语言**：`resourceConfigurations = ["zh","en"]`，避免打包无关翻译资源。
5. 保留现有 R8 minify/shrinkResources、Rust `opt-level=s` + fat LTO + strip、native 压缩、Dart `--obfuscate --split-debug-info`。

## 3. 兼容性与后续

- 默认 release 只包含 **arm64-v8a**；绝大多数 Android 手机和 Android TV 是 arm64，但 32 位设备需要：
  ```powershell
  .\\flutter\\tool\\build_universal.ps1 -Abis armeabi-v7a
  ```
  （或使用 `-Abis all` 构建兼容包）。这些包不会自动走 arm64 的在线更新清单，需单独分发。
- 若后续要同时支持 32 位 OTA，需要把 release 改成按 ABI 拆 APK，并在 update-manifest 中增加 ABI 变体；这次只做了代码侧的单 ABI 构建配置，不改变更新协议。
- 字体子集对固定 UI 文案保持 MiSans 字形；极端生僻字/部分歌词字符会落到系统 fallback，属于体积与字体完整性的取舍。

## 4. 验证方式

实际构建后可用：

```bash
unzip -l FusionPlay-Android-*.apk | sort -k1 -nr | head
```

确认不再出现 `lib/x86_64/`、`lib/armeabi-v7a/`，且 `assets/fonts/MiSansUI.ttf` 约 2 MB；最终大小以 CI 产物为准。
