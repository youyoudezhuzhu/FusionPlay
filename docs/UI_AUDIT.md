# FusionPlay Flutter UI 审计（1.7.0）

## 组件/页面清单

| 文件 | 用户可见区域 | 设计系统 | 状态 |
|---|---|---|---|
| `lib/main.dart` | 应用外壳、背景、错误 toast、设置/更新/关于入口 | `GlassBackdropScope` + `LiquidGlass` | 已接入真实折射背景；toast 使用 glass |
| `lib/ui/player_view.dart` | 播放页、有/无歌词、加载、进度、音量、菜单触发 | 统一 `LiquidGlass` + Apple Music 布局 | 已重构；歌词不再使用大玻璃卡 |
| `lib/ui/lyrics_view.dart` | 歌词同步/逐字/静态/空态/加载 | 无独立玻璃；由播放页承载 | 保留原数据流与动画 |
| `lib/ui/lyrics_playback_clock.dart` | 歌词时间轴 | 逻辑层 | 未改 |
| `lib/ui/app_menu.dart` | 三点菜单、设置右半屏 morph 面板 | `LiquidGlass` panel + `GlassSurface` 内部行 | 已统一；降低 opaque tint 使折射可见 |
| `lib/ui/settings_view.dart` | 设置全部分组、开关、输入、身份选择、动作按钮 | `_GlassStyle` token + 外层 `LiquidGlass` | 已审查；无 Material Card/AlertDialog |
| `lib/ui/update_dialogs.dart` | 检查更新/下载/安装/权限/错误 | `showGlassDialog`/`GlassDialog` | 已统一玻璃对话框 |
| `lib/ui/liquid_glass.dart` | 全局材质与弹窗 | 真实折射 + 明确 fallback | 重写渲染路径 |
| `lib/ui/glass_backdrop.dart` | 背景纹理捕获/共享 | 新增 | 无自反馈 |
| `lib/ui/remote_focus.dart` | 焦点高亮/遥控器 | 统一 focus frame | 保留 |
| `lib/ui/rounded_progress_bar.dart` | 进度/音量滑杆 | 统一焦点/颜色参数 | 保留 |
| `lib/ui/artwork.dart` | 封面/背景 | 非玻璃；提供背景 | 保留 |
| `lib/ui/vector_icon.dart` | 图标 | 非玻璃 | 保留 |
| `lib/ui/animated_press_scale.dart` | 按压反馈 | 统一曲线 | 保留 |
| `lib/ui/g2_shape.dart` | 连续圆角 | 统一形状 token | 保留 |
| `lib/ui/player_layout.dart` | 旧布局 helper | — | 已删除（无调用方） |
| `shaders/liquid_glass.frag` | 旧 ImageFilter shader | — | 已删除；不再启用反馈路径 |
| `shaders/liquid_glass_refraction.frag` | 真实折射 shader | 新 | 已接入 |

## 明确保留的业务能力

- AirPlay/MiPlay/DLNA 状态、元数据、封面、进度、音量、静音。
- `PlaybackControls` 的能力判断：不支持的上一曲/下一曲/seek 不显示或不发送。
- 歌词数据模型、所有歌词模式、AirPlay 标题歌词识别。
- 菜单/设置/检查更新/导出日志/关于回调、焦点链、返回键、reduce motion。
- 设置持久化与原生 controller 调用未改。

## 未完成/未验证项（不能宣称全项目验收完成）

- **没有真实 Android 设备**：未做 Profile 模式 60 FPS、Impeller 白帧/发热测试。
- **未逐页截图所有状态**：已有播放/设置/菜单/关于弹窗/光学验证/加载/空态截图；
  更新 busy/progress/权限错误、错误 toast、极窄分屏等状态仍缺独立截图。
- **动态背景/滚动背景**：背景纹理按 revision 重捕；播放页背景稳定，滚动内容在玻璃后的逐帧折射未做专门真机验证。
- **低端 GPU 自动降级**：shader 编译/运行失败会落到毛玻璃，但没有按设备型号的自动质量分级与日志上报。
- **全项目设计 token 仍未完全集中**：设置页 `_GlassStyle` 与播放器 inline 颜色仍存在页面级 token；视觉统一但未形成单一全局 token 文件。
- **真实设备上的折射性能**：需要实际安装 1.7.0 后验证，不能以 widget test 代替。
