## FusionPlay Android 1.3.8

### 修复玻璃闪烁（重点）

- 修复「只要有动画（播放进度、菜单展开、歌词滚动等）所有玻璃界面就闪白色底」的问题。
- 根因是 Flutter 3.32 的实验性 `ImageFilter.shader` 玻璃层在多个 `BackdropFilter` 重叠时，会把上一帧（包含自身输出）反馈进下一帧，动画触发重绘后整块玻璃变成不透明白色。上游问题：[flutter/flutter#187820](https://github.com/flutter/flutter/issues/187820)、[#170820](https://github.com/flutter/flutter/issues/170820)。
- 已默认关闭该实验 shader 路径，所有液态玻璃改用稳定的 `BackdropFilter` 毛玻璃 + tint/渐变/描边/高光叠层；外观保持一致，不再闪白。需要实验时可加 `--dart-define=FUSIONPLAY_GLASS_SHADER=true`。

### 同时包含 1.3.7 的界面更新

- 手机横屏布局修复，紧凑 chrome 与 dock 不再重叠。
- 三点圆 morph 成菜单/右侧半屏设置，右上角保持圆弧锚点。
- TV/遥控器适配：Leanback 入口、TV banner、overscan、4px 焦点框、设置默认焦点避开输入框。
- 深色液态玻璃设置、单色开关/勾选、音频来源胶囊、dock 垂直居中、官方 `PinnedHeaderSliver` 标题栏。

### 安装

- 版本 1.3.8 / versionCode 10308，固定 release 签名，可直接覆盖 1.3.7。
- 1.3.4 及以上可在「设置 → 检查更新」直接升级；1.3.3 及更早需手动安装一次。
