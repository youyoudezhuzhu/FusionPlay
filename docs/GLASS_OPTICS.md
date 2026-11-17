# Liquid Glass 真实光学验证

## 1. 实现路径

旧路径（已移除）：

```
BackdropFilter(ImageFilter.shader)
  → shader 读取包含玻璃自身的 framebuffer
  → 多层玻璃/动画重绘时反馈，Android Impeller 白帧
```

新路径：

```
GlassBackdropScope
  ├─ RepaintBoundary(background only) → toImage() → GL-free texture
  └─ app content / glass surfaces
        └─ RefractiveGlass
              FragmentShader(ImageShader)
              SDF normal → 非均匀 UV 位移 → sample captured texture
```

关键点：

- 采样纹理来自**不含玻璃的兄弟 RepaintBoundary**，玻璃自身不在纹理里，因此不存在自反馈。
- 不是 `ImageFilter.shader`，所以绕开 Flutter 3.32/Impeller 的反馈路径。
- 玻璃边缘使用圆角矩形 SDF 的数值梯度得到真实形状法线；圆形、胶囊、圆角矩形自然得到不同的边缘折射。
- 色散、Beer-Lambert 边缘吸收、Fresnel/specular 叠在同一个 shader 中。
- 背景纹理只在 artwork/背景 revision 变化时重拍，玻璃移动/交互只更新 shader uniform，不重复采样整屏。
- 无 scope、shader 编译失败或 `forceFallback` 时，明确回退到毛玻璃路径；回退不被称为折射。

## 2. 验证场景

`flutter/test/glass_optics_test.dart`：

- 背景：24px 网格 + 96px 粗网格 + 高对比圆形/方形色块。
- 上层：330×230 圆角胶囊玻璃（半径 72，thickness 34）。
- 同一场景分别渲染 `refraction=2.2` 与 `refraction=0`。
- 采样左侧 bevel 区域，要求平均 RGB 差异 > 4.0；测试通过。
- 截图：
  - `docs/glass/glass-optics.png`（开启折射：网格在玻璃边缘明显弯折、出现色散）
  - `docs/glass/glass-optics-flat.png`（refraction=0：网格线直穿玻璃）

这证明：

- 背景实际被采样；
- 位移随 SDF 边缘法线变化，不是全局模糊；
- 关闭折射后扭曲消失；
- 不是静态纹理/固定渐变/白色描边。

## 3. 生产接入

- `main.dart` 的 `MaterialApp.builder` 放置全局 `GlassBackdropScope`，供设置、弹窗、菜单等路由采样。
- 播放页再嵌套一层以 artwork 为背景的 `GlassBackdropScope`，播放器控件/菜单优先使用真实封面背景。
- 歌词不再使用大型独立玻璃卡片；歌词直接融入封面背景，只保留文字层级。
- 播放/上一曲/下一曲、歌词开关、三点菜单、设置面板、更新/关于弹窗均走同一 `LiquidGlass` 组件。

## 4. 限制与未验证

- 背景纹理按 revision 重拍；滚动内容/动态背景若在玻璃后面逐帧变化，需要提升 revision 频率（成本更高），当前生产页背景为 artwork/gradient，稳定。
- 未在真实 Android 设备上验证 60 FPS / Impeller 白帧；widget test 和 shader 编译通过，仍需要真机日志与 Profile 数据。
- 极低端 GPU 若 shader 编译/运行失败，自动落到毛玻璃 fallback；该状态不是真实折射。
