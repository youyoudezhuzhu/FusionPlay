## FusionPlay Android 1.7.1

### 播放页 21:9 视觉调整

- 顶部只显示协议，不再显示 ALAC/AAC 等音质格式。
- 删除封面下方“来源/格式”小字，进度条与时间上移并靠近。
- 封面改用独立 `FloatingAlbumArt`：柔光阴影、细边缘光，移除黑色厚底座。
- 上一曲/播放/下一曲改用上游 `liquid_glass_widgets` 玻璃按钮，播放键略大。
- 音量胶囊使用同一玻璃材质；不再叠加 FusionPlay 旧 `_GlassPaint`。
- 无歌词为居中的封面 + 信息/控件组合；有歌词保留右侧沉浸式歌词。
- 逐句流式歌词保留最近 7 句历史，不伪造未来歌词。
- 播放协议、歌词解析和业务状态逻辑未改。

### 验证

- `flutter test`：207 passed（Flutter widget 测试）。
- Android release APK 由 GitHub Actions 使用正式 release keystore 构建和验签。
