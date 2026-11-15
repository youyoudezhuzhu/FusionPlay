# Android 歌词显示

Android 端已支持在播放页显示歌词，并对妙播（MiPlay）的逐字歌词做卡拉 OK 式高亮。

## 数据来源

| 来源 | 获取方式 | 能力 |
|---|---|---|
| 妙播（MiPlay） | SDK 在 `media_info` 事件里带 `lyrics` 文本（手机下发） | 行级 / 逐字（yrc 等） |
| DLNA | DIDL 嵌入歌词或 `lyricsURI`/`.lrc` sidecar | 行级（含增强 LRC 的逐字时间戳） |
| AirPlay 2 | 协议不提供歌词 | 无 |

Kotlin 侧把歌词解析为 `LyricsDocument`（行 + 可选词级时间戳），通过 `app_state` 的
`playback.lyrics`（`lines[].startMs/text/words[].startMs/endMs/text`）下发；Flutter
只负责显示与动画。

## 支持的歌词格式

- 标准 LRC：`[mm:ss.xx]整行`
- 增强 LRC：`[mm:ss.xx]<mm:ss.xx>词<mm:ss.xx>词`（词级）
- 妙播 yrc 词流：`[mm:ss.xxx]{词}`（词级）
- QRC 风格毫秒时间：`[startMs,durationMs]词(startMs,durationMs)`（词级）
- 同时间戳的多行（翻译）会合并为一行显示
- `[offset:±ms]`、`[ar:]`/`[ti:]` 等元数据会被处理/忽略

## 交互与动效

- 歌词常驻播放页右列（新播放页设计），跟随封面/曲目信息展示；无歌词时显示空态。
- 播放页布局、三点菜单、设置浮层与音量控制见 [ANDROID_PLAYER_UI.md](ANDROID_PLAYER_UI.md)。
- 同步歌词：当前行放大加粗、其余降低透明度，行切换 200ms 过渡；自动居中滚动
  （420ms easeOutCubic），用户手动滚动后暂停 4 秒并显示「回到当前」。
- 逐字高亮：当前行按词时间填充亮色（ClipRect），位置由本地 `LyricsPlaybackClock`
  在 10Hz 状态采样之间外推（小幅回退单调处理，seek/换曲重锚）。
- loading：shimmer 扫光；无歌词：空态提示；非同步歌词：静态列表。
- 所有动画在系统"减少动态效果"（`MediaQuery.disableAnimations`）下退化为静态。

## 已知限制

- 妙播歌词格式由手机端决定，当前用启发式解析（yrc/LRC/QRC 风格）；若某个 HyperOS/App
  版本使用其他格式，需要抓一份 `media_info.lyrics` 样本再补解析器。
- 逐字高亮基于墙钟外推而非音频硬件时钟，长时间暂停/seek 后可能有 ~100ms 级偏差。
- 词高亮为硬边填充，后续可加 2px 渐变羽化。
- 歌词数据完全依赖发送端下发；Windows 端目前不显示歌词（未接状态）。

## 调试

- 打开 `设置 → 导出日志` 后，日志里会包含小米媒体信息/歌词解析相关事件；
- Kotlin 解析器单测：`flutter/android → gradlew :app:testDebugUnitTest --tests '*LyricsParserTest*'`；
- Flutter 测试：`flutter test test/lyrics_*.dart test/lyrics_player_test.dart`。
