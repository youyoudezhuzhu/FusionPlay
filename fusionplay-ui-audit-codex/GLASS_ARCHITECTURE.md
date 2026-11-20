# 玻璃渲染架构

## 生产路径

`FusionPlayHome` 在一个背景边界内绘制封面、背景模糊和可读性渐变。透明 Scaffold 承载播放器。前景、歌词、控件以及菜单都在边界之外。Navigator 上层的 `GlassBackdropProvider` 共享同一张图片，所以打开更新对话框时也能取得播放器背景。采样的内容是背景美术层；浮层下方的文字、按钮和原生视频表面不在这张纹理里。

`GlassBackdropScope` 观察边界 paint 和显式 revision。封面解码、尺寸变化或绘制动画可以发起采集。请求合并到帧后执行；一次只允许一个 toImage，采集中收到的请求安排尾帧。静态背景不会因为播放时间变化定时截图。两次采集最短间隔 33 ms。持续运动的纹理更新速率理论上限约 30 Hz。菜单形状和 shader 绘制跟随 Flutter 动画帧。关掉高级效果会停止纹理采集。

`liquid_glass_refraction.frag` 是带显式 sampler 的 Paint shader。圆角矩形的有符号距离和法线决定边缘厚度、空间偏移、RGB 色散、掠射高光和吸收。FlutterFragCoord 位于局部逻辑坐标系。绘制时取控件原点和两个单位轴，变换到全局逻辑空间，减去背景原点，再用实际采集倍率和图片宽高换算 UV。描边、指针高光和焦点层位于采样图之上。没有纹理时走形状内 BackdropFilter，shader 加载失败时记录原因并绘制模糊。

## 历史实现与取舍

仓库包含 MIT 许可的 liquid_glass_widgets 0.18.4 源码。原播放器音量、上一曲、下一曲、歌词和菜单触发器使用第三方容器；菜单变形层、错误提示和 Dialog 使用本地实现。原 Dialog 强制关闭折射。协议标签、设置子卡片和行选中态绘制半透明填充。播放主按钮为白色实体。

第三方 AdaptiveGlass 检查质量等级、平台、原生表面和 shader-filter 支持。premium 条件满足时可使用多层采集和 ImageFilter.shader；Skia 软件测试进入 lightweight 路径，其渲染对象将模糊和饱和度滤镜合成后 pushLayer。useOwnLayer 控制分组策略。单独切换等级和分组均不能消除 P01。仅给该 pushLayer 加局部裁切即可消除复现，见 COMPARISON.md 和 tools/p01-clip-isolation.patch。生产代码统一使用本地 Paint shader；保留了 vendored 源码、依赖声明和许可证。截图实验运行版本为 Flutter 3.32.8。本次没有评估更新版上游库的发布频率。

## 成本和验证范围

所有玻璃共享一张背景采样图。每个可见玻璃有各自的 shader 实例，FragmentProgram 首次加载后共享。单张纹理像素预算 2,097,152，采样倍率上限 2，RGBA8 约 8 MiB。换图时新旧资源短时共存，实际内存还包括边界层、滤镜中间缓冲和图片缓存。shader 在控件销毁时释放；旧图片延后一帧释放。当前没有启动时的 GPU 管线预热或按帧耗自动调档。

网格的有/无折射像素差见 glass_optics_test。backdrop_update_test 检查不变 revision 下重绘后纹理像素更新、非零背景原点和空闲时采集次数。Android manifest 没有指定 Impeller 开关；设备、系统和 Flutter 引擎共同决定实际后端。Android Shader 执行、GPU 帧时间、峰值内存及高频连续动画仍需要设备测量。
