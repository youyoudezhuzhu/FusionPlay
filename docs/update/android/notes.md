## FusionPlay Android 1.4.6

### AirPlay 音量控制

- 在 AP2 `/info` 中补上 shairport-sync 同款字段：
  - `initialVolume`：接收端当前音量（dB）
  - `volumeControlType`：3（标准接收端音量控制）
- iPhone 之前一直显示 100%、调音量不控制 FusionPlay，是因为只提供了 `GET_PARAMETER volume`，没有在设备自述里声明接收端音量能力。补上后 iOS 才会把音量滑块路由到接收端并发送 `SET_PARAMETER volume`。
- 仍需验证：iPhone 音量滑块是否能改变 FusionPlay 所在手机的输出音量；如果仍不行，需要抓 `SET_PARAMETER volume` 日志确认 iOS 是否发送，以及 feature bit/volumeControlType 是否需要进一步调整。

### AirPlay 进度条

- AirPlay 的 now-playing/progress 事件很稀疏，FusionPlay 之前只在歌词/元数据更新时刷新进度。
- 1.4.6 在播放器 UI 增加本地单调时钟插值：两次进度事件之间由 250ms 定时器按 1x 推进；收到新进度或 seek 时重新锚定，暂停时停止。
- 歌词高亮同步使用同一插值位置。

### 其他

- 音频播放链路保持 1.4.5（paced direct + burst 快进 + 64KB audioBufferSize），未改动，避免回归。
- APK 仍约 12.4MB，arm64-v8a。

### 安装

- 版本 1.4.6 / versionCode 10406，可直接覆盖 1.4.5。
- 1.3.4+ 可在「设置 → 检查更新」升级。
