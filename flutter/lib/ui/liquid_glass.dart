import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

/// Real liquid-glass surface: backdrop blur + a shader pass that bends the
/// backdrop like a lens, adds chromatic fringing, a rim light and a slow sheen.
///
/// The shader path requires Impeller (Android/iOS on current Flutter). When it
/// is unavailable (e.g. Windows/Skia, tests, shader load failure) the widget
/// falls back to a frosted-blur surface with the same tint/edge treatment so
/// call sites look identical.
///
/// Typical use:
/// ```dart
/// LiquidGlass(
///   borderRadius: 999,
///   blurSigma: 14,
///   child: IconButton(...),
/// )
/// ```
class LiquidGlass extends StatefulWidget {
  const LiquidGlass({
    super.key,
    required this.child,
    this.borderRadius = 24,
    this.blurSigma = 16,
    this.tint = const Color(0x1FFFFFFF),
    this.refraction = 1.0,
    this.specular = 1.0,
    this.thickness = 16.0,
    this.absorption = 0.35,
    this.animate = true,
    this.forceFallback = false,
    this.padding,
    this.clipBehavior = Clip.antiAlias,
  });

  final Widget child;

  /// Radius in logical pixels; use a large value (e.g. 999) for capsules.
  final double borderRadius;

  /// Backdrop blur sigma. The shader receives the blurred backdrop.
  final double blurSigma;

  /// Glass tint painted over the refracted backdrop.
  final Color tint;

  /// 0 disables lens bending, 1 is the tuned default, >1 exaggerates.
  final double refraction;

  /// Rim light / sheen strength, 0 disables highlights.
  final double specular;

  /// Bevel depth in logical pixels: how far the rim refracts the backdrop.
  final double thickness;

  /// Beer-Lambert rim darkening, 0 (flat) .. 1 (fully dark edge).
  final double absorption;

  /// Animate the liquid wobble/sheen. Disabled under reduce-motion by the
  /// call site or automatically when [forceFallback] is used.
  final bool animate;

  /// Force the non-shader frosted fallback (tests, Windows).
  final bool forceFallback;

  final EdgeInsetsGeometry? padding;
  final Clip clipBehavior;

  @override
  State<LiquidGlass> createState() => _LiquidGlassState();
}

class _LiquidGlassState extends State<LiquidGlass>
    with SingleTickerProviderStateMixin {
  static Future<ui.FragmentProgram?>? _programFuture;
  static ui.FragmentShader? _sharedShader;

  Ticker? _ticker;
  double _time = 0;
  ui.ImageFilter? _shaderFilter;

  static Future<ui.FragmentProgram?> _loadProgram() {
    return _programFuture ??= () async {
      try {
        return await ui.FragmentProgram.fromAsset('shaders/liquid_glass.frag');
      } catch (_) {
        return null;
      }
    }();
  }

  @override
  void initState() {
    super.initState();
    if (widget.forceFallback) return;
    _initialiseShader();
  }

  Future<void> _initialiseShader() async {
    if (!ui.ImageFilter.isShaderFilterSupported) return;
    final program = await _loadProgram();
    if (!mounted || program == null) return;
    final shader = _sharedShader ??= program.fragmentShader();
    try {
      final filter = ui.ImageFilter.shader(shader);
      setState(() => _shaderFilter = filter);
      _startTicker();
    } catch (_) {
      // Fall through to the frosted fallback.
    }
  }

  void _startTicker() {
    if (!widget.animate || _ticker != null) return;
    final reduceMotion = MediaQuery.maybeDisableAnimationsOf(context) ?? false;
    if (reduceMotion) return;
    _ticker = createTicker((elapsed) {
      if (!mounted) return;
      setState(() {
        _time = elapsed.inMilliseconds / 1000.0;
      });
    })..start();
  }

  @override
  void didUpdateWidget(covariant LiquidGlass oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.animate && !widget.animate) {
      _ticker?.dispose();
      _ticker = null;
    } else if (!oldWidget.animate && widget.animate) {
      _startTicker();
    }
  }

  @override
  void dispose() {
    _ticker?.dispose();
    super.dispose();
  }

  void _paintShaderUniforms(ui.FragmentShader shader, Size size, double dpr) {
    // The engine fills uniform 0 (vec2 size) and the sampler automatically for
    // ImageFilter.shader; the rest follow declaration order.
    // Flat float indices: uSize occupies 0/1, then the scalars, then uLight.
    // ImageFilter.shader textures are sized in physical pixels; the radius must
    // use the same unit so the lens rim stays at the visible corner.
    shader.setFloat(0, size.width * dpr);
    shader.setFloat(1, size.height * dpr);
    shader.setFloat(2, dpr);
    shader.setFloat(3, widget.borderRadius);
    shader.setFloat(4, widget.refraction);
    shader.setFloat(5, widget.specular);
    shader.setFloat(6, _time);
    shader.setFloat(7, 0.42);
    shader.setFloat(8, 0.86);
    shader.setFloat(9, widget.thickness);
    shader.setFloat(10, widget.absorption);
  }

  Widget _buildSurface(Size size) {
    final radius = BorderRadius.circular(widget.borderRadius);
    ui.ImageFilter? filter;
    if (_shaderFilter != null && !widget.forceFallback) {
      final shader = _sharedShader;
      if (shader != null) {
        _paintShaderUniforms(
          shader,
          size,
          MediaQuery.devicePixelRatioOf(context),
        );
      }
      filter = ui.ImageFilter.compose(
        inner: ui.ImageFilter.blur(
          sigmaX: widget.blurSigma,
          sigmaY: widget.blurSigma,
        ),
        outer: _shaderFilter!,
      );
    }

    final content = widget.padding == null
        ? widget.child
        : Padding(padding: widget.padding!, child: widget.child);

    return DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: radius,
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.22),
            blurRadius: 28,
            offset: const Offset(0, 10),
          ),
        ],
      ),
      child: ClipRRect(
        borderRadius: radius,
        clipBehavior: widget.clipBehavior,
        child: Stack(
          fit: StackFit.passthrough,
          children: [
            if (filter != null)
              Positioned.fill(
                child: BackdropFilter(filter: filter, child: const SizedBox()),
              )
            else
              Positioned.fill(
                child: BackdropFilter(
                  filter: ui.ImageFilter.blur(
                    sigmaX: widget.blurSigma,
                    sigmaY: widget.blurSigma,
                  ),
                  child: const SizedBox(),
                ),
              ),
            Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: radius,
                  color: widget.tint,
                  gradient: const LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: [Color(0x26FFFFFF), Color(0x08FFFFFF)],
                  ),
                ),
              ),
            ),
            Positioned.fill(
              child: IgnorePointer(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    borderRadius: radius,
                    border: Border.all(
                      color: Colors.white.withValues(alpha: 0.22),
                      width: 1.2,
                    ),
                  ),
                ),
              ),
            ),
            // Inner highlight along the top edge.
            Positioned(
              left: 8,
              right: 8,
              top: 1,
              height: 1.2,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(999),
                  gradient: LinearGradient(
                    colors: [
                      Colors.white.withValues(alpha: 0.0),
                      Colors.white.withValues(alpha: 0.55),
                      Colors.white.withValues(alpha: 0.0),
                    ],
                  ),
                ),
              ),
            ),
            content,
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final size = Size(
          constraints.hasBoundedWidth ? constraints.maxWidth : 200,
          constraints.hasBoundedHeight ? constraints.maxHeight : 200,
        );
        return _buildSurface(size);
      },
    );
  }
}
