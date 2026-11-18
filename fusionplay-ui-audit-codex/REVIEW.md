# FusionPlay 全应用 UI 审查

后续播放器光照与封面调整使用独立的摄影对照页。下文保留 ea3bc78 审查时的测量与截图。[最新封面效果与第三方方案](cover-study/README.md)


**结论：Linux Skia 下 P01 已定位并修复，Android 产品验收仍待真机。** 原始 PR #2 的 lightweight 玻璃滤镜缺少有效的局部裁切。单独补裁切即可恢复同一帧的正常背景。生产路径现在使用共享的真实背景纹理和形状相关折射；网格对照、无 revision 的重绘测试以及不同 DPR 的播放器截图均已纳入回归。详见 [诊断矩阵与截图](COMPARISON.md)。

## 环境与入口

基线为 PR #2 的 e0d82b044f1031bf8c19f674e5ed30024d48900d，两轮历史材料从 main 的 aec10e2 只读检出。Flutter 3.32.8，Dart 3.8.1，Linux x64，Widget 测试使用软件渲染。设备枚举仅有 Linux。所有截图、几何表以及菜单 MP4 来源于测试渲染。MP4 使用 17 ms 的虚拟帧步进，不可作为设备流畅度测量。

| 页面或状态 | 检查内容与证据 |
|---|---|
| 播放器 | P01–P20 包含有无歌词、长标题、缺封面、空播放和暂停。封面及呼吸、元数据、进度及时间、上下曲、主操作、音量、顶部协议与歌词切换均包含在树内。P19 修正为真正传入 null。测量表记录测试找到的元素。 |
| 歌词 | 当前行与淡化行位于独立列；portrait 截图展示窄屏上下分配。歌词可用性、播放时钟、滚动和空状态使用既有 lyrics 系列测试。 |
| 三点菜单 | G03 触发器，G04/G05 展开，menu-open 完成。app_menu 和 remote_focus 覆盖方向键、菜单动作、关闭与返回。 |
| 设置 | G06/G07 显示滚动前后；G08 子卡片。树包含接收器协议、开关、设备身份、名称输入、操作行和更新入口。settings_remote 测试验证键盘焦点。 |
| 浮层 | main.dart 创建错误提示、关于对话框和导出日志动作。update_dialogs.dart 的忙碌进度、版本与长说明、安装授权、成功、失败、取消均归入共享对话框；update_ui_test 覆盖对应状态。源码搜索未找到单独的应用级 SnackBar、PopupMenu 或 bottom-sheet 创建入口。主题定义了 Tooltip 字体。 |
| 手机、平板、TV | 指定十种逻辑尺寸均有播放器截图。DPR 1 和 3 额外测试 1.5 倍字体。遥控器以逻辑键事件驱动，平台通道使用测试替身。 |

## 复现与检查

从仓库根目录运行以下命令。采集脚本需要 Python 和 Pillow，生成菜单视频需要 ffmpeg。

```bash
cd flutter
flutter pub get
flutter analyze
FUSIONPLAY_AUDIT=1 flutter test -r expanded > ../fusionplay-ui-audit-codex/logs/test.txt
cd ..
python3 fusionplay-ui-audit-codex/tools/collect.py
ffmpeg -y -framerate 58.8235 -i /tmp/fusionplay-preview/menu-frames/frame-%02d.png -vf scale=980:420 -c:v libx264 -pix_fmt yuv420p fusionplay-ui-audit-codex/recordings/menu-skia.mp4
```

[完整测试日志](logs/test.txt) · [分析器结果](logs/analyze.txt) · [环境](logs/environment.txt) · [测量表](measurements.csv) · [逐项问题](ISSUES.md) · [修复记录](FIXES.md) · [玻璃架构](GLASS_ARCHITECTURE.md) · [设计规范](DESIGN_SYSTEM.md)

## 设备验收范围

需要在有连续封面切换、拖动、菜单形变、遥控器按键和更新流程的 Android 会话中验证实际 Impeller 后端，记录设备型号、分辨率、刷新率、Flutter profile 帧时和内存，以及光学边缘随时间的表现。Skia 结果不足以确认 GPU 管线预热、长时间功耗、低端 GPU 余量、物理遥控器和接收器端到端播放。新代码保留原生后端调用和播放状态处理。

## 本轮执行结果

依赖解析成功。全量 flutter test 通过 **213 项**。P01–P20 均未报告 RenderFlex overflow。两组 1 px 高度边界的封面中心位移均小于 3 px。Flutter analyze 返回非零，18 条均为 vendored 第三方库的弃用 info，无 error 或 warning。未屏蔽第三方诊断。
