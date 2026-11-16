## FusionPlay Android 1.4.9

### 调整

- 顶部错误提示气泡停留时间从 **7 秒缩短到 3 秒**，手动关闭仍然有效；同一错误持续上报不会重置计时。
- 新增 AP2 now-playing 键名诊断：如果 AirPlay 通过 event channel 的
  `updateMRNowPlayingInfo` 发送歌词字段，日志会打印**键名**（不打印内容），
  用于确认歌词是否真的随 AirPlay 下发。
- 音量映射保持 1.4.8 的妙播同款 0–100% 系统媒体音量曲线；PCM 源保持原音量。
- 音频、进度条逻辑不变。

### 关于 AirPlay 歌词

当前 1.4.8 真机日志中 AirPlay 元数据仍只有 title/artist/album/artwork/progress，
未知 DMAP 标签只有 `mper`、`ascp`、`caps`。AirPlay 2 标准元数据没有歌词字段；
iPhone App 内显示的歌词不会自动通过 AirPlay 发给接收端。因此 FusionPlay 只有在
实际收到 lyrics payload 时才显示“左封面/右歌词”；没有数据时显示居中空态。
1.4.9 的诊断会帮助确认 event channel 是否携带厂商歌词键；若确实没有，需要接入
在线歌词或本地 .lrc 数据源才能显示歌词。

### 安装

- 版本 1.4.9 / versionCode 10409，可直接覆盖 1.4.8。
- APK 约 12.4MB，arm64-v8a；1.3.4+ 可在「设置 → 检查更新」升级。
