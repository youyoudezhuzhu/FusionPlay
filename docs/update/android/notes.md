## FusionPlay Android 1.3.4

### 新增：应用内在线更新

- 设置 → 关于应用 → **检查更新**：从 GitHub Pages 读取更新清单并对比 versionCode。
- 发现新版本后可**在应用内下载 APK**，校验 SHA-256 与官方 release 签名证书后拉起系统安装器。
- 首次使用需要允许「安装未知应用」；APK 使用固定 release 签名，可直接覆盖安装 1.2.7 及之后的版本。
- 更新清单与 APK 由 CI 自动发布：下载页 <https://youyoudezhuzhu.github.io/FusionPlay/>，GitHub Releases 作为镜像。

### 妙播（MiPlay）

- **接收器名清洗**：名称含 `.` 或超长时不再破坏 mDNS 实例名，修复部分情况下设备从手机妙播列表"消失"的问题。
- 设置页增加设备类型提示（部分 HyperOS 版本下「音响」可能不出现在妙播列表，可改用「电视」）。
- 新增 HE-AAC 诊断事件（排查有连接无声/变调）；`media_info` 增加歌词字段；迟到丢帧阈值可用环境变量调整；新增可选的 UDP 5355 legacy 发现通道（默认关闭）。

### DLNA

- 网易云音乐 `video/flv` 纯音频投屏在 DLNA 控制面归类为音频，不再按视频拒绝；新增 `.aiff/.aif` 识别。
- 播放仍由系统 Media3 负责，遇到不支持的编码会明确报错。

### 安装与兼容

- 版本 1.3.4 / versionCode 10304，包名 `com.fusionplay.android`，最低 Android 5.0（API 21）。
- 覆盖安装保留设置与数据；从 1.2.6 或更早的 debug 签名版本升级需要先卸载一次。
- 本版 APK 继续使用固定 release 密钥签名，构建时校验证书 SHA-256。
