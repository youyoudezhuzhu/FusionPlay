## FusionPlay Android 1.5.1

### 妙播（MiPlay）暂停状态修复

另一 Agent 报告的 `docs/MIPLAY_PAUSE_ACK_NO_UI_UPDATE.md` 问题已修：

- PushMi legacy 发送端暂停时，FusionPlay 回了 PAUSE_ACK，但 active 媒体源没有登记/已随控制 socket 关闭而清空，导致 UI 仍显示播放、进度继续。
- 处理：
  - legacy 明文 `wfd://`/`rtsp://` OPEN 不再被 `SAFETY_INFO` 分支吞掉；OPEN 正确登记媒体路由。
  - 保留最近媒体源（10 分钟 TTL），控制 socket 关闭后仍能用其 PlaybackGate 暂停/恢复。
  - PAUSE/RESUME/SET_MEDIA_STATE 依次按 active → recent → 唯一媒体路由兜底；ACK 仍先回。
  - 增加 `media_route_activated` / `playback_command_resolution` 定向日志。
- 预期：PushMi 开始推送后，FusionPlay 点暂停 → UI 显示暂停、进度停住；恢复/手机侧暂停也能同步。
- 仍需 PushMi 2.6.20a10 真机确认。

### AirPlay 歌词

保持 1.5.0 的“标题持续变化即歌词”识别（网易云 title 是歌词行、artist 是歌曲名/作者）：
- 连续 3 个不同 title 判定为歌词；
- 左封面/右歌词、300ms fade+scale 动画、移除空态；
- 哔哩哔哩等 title 稳定的发送端不误判。

### 其他

- 音量映射保持 1.4.8 的妙播同款系统媒体音量曲线。
- 错误气泡 3 秒自动消失。
- APK 约 12.4MB，arm64-v8a。

### 安装

- 版本 1.5.1 / versionCode 10501，可直接覆盖 1.5.0。
- 1.3.4+ 可在「设置 → 检查更新」升级。
