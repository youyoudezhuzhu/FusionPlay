## FusionPlay Android 1.4.1

### AirPlay 识别修复

- 修复 iPhone 把 FusionPlay 当成 TV/投屏目标的问题。根因不是 mDNS，而是连接时自述：`GET /info`、AirPlay 2 event channel 的 `updateInfo`、`/server-info` 仍固定返回 `AppleTV6,2` 和含 video/HLS 位的 features，覆盖了正确的音频广播。
- 现在音源模式在 `/info`、`updateInfo`、`/server-info` 全部返回纯音频 features（不再含 video/photo/screen/HLS 位）和 `AirPort10,115` 模型，并且不广告 `displays`；iPhone 应只把它当音响，不再投视频。

### AirPlay 2 播放稳定性

- 复核 AAC/ALAC、时间戳、重采样和 Oboe 回调后未发现解码/丢包根因；针对 ColorOS/OPPO 上 `LowLatency + Shared` 小硬件缓冲导致的 underrun，改为 Oboe 稳定模式，Android 本地缓冲 240→480 ms、预填充 120→240 ms（总延迟仍以协议 2 s 为主）。
- 增加 underrun/重新预填充限频日志，下一次导出的 `airplay.jsonl` 可直接确认是否还有欠载。

### 歌词与界面

- 不再按协议写死「AirPlay 2 不提供歌词」：改为实时判断，无歌词统一显示「音频流未提供歌词」，AirPlay 有歌词时正常渲染并显示歌词开关。
- 布局顺序调整为封面 → 曲名 → 作者 → 歌词（曲名在作者上方）；360 高手机空态也保留作者行。
- 修复每切一句歌词封面旋转：封面动画身份改为封面 source 的稳定指纹，同一封面遇到频繁 now-playing 更新不再 flip。
- 顶部错误提示从白色 Material 改为深色玻璃 toast（例如妙播音量同步失败这类提示），与整体玻璃系统一致。
- AirPlay 协议层未找到可验证的歌词字段；新增未知 DMAP 标签（仅标签名+长度，不含内容）诊断日志，下次可确认发送端是否使用厂商扩展歌词标签。

### 安装

- 版本 1.4.1 / versionCode 10401，固定 release 签名，可直接覆盖 1.4.0。
- 1.3.4 及以上可在「设置 → 检查更新」直接升级。
