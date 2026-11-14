# ESP32 复用可行性笔记（未来选项，非当前 roadmap）

结论（2026-11 评估）：**ESP32 做通用音频接收器可行，但不应把 FusionPlay 的 Rust 栈整体移植。**
AirPlay 2 已有开源实现跑在 240MHz ESP32-S3 上；妙播（MiPlay）也有两套完整 ESP32 实现可直接参考/移植。
若将来启动 ESP32 版本，建议按 "Linux SBC 先落地 → ESP32-S3 精简版（AirPlay2+DLNA）→ MiPlay 二期" 走。

## 1. 硬件门槛

| 芯片 | CPU | 内部 SRAM | PSRAM | 内置 WiFi/BT | 说明 |
|---|---|---|---|---|---|
| ESP32（经典） | 240MHz 双核 Xtensa | ~520KB | 可选（WROVER 4MB） | WiFi + BT Classic | 有 BT Classic 才能做 A2DP |
| ESP32-S3 | 240MHz 双核 LX7 | 512KB | 8MB（Octal）常见 | WiFi + BLE only | **不能做 A2DP**；AirPlay 2/MiPlay 都用 S3 验证过 |
| ESP32-P4 | 400MHz 双核 RISC-V | 768KB | 支持 | **无 WiFi**（需 C6/ESP-Hosted） | 内存/算力更宽裕，设计更复杂 |
| ESP32-C5/C6 | 240MHz/160MHz | 512KB | 小 | WiFi6/BLE | airplay-esp32 支持 C5 |

内存估算：AirPlay 2 默认 2s 缓冲约 350KB；MiPlay 队列 1.5s f32 ≈ 576KB；控制帧上限 256KB，单帧解密 256KB。**S3 必须外挂 PSRAM**，PSRAM 带宽（Quad < Octal）会影响稳定性。

## 2. 现成可复用项目

| 项目 | 许可 | 可复用内容 |
|---|---|---|
| [rbouteiller/airplay-esp32](https://github.com/rbouteiller/airplay-esp32) | GPL-3.0（含 Espressif 闭源解码器 linking exception） | AirPlay 2 + ALAC/AAC + PTP 多房间 + 48k；ESP32/S2/S3/C5 实机验证 |
| [MYHealer/miplay-mini-amp](https://github.com/MYHealer/miplay-mini-amp) | AGPL-3.0 | MiPlay 完整链路（5355+mDNS、TCP 8899、RTSP/RTP/TS/AES/AAC），20 元 BOM 实机验证；身份/发现/显示名/内存实测数据 |
| [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay) | MIT | MiPlay + DLNA 渲染器、5355 legacy 应答、多 App DLNA 兼容经验 |
| [MYHealer/esp-sonic-sync](https://github.com/MYHealer/esp-sonic-sync) | MIT | 小米多设备同步 4 档漂移模型 + Sonic 时间伸缩（详见 MULTIROOM_SYNC_NOTES.md） |
| [MYHealer/dlna-esp32](https://github.com/MYHealer/dlna-esp32) | — | ESP32 DLNA 渲染器（GMF 管线、GENA、歌词） |

## 3. 关键移植约束（来自实机经验）

- **Codec**：ESP32 侧用 Espressif `esp_audio_codec`（ALAC/AAC 闭源二进制，仅限 Espressif 芯片）时，固件许可需要类似 airplay-esp32 的 GPL linking exception；否则改用开源解码器并重新评估 240MHz 实时余量。妙播侧 esp_miplay 启用 AAC+（HE-AAC）解码。
- **发现**：手机 SystemUI 会走 UDP 5355 `_miplay_lan._tcp`；mDNS 5353 + 5355 双通道身份必须逐字节一致（idHash 5355 侧用 base64url）。
- **显示名**：不能含 `.`，实例名编码后 ≤63 字节（ESP32 实现里还额外受 27 字节 snprintf 限制）。
- **帧/内存**：控制帧 256KB（封面 base64+逐字歌词），64KB 缓冲会丢同步；PSRAM 需放接收/解密缓冲。
- **音频输出**：I2S + PCM5102/ES9038 等 DAC；I2S DMA 本身可当 jitter buffer，是否加 ring 取决于实测。
- **AirPlay 商标**：非 MFi 只能做兼容实现，不能宣称官方认证。

## 4. 建议路线

1. **短期**：Linux SBC（Pi Zero 2W / RK3308 / 香橙派）+ ALSA 复用 fnOS Rust 后端（三协议齐全，1-3 周）。
2. **中期**：ESP32-S3 + 8MB PSRAM，先 AirPlay 2 + DLNA（复用 airplay-esp32/esp_miplay 思路，4-8 周）。
3. **二期**：MiPlay 移植（参考 miplay-mini-amp/esp_miplay，2-4 个月，HyperOS 兼容是主要风险）。
