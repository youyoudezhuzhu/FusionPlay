# Android APK：MiPlay 旧版固定密钥媒体兼容（默认开启）

## 背景

Codex 在 `codex/miplay-clock-observer`（commit `7d3cf88`）复现并定位了兼容问题：

- 官方小米设备作为 MiPlay 发送端时，部分会话**不宣告 `0x6c` 会话密钥**，媒体是
  "旧版固定密钥 + 每 PES 私有 IV" 的加密格式；
- 原实现 `TsDemuxer::finish_pes` 在没有 `self.keys` 时**原样返回加密载荷**；
- 解码器拿到密文 → 采样率/TNS 解析失败（8kHz/96kHz 报错）→ 无声，随后可能伴随 RTSP 错误。

Codex 的 Linux 观测器用 `--legacy-fixed-media` 显式开启后，用已知的库固定密钥 +
每个 PES 私有 IV 成功解出 48kHz ADTS（49/49、27/27、50/50 帧），官方平板→NAS 实测有声。

## Android APK 的行为

- **默认开启**该兼容解码：`nativeStartMiPlay` 在创建 receiver 前调用
  `media::enable_legacy_fixed_media_compatibility()`。
- 触发条件很窄：仅当 **没有会话密钥** 且 **PES 带私有 IV** 时走固定密钥；
  正常带会话密钥的 MiPlay 媒体、以及明码 ADTS 路径完全不变。
- **开关**：在应用 identity 目录放一个空文件 `disable-legacy-fixed-media` 即关闭
  （identity 目录 = Android 应用私有目录 `files/Identity`，adb 可写：
  `adb shell run-as <applicationId> touch files/Identity/disable-legacy-fixed-media`）。
  启动时会发事件 `miplay_legacy_fixed_media_compat`（enabled true/false），日志可见。
- clock observer 仍保持 Codex 的 opt-in 机制（identity 目录 `enable-clock-observer`），
  普通安装不产生任何观测文件。

## 构建

GitHub Actions（推荐，Linux 上产 release APK）：

```bash
# 在 fork 仓库的 Actions 页面手动触发，或：
gh workflow run android.yml --repo youyoudezhuzhu/FusionPlay --ref compat/android-legacy-miplay
```

产物：`FusionPlay-Android-APK` artifact 里的 APK（1.2.5 起默认启用；1.2.6 起同时改为 AirPlay 音响身份广播，见 `docs/AIRPLAY_AUDIO_ONLY.md`）
（Flutter 默认用 debug keystore 签名，可直接安装；覆盖安装需卸载旧版以更换签名）。

本机标准构建流程见仓库 README（Flutter 3.32.8 / Android SDK 36 / NDK 28.2.13676358 /
JDK 21 / Rust + cargo-ndk）。
