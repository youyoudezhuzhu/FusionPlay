## FusionPlay Android 1.3.6

### 全新播放页（YAQMC 风格）

- 左侧大封面 + 曲目信息，右侧常驻大号歌词；底部悬浮液态玻璃控制栏（进度、上一曲/播放/下一曲、音量滑杆/静音）。
- 右上角改为三点菜单（液态玻璃悬浮展开）：设置、检查更新、导出日志、关于；设置改为居中玻璃浮层，不再从侧边滑入。
- 参考 FlClash 的液态玻璃动效：按钮/菜单/控制栏带折射、边缘高光与流动光感（Android 真机走驱动 shader，其余平台自动退化为毛玻璃）。
- 播放/暂停图标 morph、进度平滑、封面呼吸等动效，全部遵循系统「减少动态效果」。

### 歌词

- 妙播逐字歌词（yrc/增强 LRC/QRC 风格）卡拉 OK 式填色；DLNA 支持 LRC/嵌入歌词/`.lrc` sidecar。
- 有歌词时默认显示，可用右上角「歌词」按钮手动隐藏/恢复，换曲后恢复默认显示。
- 没有歌词时改为封面居中布局，并按渠道提示：AirPlay 2 不提供歌词 / 此 DLNA 发送端未提供歌词 / 此歌曲没有歌词。
- 歌词加载 shimmer、空态、自动居中滚动、手动滚动后「回到当前」均保留。

### AirPlay 2 内核更新

- 支持 AirPlay 2 **buffered ALAC（无损）**，Apple Music/本地无损不再可能静音。
- 兼容 iOS/macOS 27 的未知 SSRC 首包、SETMAGICCOOKIE、RTCP 控制通道时间通告。
- 长时间播放（>13.5 小时）改为 64-bit RTP 时间线；PTP 主时钟选择与路径延迟补偿；自定义 ALAC 解码器边界加固。

### DLNA 控制面更新

- Sink ProtocolInfo 与实际解码能力对齐（只广告音频格式），避免控制点选中视频/HLS 后报错。
- GENA 事件重试/订阅上限/私网回调；SSDP 支持 MX 随机延迟、BOOTID/CONFIGID、IPv6 发现与双栈 HTTP；补 `PrepareForConnection`。
- Android 的 DLNA 播放仍由 ExoPlayer 负责，HTTPS/Range 能力沿用系统播放器。

### 安装

- 版本 1.3.6 / versionCode 10306，固定 release 签名，可直接覆盖 1.3.5。
- 1.3.4 及以上可在「设置 → 检查更新」直接升级；1.3.3 及更早需手动安装一次。
