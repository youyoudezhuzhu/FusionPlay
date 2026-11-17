## FusionPlay Android 1.7.2

### Android 显示问题修复

- 播放页玻璃控件切换为 `liquid_glass_widgets` 的 **minimal（shader-free）** 质量档，避免部分 Android/Impeller 设备上第三方高级 shader 导致的控件黑屏/只显示背景痕迹。
- 保留 1.7.1 的 21:9 布局、FloatingAlbumArt、玻璃按钮/音量胶囊外形和全部业务逻辑。
- 这是兼容性回退，不是最终形态；待确认设备后可在支持 Impeller 的机型上恢复更高折射质量。

### 已知

- 1.7.1 在部分 Android 设备上出现播放控件不显示/黑屏；本版优先保证可显示和可操作。
