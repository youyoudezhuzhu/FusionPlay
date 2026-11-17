# Liquid Glass Playground（独立实验，不进入生产 UI）

## 1. 目的与环境

隔离实验三种 Liquid Glass 渲染栈，使用同一背景、同一控件尺寸、同一 Apple Music 横屏布局进行视觉对照。

- 生产代码：Flutter 3.32.8 / Dart 3.8.1，未升级 SDK，未修改播放器协议/歌词/设置/生产 UI。
- 实验目录：`experiments/liquid_glass_playground/`。
- 生产包通过 path dependency 只读引用（`fusionplay_android_flutter`），不反向影响生产。
- 实验依赖：
  - `liquid_glass_widgets 0.18.4`（vendored 到 `third_party/`，见兼容性）
  - `liquid_glass_renderer 0.2.0-dev.4`
  - `vector_math 2.3.0` override

运行（本次只做实验工程，不构建 APK）：

```sh
cd experiments/liquid_glass_playground
flutter pub get
flutter test test/preview_test.dart   # 生成 docs/ 与 /tmp/fusionplay-glass-playground 截图
```

## 2. 三种渲染路径

### A. FusionPlay 自研（对照）

- 生产 `LiquidGlass` + `GlassBackdropScope`。
- 背景由独立 RepaintBoundary 捕获，不含玻璃自身。
- `liquid_glass_refraction.frag` 用圆角 SDF 法线做非均匀 UV 位移。
- 实际渲染路径标注：`捕获背景 + SDF FragmentShader（真实位移）`。

### B. liquid_glass_widgets 0.18.4

- `AdaptiveGlass` / `GlassButton` / `GlassSlider`，默认 settings 尽量保持第三方原生参数。
- 在 Impeller 上 premium 使用其完整 shader；在 Skia/测试环境自动降级 lightweight shader（仅模糊/色调，无几何折射）。
- 实际路径会显示在左下角；当前测试环境为 `Skia lightweight shader（无几何折射）`，因此截图不能代表 Android Impeller 的最终折射效果。
- **兼容性问题**：
  - 官方 0.18.4 声明 Flutter ≥3.24，但在 Flutter 3.32.8 编译失败：`FragmentShader.setImageSampler(filterQuality:)` 尚未提供，且 `Matrix4.translateByDouble` 依赖较新的 `vector_math`。
  - 实验将 0.18.4 源码 vendored，删除 4 处 `filterQuality:` 参数（退回默认采样质量），并 override `vector_math 2.3.0`。
  - **此 patch 仅用于实验，不应直接进入生产**；生产若采用 LGW，需升级到支持该 API 的 Flutter 或等待上游修复。
- 许可证：MIT（Copyright 2024 Sebastian Degenaar）。

### C. liquid_glass_renderer 0.2.0-dev.4

- `LiquidGlassLayer` + `LiquidGlass`；依赖 Impeller 的 shader filter。
- 在 Skia/测试环境 `ImageFilter.isShaderFilterSupported == false`，实验**显式显示不可用**，不会静默替换成自研毛玻璃。
- 在 Android Impeller 真机上才可评估真实折射/边缘/Fresnel/厚度。
- 已知限制（包 README）：仅 Impeller、形状动画有纹理内存尖峰、单 group 最多 16 形状。
- 许可证：MIT（Copyright 2025 Tim Lehmann / whynotmake.it）。

## 3. 截图

所有截图使用同一 1280×800 逻辑布局、同一演示数据和同一组控件；底色/网格仅切换背景。

- `docs/compare-grid-3way.png`：三种方案同屏对照（renderer 显示显式不可用）。
- `docs/compare-light-2way.png`：自研 vs LGW 浅色背景。
- `docs/compare-cover-2way.png`：自研 vs LGW 彩色封面背景。
- `docs/apple-music-lyrics-vs-none.png`：Apple Music 有歌词/无歌词布局（自研）。
- `docs/custom-dynamic-frames.png`：动态背景两帧（自研；captured-texture 跟随背景 revision）。
- `docs/custom-*.jpg`、`docs/widgets-*.jpg`：原始布局截图。
- `docs/renderer-grid.jpg`：renderer 在 Skia 下的显式不可用状态。

截图来自 Flutter widget test 的 `RepaintBoundary.toImage`，不是 Android 真机截图；色差/性能需以真机为准。

## 4. 结论（基于当前证据）

- **哪个最自然？** 目前无法给最终答案：LGW 的 premium 和 renderer 都必须在 Android Impeller 真机上看；Skia 截图里的 LGW 只是 lightweight 路径（无几何折射），不能与自研真实折射公平比较。
- **哪个最接近 Apple Music？** LGW 的组件生态与 iOS 26 控件（GlassButton/GlassMenu/GlassDialog/滑杆）最接近；但本次截图只验证了布局和按钮/滑杆，菜单/弹窗尚未完成三方案对照。
- **哪个高光最克制？** 当前 Skia 截图中 LGW lightweight 明显更克制；自研仍有明显亮白边缘（与用户反馈一致）。premium/renderer 的最终高光需真机验证。
- **Android 稳定性最好？** 无法确认。自研路径已有真机历史但仍有白边问题；LGW 需 Flutter 升级/patch；renderer 明确要求 Impeller 且未真机验证。
- **适合全项目统一？** 暂不推荐立即替换。建议先在 Android Impeller 真机用本 Playground 跑 LGW premium 与 renderer，记录 Profile FPS/白帧；通过后再评估。
- **是否废弃当前自研内核？** 暂不。自研是唯一已在当前 SDK 上跑通捕获纹理折射的路径；第三方都有 SDK/渲染后端门槛。若真机验证第三方明显更自然，再整体替换。

## 5. 尚未完成/未验证

- 没有真实 Android 设备：未获得 Profile 帧耗时/P95/白帧/发热数据，无法判断 LGW premium 与 renderer 的 Android 稳定性。
- renderer 在当前 Skia 环境不可运行；仅验证了“显式不可用”路径。
- LGW 菜单/弹窗/形状融合尚未完成三方案同屏对照（按钮/滑杆已接入）。
- 实验用的 LGW 是 vendored patch（filterQuality/vector_math），不是官方原样依赖。
- 截图中的动态背景两帧来自 widget test，不是屏幕录制。

## 6. 生产隔离

本次只新增 `experiments/liquid_glass_playground/`；生产 `flutter/`、Shaders、协议、歌词、设置数据结构均未改动。实验不进入 Android 构建，不声明生产完成。
