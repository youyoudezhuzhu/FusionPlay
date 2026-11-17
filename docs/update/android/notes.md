## FusionPlay Android 1.7.0

### Liquid Glass 真实折射架构

- 不再使用会自反馈的 `ImageFilter.shader` 路径。
- 新增 `GlassBackdropScope`：把**不含玻璃自身**的背景 RepaintBoundary 捕获成纹理。
- 新增 `liquid_glass_refraction.frag`：按圆角 SDF 的边缘法线做非均匀 UV 位移，叠加色散、Beer-Lambert 边缘吸收、Fresnel/specular。
- 圆形、胶囊、圆角矩形会得到不同的边缘法线；玻璃移动/页面变化后采样新的背景纹理。
- 生产路径：全局 app 背景 + 播放页 artwork 背景各一个 scope；播放控制、菜单、设置、更新/关于弹窗、错误 toast 都复用同一 `LiquidGlass`。
- 无 scope、shader 失败或测试 fallback 时回退到稳定的 BackdropFilter 毛玻璃；回退明确标记为降级，不冒充真实折射。

### 播放页

- 歌词不再包在大玻璃卡片里，直接融入封面背景。
- 保留 Apple Music 风格左右布局、有歌词/无歌词/加载状态、AirPlay 标题歌词、控制能力判断、焦点和菜单。
- 播放/上一曲/播放/下一曲、歌词开关、三点菜单使用同一套折射玻璃。

### 验证

- `flutter analyze` 通过。
- Flutter 全量测试 **208 passed**。
- 光学场景 `test/glass_optics_test.dart`：网格背景 + 胶囊玻璃；`refraction=2.2` 与 `refraction=0` 的 bevel 区域平均 RGB 差异 > 4，截图见 `docs/glass/glass-optics.png` / `glass-optics-flat.png`。
- 新页面截图：`docs/glass/settings-960x540.png`、`menu-960x540.png`、`dialog-960x540.png`。
- 尚未在真实 Android 设备 Profile 模式验证 60 FPS/白帧；需真机复测。

### 安装

- 版本 1.7.0 / versionCode 10700，可直接覆盖 1.6.1。
- 1.3.4+ 可在「设置 → 检查更新」升级。
