# DLNA 兼容性说明（Windows / fnOS）

FusionPlay 的 DLNA 侧是 **MediaRenderer（DMR）**：SSDP 发现 + AVTransport/RenderingControl + GENA 事件回传，媒体拉流由宿主侧 `dlna_player` 完成（HTTP/file → symphonia 解码 → 48kHz 立体声 → 统一引擎）。

## 1. 支持的媒体格式

| 容器/编码 | 状态 | 备注 |
|---|---|---|
| MP3 | ✅ | |
| AAC（ADTS / m4a） | ✅ | AAC-LC；HE-AAC（SBR）需发送端/解码器支持 |
| FLAC | ✅ | |
| WAV（PCM/ADPCM） | ✅ | |
| OGG / Vorbis | ✅ | 1.3.2 起 |
| M4A / MP4 音频轨 | ✅ | 只播音频轨；含视频轨的 mp4 会拒绝 |
| **FLV 封装 AAC**（网易云音乐） | ✅ | 网易云把纯音频上报为 `video/flv` + `videoItem`；core 归类为音频，宿主做 FLV AAC demux |
| **FLV 封装 MP3** | ✅ | FLV SoundFormat=2；按 MP3 帧头切帧后交给 MP3 解码器 |
| ALAC / AIFF | ✅ | symphonia `alac`/`aiff` feature |
| WMA / APE / 真实视频 / HLS / 转码 | ❌ | 明确报错；不静默失败 |
| M4A/ALAC 走 HTTP 且 moov 在文件尾 | ⚠️ | 无 HTTP Range 时无法 seek 到 moov；`file://` 正常 |
| HTTPS 媒体源 | ❌ | 当前未打包 TLS 栈（见 §4） |

## 2. HTTP 拉流行为

- 支持 `Content-Length`、`Transfer-Encoding: chunked`、连接关闭定界；跟随 301/302/303/307/308（最多 4 次）。
- **User-Agent 默认使用浏览器 UA**（Chrome/126 系列）：QQ 音乐 CDN 会对非浏览器 UA 返回 403，网易云等 CDN 接受通用 UA。
- **401/403 自动回退**：首次用浏览器 UA；若被拒，重连并改用旧式 `FusionPlay-Windows/<版本>` / `FusionPlay-fnOS/<版本>` UA 重试一次，日志记录 `used_fallback_ua`。
- 可用 `FUSIONPLAY_DLNA_USER_AGENT` 覆盖默认 UA、`FUSIONPLAY_DLNA_FALLBACK_USER_AGENT` 覆盖回退 UA（排查个别服务器时有用）。
- 回退只针对 401/403、每个请求链最多一次；日志里记录 `used_fallback_ua`。
- 整体遵守 abort/timeout/重定向限制；`stop()` 后阻塞读会在 ≤1s 内退出。

## 3. 国产音乐 App 兼容要点

| App | 现象 | 处理 |
|---|---|---|
| 网易云音乐 | 纯音频投屏用 `video/flv` + `object.item.videoItem`，1.3.1 前被当视频拒绝 | core `media_kind()` 特判 FLV 音频；宿主 FLV AAC/MP3 demux |
| QQ 音乐 | CDN 对非浏览器 UA 返回 403；`SetPosition` 调用频繁 | 浏览器 UA + 401/403 回退；控制面按现有节流处理 |
| 网易云/QQ | 切歌时 Stop→SetURI→Play 间隔很短、`SetNextAVTransportURI` 可能延迟 | DMR 状态机已有测试覆盖；如遇具体时序问题，需要抓包后针对性调整（见 §5） |

## 4. 暂未实现：HTTPS 媒体源

许多 NAS/云音乐直链使用 `https://`。当前两个宿主只实现 HTTP/1.1 明文拉流（无 TLS 栈），因为：

- 需要引入 rustls + 根证书，并把两端 `HttpBody`/`HttpSource` 的 `TcpStream` 泛型化（超时轮询、chunked、重定向都要保留）；
- 需要在 Windows/fnOS/CI 上做真实证书握手与超时行为测试；
- 增加二进制体积与安全面（SNI、证书验证策略）。

建议作为独立变更（TLS 网络栈）而不是混在本次兼容性修复里；实现时优先复用一份共享的 `HttpTransport` 抽象，避免两端再复制一遍。

## 5. 暂未实现：DMR 控制面 edge case

以下行为需要真实 App 抓包才能确认差异，未做盲改：

- QQ 音乐高频 `SetPosition` 的相对/绝对映射与节流；
- 网易云 `Stop → SetAVTransportURI → Play` 极短间隔下的状态广播顺序；
- QQ `SetNextAVTransportURI` 延迟到达时的"下一曲"预加载语义；
- GENA 订阅续订/断线重连时序。

如果遇到具体问题，导出 `%LOCALAPPDATA%\FusionPlay\exports\FusionPlay-diagnostics-*.txt`（含 SOAP 请求/事件日志）后可针对性补测试与修复。

## 6. 参考

- [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay)（MIT）：ESP32 DLNA 渲染器，含 QQ/网易云/酷狗兼容注释（FLV MIME、QQ CDN UA、GENA 事件、切歌时序）。
- [MYHealer/dlna-esp32](https://github.com/MYHealer/dlna-esp32)：ESP32 DLNA 渲染器参考实现。
