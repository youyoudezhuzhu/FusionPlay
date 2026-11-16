## FusionPlay Android 1.4.8

### AirPlay 音量曲线对齐妙播

- 1.4.7 已确认 iPhone 调音量能改变 FusionPlay 所在设备的系统媒体音量，但映射比例不对：
  之前用 `10^(dB/20)` 的线性幅度百分比，iPhone 中段音量会被压得很低。
- 1.4.8 改为 **AirPlay 音量 dB 的线性曲线**：AirPlay 的 0 dB…-30 dB 映射到系统媒体
  音量的 100%…0%，与妙播直接下发 0–100 百分比的曲线对齐；PCM 仍保持 unity gain，
  不缩放音频源。
- 换算：`percent = (db + 30) / 30 * 100`，`db = -30 + percent * 0.3`；0% 仍视为静音
  （-144 dB）。
- 仍需真机确认低/中/高段听起来与妙播一致。

### AirPlay 歌词：协议限制说明

- AirPlay 2 **标准元数据不包含歌词字段**。本机实际收到的 now-playing 只有
  title/artist/album/artwork/progress；日志中的未知 DMAP 标签只有
  `mper`、`ascp`、`caps`，没有 `asly` 或其他歌词标签。
- iPhone 上能显示歌词，是发送端 App 自己的界面；它不会通过 AirPlay 元数据把歌词
  发给接收端。因此 FusionPlay 的 AirPlay 页面只能显示封面/标题/作者，无法凭空得到歌词。
- 当前 UI 规则：**只有实际收到歌词时**才显示“左封面、右歌词”的双栏；没有歌词数据时
  显示居中封面 + 「音频流未提供歌词」。这不是布局 bug，是当前 AirPlay 流确实没有
  歌词 payload。
- 如果希望在 AirPlay 下也显示歌词，需要额外接入歌词数据源（例如按标题/艺术家查询
  在线歌词或本地 .lrc），这不属于 AirPlay 2 标准能力，需要单独确认是否要做。

### 其他

- 音频链路、进度条、音量系统映射保持 1.4.7；APK 约 12.4MB，arm64-v8a。

### 安装

- 版本 1.4.8 / versionCode 10408，可直接覆盖 1.4.7。
- 1.3.4+ 可在「设置 → 检查更新」升级。
