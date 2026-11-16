## FusionPlay Android 1.4.2

### 安装包大幅缩小

- 默认发布包改为 **arm64-v8a 单架构**：不再把 arm64 / armv7 / x86_64 三套 Flutter engine 与 Rust core 一起打包。实测 APK 从 **31.8 MB → 约 12.3 MB**（约 -61%）。
- MiSans 字体子集化（保留 UI 文案 + GB2312 常用字 + ASCII，约 2.1 MB）；移除未使用的 HLS 依赖并限制资源语言。
- **兼容性说明**：默认包仅支持 64 位 Android 设备。32 位设备需使用 `flutter/tool/build_universal.ps1 -Abis armeabi-v7a` 单独构建；不会走当前 arm64 在线更新。

### AirPlay 2 修复

- 修正 AAC 解码对 raw AAC / ADTS 的处理（symphonia 直接吃 raw data block；带 ADTS 时先剥离），避免把声音解成静音。
- 回退 1.4.1 的 Oboe/缓冲实验到 1.4.0 可出声配置（LowLatency、240/120ms），并增加“已开始输出 / 6 秒无解码 PCM / underrun”诊断，便于下一步真机定位。
- AirPlay 音量改为接收端本地 gain：调节不再需要发送端支持 SetVolume，不再弹「当前设备不支持此远程控制命令」；`GET volume` 返回接收端当前音量，sender 与本地滑杆共享音量状态。
- 音量滑杆拖动只跟手、松手只发一次；顶部错误提示改为玻璃 toast，7 秒自动消失，同一错误不会重置计时。

### 其他

- 保持 1.4.1 的 AirPlay 纯音频身份修复（`/info`、updateInfo、`/server-info` 均返回 AirPort10,115 / 无 video 位）。
- 歌词布局、封面翻转、玻璃/设置界面等 1.4.1 改动保留。

### 安装

- 版本 1.4.2 / versionCode 10402，固定 release 签名，可直接覆盖 1.4.1。
- 1.3.4 及以上可在「设置 → 检查更新」直接升级。
