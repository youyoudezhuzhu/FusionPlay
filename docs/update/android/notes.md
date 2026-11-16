## FusionPlay Android 1.4.7

### AirPlay 音量改为真正控制本机媒体音量

- 1.4.6 已在 `/info` 声明 `initialVolume` / `volumeControlType`，iPhone 现在会发送
  `SET_PARAMETER volume`；但 1.4.6 把它映射到了 PCM 增益，听起来只是“音频缩放”，
  不是系统媒体音量。
- 1.4.7 改为与小米妙播一致：AirPlay 音量直接映射到 Android `STREAM_MUSIC`，
  PCM 源保持 unity gain，不再做二次衰减。
- 启动时把当前系统媒体音量同步给 AirPlay 接收端，使 `/info initialVolume` 不再是
  固定 0dB/100%。
- iPhone 音量滑块现在应真正改变 FusionPlay 所在手机/平板的媒体音量；仍需真机确认。

### AirPlay 进度条

- 播放器 UI 继续使用 250ms 本地时钟插值（1.4.6 已加入），即使 AirPlay 没有新的
  progress/歌词事件，进度条也会持续推进。

### 歌词提示

- 修复布局切换时旧的居中空态残留在 `AnimatedSwitcher` 中，导致实际已经显示歌词、
  下面仍出现「音频流未提供歌词」的问题；有歌词时不再保留旧的居中空态。

### 其他

- 音频链路保持 1.4.5/1.4.6，不改 paced direct / 64KB audioBufferSize，避免音频回归。
- APK 约 12.4MB，arm64-v8a。

### 验证建议

- iPhone 连接后调音量，确认 FusionPlay 所在设备的系统媒体音量变化（而不是只有音频缩放）。
- 观察进度条是否在两次歌词更新之间持续前进。
- 有歌词时确认不再出现「音频流未提供歌词」。
- 本次音量映射尚未经过 iPhone 真机验证，不能提前宣称完全解决。

### 安装

- 版本 1.4.7 / versionCode 10407，可直接覆盖 1.4.6。
- 1.3.4+ 可在「设置 → 检查更新」升级。
