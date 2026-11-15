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
| M4A/ALAC 走 HTTP 且 moov 在文件尾 | ✅ | 服务器支持 `Range` 时可直接 seek 到 moov；无 Range 时仍退回顺序读取 |
| HTTPS 媒体源 | ✅ | rustls + Mozilla 根证书，强制校验证书链/主机名（自签证书会被拒绝） |
| HTTP `Range` 跳转 | ✅ | 支持 `206`/`Content-Range`；不可 Range 的服务器回退顺序解码丢弃 |

## 2. HTTP 拉流行为

- 支持 `Content-Length`、`Transfer-Encoding: chunked`、连接关闭定界；跟随 301/302/303/307/308（最多 4 次，http↔https 跨 scheme 额外限次）。
- 支持 `https://`（默认 443，SNI/证书校验，TLS 握手受 timeout/abort 约束）与 HTTP `Range: bytes=N-` 断点/跳转读取。
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

## 4. HTTPS / TLS

两个宿主的 `dlna_player` 已接入 rustls（ring provider）+ webpki/Mozilla 根证书：

- 证书链与主机名（SNI + SAN）强制校验，没有“跳过校验”开关；
- 握手受连接超时/abort 约束，失败返回中文错误；
- 重定向允许 http↔https，但跨 scheme 有次数上限；
- **自签名证书目前会被拒绝**。如果 NAS 场景确实需要，应增加“自定义 CA / 信任列表”配置，而不是默认关闭校验。

## 5. HTTP Range 与 seek

- HTTP 源实现了 `Seek`：服务器 `Accept-Ranges: bytes` 或 `Range` 请求返回 `206` 时启用，`Content-Range`/`Content-Length` 做严格一致性校验。
- DLNA `Seek` 优先走 Symphonia 时间 seek（`seek_mode=format`）；不支持时用 `Range: bytes=<估算偏移>-` 重开（`seek_mode=range`），再不行才顺序解码丢弃（`seek_mode=discard`）。
- moov 在文件尾的 M4A 现在可以经 HTTP Range 直接播放/拖动；无 Range 的服务器仍按原顺序回退。

## 6. 控制面更新（2026-11）

- Sink ProtocolInfo 已与真实解码能力对齐：只广告 MP3/MP4/AAC/FLAC/WAV/OGG/OPUS/ALAC/AIFF，移除视频/HLS。
- GENA：回调允许同 peer 或双方私网（RFC1918 / IPv6 ULA），继续拒绝 loopback（异主）/link-local/组播/公网/域名；NOTIFY 失败 200ms 后同 SID/SEQ 重试一次；订阅数限制（单 peer ≤8、总 ≤64）；过期订阅清理。
- SSDP：解析 `MX` 并在 0..min(MX,3s) 随机延迟后异步应答；M-SEARCH/NOTIFY 增加 `BOOTID.UPNP.ORG`/`CONFIGID.UPNP.ORG`；支持 IPv6 SSDP（`ff02::c`）与双栈 HTTP listener（失败回退 IPv4）；纯 link-local IPv6 环境不回 v6 广告。
- ConnectionManager 增加 `PrepareForConnection`（按规范返回 ConnectionID/AVTransportID/RcsID）。

## 7. 仍需真实 App 抓包确认的时序

- QQ 音乐高频 `SetPosition` 的相对/绝对映射与节流；
- 网易云 `Stop → SetAVTransportURI → Play` 极短间隔下的状态广播顺序；
- QQ `SetNextAVTransportURI` 延迟到达时的“下一曲”预加载语义；
- 复杂 NAT/多网卡环境下的 GENA 回调与事件丢失对账。

如果遇到具体问题，导出 `%LOCALAPPDATA%\FusionPlay\exports\FusionPlay-diagnostics-*.txt`（含 SOAP 请求/事件日志）后可针对性补测试与修复。

## 8. 参考

- [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay)（MIT）：ESP32 DLNA 渲染器，含 QQ/网易云/酷狗兼容注释（FLV MIME、QQ CDN UA、GENA 事件、切歌时序）。
- [MYHealer/dlna-esp32](https://github.com/MYHealer/dlna-esp32)：ESP32 DLNA 渲染器参考实现。
- [hzeller/gmrender-resurrect](https://github.com/hzeller/gmrender-resurrect)（GPL-2.0）：成熟 UPnP/DLNA MediaRenderer，仅阅读其 SSDP/GENA/AVTransport 行为。
