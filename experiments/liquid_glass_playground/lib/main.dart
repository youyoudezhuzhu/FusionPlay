// Independent Liquid Glass comparison playground.
//
// This file is intentionally outside the production Flutter app. It renders
// the same background/controls through three unrelated glass stacks:
//   1. FusionPlay current custom material (production package, real refraction)
//   2. liquid_glass_widgets 0.18.4 (AdaptiveGlass/GlassButton/GlassSlider)
//   3. liquid_glass_renderer 0.2.0-dev.4 (Impeller-only; explicit unavailable
//      state on Skia/test backends, never a silent fake-glass replacement)
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:fusionplay_android_flutter/ui/glass_backdrop.dart' as fp;
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart' as fp;
import 'package:liquid_glass_renderer/liquid_glass_renderer.dart' as lgr;
import 'package:liquid_glass_widgets/liquid_glass_widgets.dart' as lgw;

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const GlassPlaygroundApp());
}

enum GlassScheme { custom, widgets, renderer }

enum PlaygroundBackground { dark, light, cover, grid }

extension on GlassScheme {
  String get path => switch (this) {
    GlassScheme.custom => '捕获背景 + SDF FragmentShader（真实位移）',
    GlassScheme.widgets =>
      ui.ImageFilter.isShaderFilterSupported
          ? 'AdaptiveGlass premium / Impeller'
          : 'Skia lightweight shader（无几何折射）',
    GlassScheme.renderer => 'LiquidGlassLayer / 仅 Impeller',
  };

  String get label => switch (this) {
    GlassScheme.custom => 'FusionPlay 自研',
    GlassScheme.widgets => 'liquid_glass_widgets',
    GlassScheme.renderer => 'liquid_glass_renderer',
  };
}

extension on PlaygroundBackground {
  String get label => switch (this) {
    PlaygroundBackground.dark => '深色',
    PlaygroundBackground.light => '浅色',
    PlaygroundBackground.cover => '彩色封面',
    PlaygroundBackground.grid => '高对比网格',
  };
}

class GlassPlaygroundApp extends StatelessWidget {
  const GlassPlaygroundApp({
    super.key,
    this.initialScheme = GlassScheme.custom,
    this.initialBackground = PlaygroundBackground.cover,
    this.initialLyrics = true,
    this.initialDynamic = false,
  });

  final GlassScheme initialScheme;
  final PlaygroundBackground initialBackground;
  final bool initialLyrics;
  final bool initialDynamic;

  @override
  Widget build(BuildContext context) => MaterialApp(
    debugShowCheckedModeBanner: false,
    title: 'Liquid Glass Playground',
    theme: ThemeData(
      brightness: Brightness.dark,
      useMaterial3: true,
      fontFamily: 'MiSans',
      scaffoldBackgroundColor: const Color(0xFF090B10),
    ),
    home: PlaygroundScreen(
      initialScheme: initialScheme,
      initialBackground: initialBackground,
      initialLyrics: initialLyrics,
      initialDynamic: initialDynamic,
    ),
  );
}

class PlaygroundScreen extends StatefulWidget {
  const PlaygroundScreen({
    super.key,
    this.initialScheme = GlassScheme.custom,
    this.initialBackground = PlaygroundBackground.cover,
    this.initialLyrics = true,
    this.initialDynamic = false,
  });

  final GlassScheme initialScheme;
  final PlaygroundBackground initialBackground;
  final bool initialLyrics;
  final bool initialDynamic;

  @override
  State<PlaygroundScreen> createState() => _PlaygroundScreenState();
}

class _PlaygroundScreenState extends State<PlaygroundScreen>
    with SingleTickerProviderStateMixin {
  late GlassScheme scheme = widget.initialScheme;
  late PlaygroundBackground background = widget.initialBackground;
  late bool lyrics = widget.initialLyrics;
  late bool dynamicBackground = widget.initialDynamic;
  late final AnimationController _motion = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 8),
  )..repeat();
  final ScrollController _scroll = ScrollController();

  @override
  void dispose() {
    _motion.dispose();
    _scroll.dispose();
    super.dispose();
  }

  bool get _rendererSupported => ui.ImageFilter.isShaderFilterSupported;

  @override
  Widget build(BuildContext context) {
    final showUnavailable =
        scheme == GlassScheme.renderer && !_rendererSupported;
    return Scaffold(
      body: SafeArea(
        child: Column(
          children: [
            _toolbar(),
            Container(height: 1, color: Colors.white.withValues(alpha: .08)),
            Expanded(
              child: Stack(
                fit: StackFit.expand,
                children: [
                  _AnimatedBackground(
                    mode: background,
                    motion: dynamicBackground ? _motion : null,
                  ),
                  if (showUnavailable)
                    _UnavailablePane(
                      reason: _rendererUnavailableReason(),
                      onUsePrevious: () =>
                          setState(() => scheme = GlassScheme.custom),
                    )
                  else
                    _schemeContent(),
                  Positioned(
                    left: 18,
                    bottom: 12,
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        color: Colors.black.withValues(alpha: .52),
                        borderRadius: BorderRadius.circular(8),
                        border: Border.all(color: Colors.white12),
                      ),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 9,
                          vertical: 5,
                        ),
                        child: Text(
                          '渲染路径: ${scheme.path}',
                          style: const TextStyle(
                            fontSize: 11,
                            color: Colors.white70,
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  String _rendererUnavailableReason() {
    final backend = ui.ImageFilter.isShaderFilterSupported
        ? 'Impeller shader filters'
        : 'Skia/no shader filters';
    return '当前渲染后端：$backend。liquid_glass_renderer 的 LiquidGlassLayer '
        '需要 Android Impeller；此模式不会静默替换为毛玻璃。请在 Android Impeller '
        '真机/Profile 构建中查看。';
  }

  Widget _schemeContent() => switch (scheme) {
    GlassScheme.custom => _customContent(),
    GlassScheme.widgets => _widgetsContent(),
    GlassScheme.renderer => _rendererContent(),
  };

  Widget _customContent() {
    final content = _PlayerLayout(
      scheme: scheme,
      lyrics: lyrics,
      glass: _CustomGlassBuilder(),
      onToggleLyrics: () => setState(() => lyrics = !lyrics),
      scrollController: _scroll,
    );
    return fp.GlassBackdropScope(
      revision:
          '${background.name}|$dynamicBackground|${dynamicBackground ? (_motion.value * 24).floor() : 0}',
      background: _AnimatedBackground(
        mode: background,
        motion: dynamicBackground ? _motion : null,
      ),
      child: content,
    );
  }

  Widget _widgetsContent() => lgw.LiquidGlassWidgets.wrap(
    child: _PlayerLayout(
      scheme: scheme,
      lyrics: lyrics,
      glass: _WidgetsGlassBuilder(),
      onToggleLyrics: () => setState(() => lyrics = !lyrics),
      scrollController: _scroll,
    ),
  );

  Widget _rendererContent() => lgr.LiquidGlassLayer(
    settings: const lgr.LiquidGlassSettings(
      thickness: 20,
      blur: 5,
      glassColor: Color(0x12FFFFFF),
      lightIntensity: .5,
      chromaticAberration: .01,
      refractiveIndex: 1.2,
    ),
    child: _PlayerLayout(
      scheme: scheme,
      lyrics: lyrics,
      glass: _RendererGlassBuilder(),
      onToggleLyrics: () => setState(() => lyrics = !lyrics),
      scrollController: _scroll,
    ),
  );

  Widget _toolbar() => Container(
    padding: const EdgeInsets.fromLTRB(18, 8, 18, 8),
    color: const Color(0xCC0B0E14),
    child: Row(
      children: [
        const Text(
          'Liquid Glass Playground',
          style: TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
        ),
        const SizedBox(width: 20),
        _segmented<GlassScheme>(
          values: GlassScheme.values,
          selected: scheme,
          label: (v) => v.label,
          onSelected: (v) => setState(() => scheme = v),
        ),
        const SizedBox(width: 20),
        _segmented<PlaygroundBackground>(
          values: PlaygroundBackground.values,
          selected: background,
          label: (v) => v.label,
          onSelected: (v) => setState(() => background = v),
        ),
        const Spacer(),
        _toggle('动态背景', dynamicBackground, (v) {
          setState(() => dynamicBackground = v);
        }),
        const SizedBox(width: 14),
        _toggle('歌词', lyrics, (v) => setState(() => lyrics = v)),
      ],
    ),
  );

  Widget _segmented<T>({
    required List<T> values,
    required T selected,
    required String Function(T) label,
    required ValueChanged<T> onSelected,
  }) => Container(
    padding: const EdgeInsets.all(3),
    decoration: BoxDecoration(
      color: Colors.white.withValues(alpha: .07),
      borderRadius: BorderRadius.circular(12),
      border: Border.all(color: Colors.white.withValues(alpha: .08)),
    ),
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final value in values)
          GestureDetector(
            onTap: () => onSelected(value),
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 160),
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
              decoration: BoxDecoration(
                color: value == selected
                    ? Colors.white.withValues(alpha: .18)
                    : Colors.transparent,
                borderRadius: BorderRadius.circular(9),
              ),
              child: Text(
                label(value),
                style: TextViewStyle.size(
                  11.5,
                  color: value == selected ? Colors.white : Colors.white60,
                  weight: value == selected ? FontWeight.w700 : FontWeight.w500,
                ),
              ),
            ),
          ),
      ],
    ),
  );

  Widget _toggle(String label, bool value, ValueChanged<bool> onChanged) =>
      GestureDetector(
        onTap: () => onChanged(!value),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              label,
              style: const TextStyle(fontSize: 12, color: Colors.white70),
            ),
            const SizedBox(width: 6),
            Container(
              width: 32,
              height: 18,
              padding: const EdgeInsets.all(2),
              decoration: BoxDecoration(
                color: value ? Colors.white : Colors.white24,
                borderRadius: BorderRadius.circular(9),
              ),
              child: Align(
                alignment: value ? Alignment.centerRight : Alignment.centerLeft,
                child: Container(
                  width: 14,
                  height: 14,
                  decoration: const BoxDecoration(
                    color: Color(0xFF14171D),
                    shape: BoxShape.circle,
                  ),
                ),
              ),
            ),
          ],
        ),
      );
}

class TextViewStyle {
  static TextStyle size(
    double size, {
    Color color = Colors.white,
    FontWeight weight = FontWeight.w500,
  }) => TextStyle(fontSize: size, color: color, fontWeight: weight);
}

/// Glass-builder abstraction: every scheme receives the same logical control
/// and is responsible for its own native material. No overlay highlights are
/// added here.
abstract class _GlassBuilder {
  Widget control({
    required Widget child,
    required double width,
    required double height,
    double radius = 999,
    bool isButton = false,
  });

  Widget panel({required Widget child, double radius = 28});

  Widget slider({
    required double value,
    required ValueChanged<double> onChanged,
  });

  Widget cover({required Widget child, required double size});
}

class _CustomGlassBuilder implements _GlassBuilder {
  @override
  Widget control({
    required Widget child,
    required double width,
    required double height,
    double radius = 999,
    bool isButton = false,
  }) => SizedBox(
    width: width,
    height: height,
    child: fp.LiquidGlass(
      level: fp.GlassLevel.control,
      borderRadius: radius,
      interactive: isButton,
      refraction: 1.0,
      thickness: 16,
      specular: 1,
      child: Center(child: child),
    ),
  );

  @override
  Widget panel({required Widget child, double radius = 28}) => fp.LiquidGlass(
    level: fp.GlassLevel.panel,
    borderRadius: radius,
    tint: const Color(0x36101419),
    child: child,
  );

  @override
  Widget slider({
    required double value,
    required ValueChanged<double> onChanged,
  }) => Slider(value: value, onChanged: onChanged);

  @override
  Widget cover({required Widget child, required double size}) =>
      SizedBox(width: size, height: size, child: child);
}

class _WidgetsGlassBuilder implements _GlassBuilder {
  static const settings = lgw.LiquidGlassSettings(
    thickness: 18,
    blur: 5,
    glassColor: Color(0x12FFFFFF),
  );

  @override
  Widget control({
    required Widget child,
    required double width,
    required double height,
    double radius = 999,
    bool isButton = false,
  }) {
    if (isButton && radius >= 999) {
      return lgw.GlassButton(
        icon: child,
        onTap: () {},
        width: width,
        height: height,
        useOwnLayer: true,
        quality: lgw.GlassQuality.premium,
        settings: settings,
      );
    }
    return SizedBox(
      width: width,
      height: height,
      child: lgw.AdaptiveGlass(
        shape: lgw.LiquidRoundedSuperellipse(borderRadius: radius),
        settings: settings,
        quality: lgw.GlassQuality.premium,
        useOwnLayer: true,
        child: Center(child: child),
      ),
    );
  }

  @override
  Widget panel({required Widget child, double radius = 28}) =>
      lgw.AdaptiveGlass(
        shape: lgw.LiquidRoundedSuperellipse(borderRadius: radius),
        settings: settings,
        quality: lgw.GlassQuality.premium,
        useOwnLayer: true,
        child: child,
      );

  @override
  Widget slider({
    required double value,
    required ValueChanged<double> onChanged,
  }) => lgw.GlassSlider(value: value, onChanged: onChanged, useOwnLayer: true);

  @override
  Widget cover({required Widget child, required double size}) =>
      SizedBox(width: size, height: size, child: child);
}

class _RendererGlassBuilder implements _GlassBuilder {
  @override
  Widget control({
    required Widget child,
    required double width,
    required double height,
    double radius = 999,
    bool isButton = false,
  }) => SizedBox(
    width: width,
    height: height,
    child: lgr.LiquidGlass(
      shape: lgr.LiquidRoundedSuperellipse(borderRadius: radius),
      glassContainsChild: true,
      child: Center(child: child),
    ),
  );

  @override
  Widget panel({required Widget child, double radius = 28}) => lgr.LiquidGlass(
    shape: lgr.LiquidRoundedSuperellipse(borderRadius: radius),
    glassContainsChild: true,
    child: child,
  );

  @override
  Widget slider({
    required double value,
    required ValueChanged<double> onChanged,
  }) => Slider(value: value, onChanged: onChanged);

  @override
  Widget cover({required Widget child, required double size}) =>
      SizedBox(width: size, height: size, child: child);
}

class _PlayerLayout extends StatefulWidget {
  const _PlayerLayout({
    required this.scheme,
    required this.lyrics,
    required this.glass,
    required this.onToggleLyrics,
    required this.scrollController,
  });

  final GlassScheme scheme;
  final bool lyrics;
  final _GlassBuilder glass;
  final VoidCallback onToggleLyrics;
  final ScrollController scrollController;

  @override
  State<_PlayerLayout> createState() => _PlayerLayoutState();
}

class _PlayerLayoutState extends State<_PlayerLayout> {
  double _progress = .32;
  double _volume = .62;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxHeight < 420;
        final cover = (constraints.maxHeight * (compact ? .34 : .48))
            .clamp(96.0, 330.0)
            .toDouble();
        return Padding(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 18),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                flex: 5,
                child: SingleChildScrollView(
                  controller: widget.scrollController,
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      widget.glass.cover(
                        size: cover,
                        child: Container(
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(26),
                            gradient: const LinearGradient(
                              begin: Alignment.topLeft,
                              end: Alignment.bottomRight,
                              colors: [
                                Color(0xFF3155E8),
                                Color(0xFFE65C3A),
                                Color(0xFFF5C04D),
                              ],
                            ),
                          ),
                          child: const Center(
                            child: Icon(
                              Icons.album_rounded,
                              size: 84,
                              color: Colors.white70,
                            ),
                          ),
                        ),
                      ),
                      const SizedBox(height: 16),
                      const Text(
                        'Midnight Grid',
                        style: TextStyle(
                          fontSize: 28,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 4),
                      const Text(
                        'FusionPlay Demo · 测试背景',
                        style: TextStyle(fontSize: 14, color: Colors.white70),
                      ),
                      const SizedBox(height: 18),
                      _progressBar(),
                      const SizedBox(height: 10),
                      Row(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          _iconButton(Icons.skip_previous_rounded, 44),
                          const SizedBox(width: 14),
                          _iconButton(Icons.pause_rounded, 62),
                          const SizedBox(width: 14),
                          _iconButton(Icons.skip_next_rounded, 44),
                        ],
                      ),
                      const SizedBox(height: 12),
                      Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          const Icon(
                            Icons.volume_up_rounded,
                            size: 18,
                            color: Colors.white70,
                          ),
                          const SizedBox(width: 8),
                          SizedBox(
                            width: 180,
                            child: widget.glass.slider(
                              value: _volume,
                              onChanged: (v) => setState(() => _volume = v),
                            ),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
              SizedBox(width: compact ? 16 : 30),
              if (widget.lyrics)
                Expanded(
                  flex: 6,
                  child: widget.glass.panel(
                    child: Padding(
                      padding: const EdgeInsets.all(26),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: const [
                          Text(
                            '雨后的城市 安静得像一封没寄出的信',
                            style: TextStyle(
                              fontSize: 17,
                              color: Colors.white60,
                            ),
                          ),
                          SizedBox(height: 16),
                          Text(
                            '我把想念折成一只纸船',
                            style: TextStyle(
                              fontSize: 26,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                          SizedBox(height: 16),
                          Text(
                            '顺着路灯漂流到你的窗前',
                            style: TextStyle(
                              fontSize: 18,
                              color: Colors.white60,
                            ),
                          ),
                          SizedBox(height: 12),
                          Text(
                            '如果晚风也恰好经过',
                            style: TextStyle(
                              fontSize: 18,
                              color: const Color(0x80FFFFFF),
                            ),
                          ),
                          SizedBox(height: 12),
                          Text(
                            '请替我说一声晚安',
                            style: TextStyle(
                              fontSize: 18,
                              color: const Color(0x80FFFFFF),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
            ],
          ),
        );
      },
    );
  }

  Widget _iconButton(IconData icon, double size) => widget.glass.control(
    width: size,
    height: size,
    isButton: true,
    child: Icon(icon, size: size * .42, color: Colors.white),
  );

  Widget _progressBar() => Row(
    children: [
      const Text('1:12', style: TextStyle(fontSize: 11, color: Colors.white54)),
      const SizedBox(width: 8),
      Expanded(
        child: Slider(
          value: _progress,
          activeColor: Colors.white,
          inactiveColor: Colors.white24,
          onChanged: (v) => setState(() => _progress = v),
        ),
      ),
      const Text('3:48', style: TextStyle(fontSize: 11, color: Colors.white54)),
    ],
  );
}

class _AnimatedBackground extends StatelessWidget {
  const _AnimatedBackground({required this.mode, this.motion});

  final PlaygroundBackground mode;
  final Animation<double>? motion;

  @override
  Widget build(BuildContext context) {
    if (motion == null) return _StaticBackground(mode: mode);
    return AnimatedBuilder(
      animation: motion!,
      builder: (context, _) =>
          _StaticBackground(mode: mode, phase: motion!.value * 6.28318),
    );
  }
}

class _StaticBackground extends StatelessWidget {
  const _StaticBackground({required this.mode, this.phase = 0});

  final PlaygroundBackground mode;
  final double phase;

  @override
  Widget build(BuildContext context) {
    final light = mode == PlaygroundBackground.light;
    return Stack(
      fit: StackFit.expand,
      children: [
        if (mode == PlaygroundBackground.cover)
          Positioned.fill(
            child: Image.asset(
              'assets/cover.png',
              fit: BoxFit.cover,
              filterQuality: FilterQuality.high,
            ),
          ),
        if (mode == PlaygroundBackground.cover)
          const DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Color(0x3D070A0E), Color(0x99070A0E)],
              ),
            ),
          )
        else
          DecoratedBox(
            decoration: BoxDecoration(
              color: light ? const Color(0xFFF2EFEA) : null,
              gradient: mode == PlaygroundBackground.light
                  ? null
                  : const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        Color(0xFF29304A),
                        Color(0xFF121A26),
                        Color(0xFF07090D),
                      ],
                    ),
            ),
          ),
        if (mode == PlaygroundBackground.grid ||
            mode == PlaygroundBackground.cover)
          CustomPaint(
            painter: _GridPainter(light: light, phase: phase),
          ),
      ],
    );
  }
}

class _GridPainter extends CustomPainter {
  const _GridPainter({required this.light, required this.phase});

  final bool light;
  final double phase;

  @override
  void paint(Canvas canvas, Size size) {
    final base = Paint()
      ..color = (light ? Colors.black : Colors.white).withValues(alpha: .10);
    for (var x = -size.height; x < size.width + size.height; x += 32) {
      canvas.drawLine(
        Offset(x + phase * 8, 0),
        Offset(x - size.height + phase * 8, size.height),
        base,
      );
    }
    for (var y = 0.0; y < size.height; y += 32) {
      canvas.drawLine(Offset(0, y), Offset(size.width, y), base);
    }
    final ring = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2
      ..color = (light ? Colors.black : Colors.white).withValues(alpha: .18);
    canvas.drawCircle(Offset(size.width * .72, size.height * .48), 120, ring);
  }

  @override
  bool shouldRepaint(covariant _GridPainter oldDelegate) =>
      oldDelegate.phase != phase || oldDelegate.light != light;
}

class _UnavailablePane extends StatelessWidget {
  const _UnavailablePane({required this.reason, required this.onUsePrevious});

  final String reason;
  final VoidCallback onUsePrevious;

  @override
  Widget build(BuildContext context) => Center(
    child: Container(
      constraints: const BoxConstraints(maxWidth: 560),
      padding: const EdgeInsets.all(28),
      decoration: BoxDecoration(
        color: const Color(0xCC12161D),
        borderRadius: BorderRadius.circular(24),
        border: Border.all(color: const Color(0x33FFFFFF)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'liquid_glass_renderer 不可用',
            style: TextStyle(fontSize: 22, fontWeight: FontWeight.w700),
          ),
          const SizedBox(height: 10),
          Text(
            reason,
            style: const TextStyle(
              fontSize: 14,
              height: 1.5,
              color: Colors.white70,
            ),
          ),
          const SizedBox(height: 18),
          TextButton(
            onPressed: onUsePrevious,
            child: const Text('返回 FusionPlay 自研对照'),
          ),
        ],
      ),
    ),
  );
}
