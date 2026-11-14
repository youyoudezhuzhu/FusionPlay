# 多设备/多房间同步笔记

## 1. 小米 MiPlay 的同步算法（esp-sonic-sync 逆向，MIT）

[esp-sonic-sync](https://github.com/MYHealer/esp-sonic-sync) 从小米 `libaudiomirror-jni.so` 逆出多设备同步逻辑，核心是 **4 档漂移状态机 + Sonic 时间伸缩**：

| 漂移 |drift| | 策略 |
|---|---|---|
| < 100ms（deadzone） | 不调整，平滑回到 1.0× |
| ≤ 300ms | 温和修正（约 0.03%/ms） |
| ≤ 600ms | 激进修正（约 0.08%/ms） |
| > 600ms | 丢弃缓冲、重建 |

- 所有档位按音频帧对齐，避免 PCM 帧撕裂；
- 用 Sonic 时间伸缩变速不变调（±10% 内不明显）；
- **±0.5% 内旁路 Sonic**：其 look-ahead 在接近 1.0× 时会造成 I2S 欠载（爆音/卡顿）；
- deadzone 建议 > 物理距离延迟（10m≈30ms）+ WiFi 抖动（约 20ms）。

该实现是算法 + ESP32 I2S hook，**不含漂移测量/传输**（需自行用 ESP-NOW/NTP/主从时钟提供 drift）。

## 2. FusionPlay 现状与已做改动

- **Windows / Android（单设备）**：妙播按发送端 presentation clock 排程（800ms 目标队列减主机输出延迟），提前等待、迟到帧硬丢弃、按发送端时钟微调速率；这是单设备同步，不涉及多设备漂移。
- **fnOS 多输出设备**：每个 ALSA 设备独立 ring（目标填充 120ms、容量 300ms）+ rubato 重采样漂移补偿 + 延迟补偿。参考 4 档模型新增了**分级漂移控制**：
  - 档位按 fnOS 小 ring 实际换算（不是生搬 100/300/600ms）：`|误差| < 5ms` Hold（维持原有连续 P 环，不粗调）/ `≤40ms` Gentle（速率限幅 300ppm）/ `≤100ms` Aggressive（限幅 1000ppm）/ `>100ms` Recover（整帧 prefill/discard 重建 + 重校准）；
  - 阈值与 ppm 可用 `FUSIONPLAY_FNOS_DRIFT_TIERS`（0 关闭回退旧行为）及 `FUSIONPLAY_FNOS_DRIFT_{HOLD,GENTLE,AGGRESSIVE}_MS`、`FUSIONPLAY_FNOS_DRIFT_{GENTLE,AGGRESSIVE}_PPM` 调整；
  - 小误差行为与改动前完全一致，校准预热期间不触发重建；仍使用 rubato 做速率修正。
- **未做 time-stretch**：Sonic 类变速不变调需要独立的 DSP 子系统（PC 端需引入 SoundTouch/rubberband 或自研 WSOLA），且必须用真实多设备/声卡验证听感；PC/DAC 与主机时钟差是 ppm 级，现有 ±0.1% 重采样已不可闻，Sonic 只在 ESP32 那种 ±10% 大漂移场景收益明显，列为后续独立工作项。

## 3. 后续可做

1. PC 端时间伸缩库选型（SoundTouch/rubberband/WSOLA），先离线对比音质与 CPU；
2. 把 fnOS 的 4 档阈值与现有延迟补偿统一成一套可观测指标（每设备 drift_ms/修正速率/重建次数），暴露到诊断日志；
3. 多房间"组"协议本身（Xiaomi group play / AirPlay 多房间）是新产品能力，不是当前优化项；如做，需要先定义主时钟与漂移测量通道；
4. 可选诊断：把每设备当前档位/重建次数暴露到状态或日志，便于现场判断 Recover 是否频繁触发。
