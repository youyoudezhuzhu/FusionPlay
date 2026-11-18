import 'dart:math' as math;
import 'dart:ui' as ui;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'glass_backdrop.dart';
import 'remote_focus.dart';

/// Unified liquid-glass material levels.
///
/// All glass surfaces in the app resolve their tint/blur/edge/sheen parameters
/// from these tokens so pages no longer hard-code their own glass colours.
///
/// - [card]    Level 1: plain content cards / small chips (light tint, soft rim)
/// - [control] Level 2: interactive controls (buttons, toggles, dock controls)
/// - [panel]   Level 3: menus, sheets, dialogs (deep tint, stronger rim)
enum GlassLevel { card, control, panel, popup }

@immutable
class GlassSpec {
  const GlassSpec({
    required this.tint,
    required this.blur,
    required this.radius,
    required this.borderAlpha,
    required this.rimAlpha,
    required this.edgeAlpha,
    required this.sheenAlpha,
    required this.shadowAlpha,
    required this.shadowBlur,
    required this.shadowOffsetY,
    this.gradient,
    this.refraction = .85,
    this.specular = .30,
    this.thickness = 14,
  });

  final double refraction;
  final double specular;
  final double thickness;
  final Color tint;
  final double blur;
  final double radius;
  final double borderAlpha;

  /// Strength of the inner rim highlight along the top/left edge.
  final double rimAlpha;

  /// Strength of the inner dark edge along the bottom/right edge.
  final double edgeAlpha;

  /// Dynamic pointer/press sheen strength.
  final double sheenAlpha;
  final double shadowAlpha;
  final double shadowBlur;
  final double shadowOffsetY;
  final Gradient? gradient;

  static const card = GlassSpec(
    tint: Color(0x1FFFFFFF),
    blur: 18,
    radius: 22,
    borderAlpha: .14,
    rimAlpha: .34,
    edgeAlpha: .10,
    sheenAlpha: .30,
    shadowAlpha: .16,
    shadowBlur: 20,
    shadowOffsetY: 6,
  );

  static const control = GlassSpec(
    tint: Color(0x24FFFFFF),
    blur: 16,
    radius: 22,
    borderAlpha: .22,
    rimAlpha: .52,
    edgeAlpha: .14,
    sheenAlpha: .42,
    // Soft ambient diffusion only: no hard dark plinth under round controls.
    shadowAlpha: .07,
    shadowBlur: 22,
    shadowOffsetY: 3,
  );

  static const panel = GlassSpec(
    // Overlay chrome: tinted depth without a thick white 3D rim. The player
    // controls no longer use this spec, so it is tuned for menus/settings.
    tint: Color(0xA814191F),
    blur: 24,
    radius: 22,
    borderAlpha: .08,
    rimAlpha: .10,
    edgeAlpha: .10,
    sheenAlpha: .12,
    shadowAlpha: .22,
    shadowBlur: 26,
    shadowOffsetY: 10,
    gradient: LinearGradient(
      begin: Alignment.topLeft,
      end: Alignment.bottomRight,
      colors: [Color(0x0AFFFFFF), Color(0x00000000)],
    ),
  );

  static GlassSpec of(GlassLevel level) => switch (level) {
    GlassLevel.card => card,
    GlassLevel.control => control,
    GlassLevel.panel || GlassLevel.popup => panel,
  };
}

/// Liquid-glass surface with real screen-space refraction.
///
/// When a [GlassBackdropScope] is available, the surface samples its captured,
/// glass-free backdrop texture through `liquid_glass_refraction.frag`; the SDF
/// normal displaces UVs around the actual shape, so grid lines/text behind the
/// rim visibly bend. This path deliberately does **not** use
/// `ImageFilter.shader`, which caused the Android Impeller white-feedback bug.
///
/// Without a scope (tests, desktop previews, low-end fallback) the widget uses
/// the stable `BackdropFilter` frosted path. The fallback is marked as such and
/// is not claimed to be refractive.
///
/// Dynamic highlights are pointer/press driven only: pointer events update a
/// [ValueNotifier] consumed by a [CustomPaint], so no ticker/setState runs
/// while idle or during animations.
class LiquidGlass extends StatefulWidget {
  const LiquidGlass({
    super.key,
    required this.child,
    this.level = GlassLevel.control,
    this.interactive = false,
    this.borderRadius = 24,
    this.blurSigma,
    this.tint,
    this.tintGradient,
    this.refraction,
    this.specular,
    this.thickness,
    this.absorption = 0.35,
    this.animate = false,
    this.forceFallback = false,
    this.padding,
    this.clipBehavior = Clip.antiAlias,
  });

  final Widget child;

  /// Material level; supplies tint/blur/edge/sheen defaults.
  final GlassLevel level;

  /// Tracks pointer position/press for the dynamic specular sheen. Keep false
  /// for large panels whose content is expensive, unless the sheen is wanted.
  final bool interactive;

  /// Radius in logical pixels; use a large value (e.g. 999) for capsules.
  final double borderRadius;

  /// Backdrop blur sigma; defaults to the [level] token.
  final double? blurSigma;

  /// Glass tint; defaults to the [level] token.
  final Color? tint;

  /// Optional gradient painted over [tint] (e.g. the morph panel's dark sheen).
  final Gradient? tintGradient;

  /// Strength of the real SDF backdrop displacement (0 disables refraction).
  final double? refraction;

  /// Rim/sheen strength multiplier, 0 disables highlights.
  final double? specular;

  /// Optical thickness in logical pixels; controls the bevel width.
  final double? thickness;

  /// Beer-Lambert-style bottom/right edge darkening, 0..1.
  final double absorption;

  /// Reserved for future animated caustics; the current material is static.
  final bool animate;

  /// Explicitly force the stable frosted fallback (tests / compatibility).
  final bool forceFallback;

  final EdgeInsetsGeometry? padding;
  final Clip clipBehavior;

  @override
  State<LiquidGlass> createState() => _LiquidGlassState();
}

@immutable
class _Sheen {
  const _Sheen({this.pointer, this.pressed = false});

  final Offset? pointer;
  final bool pressed;

  _Sheen copyWith({Offset? pointer, bool? pressed, bool clear = false}) =>
      _Sheen(
        pointer: clear ? null : (pointer ?? this.pointer),
        pressed: pressed ?? this.pressed,
      );
}

class _LiquidGlassState extends State<LiquidGlass> {
  final ValueNotifier<_Sheen> _sheen = ValueNotifier(const _Sheen());

  @override
  void dispose() {
    _sheen.dispose();
    super.dispose();
  }

  void _pointer(Offset local, {bool? pressed}) {
    _sheen.value = _sheen.value.copyWith(pointer: local, pressed: pressed);
  }

  @override
  Widget build(BuildContext context) {
    final spec = GlassSpec.of(widget.level);
    final focused = FusionFocusHighlight.of(context);
    final tint = widget.tint ?? spec.tint;
    final blur = widget.blurSigma ?? spec.blur;
    final radius = BorderRadius.circular(widget.borderRadius);
    final child = widget.padding == null
        ? widget.child
        : Padding(padding: widget.padding!, child: widget.child);

    final backdrop = GlassBackdropScope.of(context);
    final useRefraction = backdrop != null && !widget.forceFallback;
    Widget surface = DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: radius,
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: spec.shadowAlpha),
            blurRadius: spec.shadowBlur,
            offset: Offset(0, spec.shadowOffsetY),
          ),
        ],
      ),
      child: ClipRRect(
        borderRadius: radius,
        clipBehavior: widget.clipBehavior,
        child: useRefraction
            ? _RefractiveSurface(
                backdrop: backdrop,
                borderRadius: widget.borderRadius,
                refraction: widget.refraction ?? spec.refraction,
                specular: widget.specular ?? spec.specular,
                thickness: widget.thickness ?? spec.thickness,
                absorption: widget.absorption,
                tint: tint,
                sheen: _sheen,
                spec: spec,
                focused: focused,
                child: child,
              )
            : Stack(
                fit: StackFit.passthrough,
                children: [
                  Positioned.fill(
                    child: BackdropFilter(
                      filter: ui.ImageFilter.blur(sigmaX: blur, sigmaY: blur),
                      child: const SizedBox(),
                    ),
                  ),
                  Positioned.fill(
                    child: IgnorePointer(
                      child: RepaintBoundary(
                        child: CustomPaint(
                          painter: _GlassPaint(
                            sheen: _sheen,
                            spec: spec,
                            tint: tint,
                            gradient: widget.tintGradient ?? spec.gradient,
                            borderRadius: widget.borderRadius,
                            specular: widget.specular ?? spec.specular,
                            absorption: widget.absorption,
                            focused: focused,
                          ),
                        ),
                      ),
                    ),
                  ),
                  child,
                ],
              ),
      ),
    );

    if (!widget.interactive) return surface;

    return Listener(
      behavior: HitTestBehavior.translucent,
      onPointerDown: (event) => _pointer(event.localPosition, pressed: true),
      onPointerMove: (event) => _pointer(event.localPosition),
      onPointerUp: (event) => _pointer(event.localPosition, pressed: false),
      onPointerCancel: (_) => _pointer(const Offset(-1, -1), pressed: false),
      child: MouseRegion(
        onHover: (event) => _pointer(event.localPosition),
        onExit: (_) => _sheen.value = _sheen.value.copyWith(clear: true),
        child: surface,
      ),
    );
  }
}

/// Real screen-space refraction backed by a glass-free captured texture.
///
/// The shader samples [GlassBackdropScope]'s texture with an SDF-shaped normal
/// displacement. Since the texture is captured from a sibling RepaintBoundary
/// that excludes this widget, there is no feedback loop.
class _RefractiveSurface extends StatefulWidget {
  const _RefractiveSurface({
    required this.backdrop,
    required this.borderRadius,
    required this.refraction,
    required this.specular,
    required this.thickness,
    required this.absorption,
    required this.tint,
    required this.sheen,
    required this.spec,
    required this.focused,
    required this.child,
  });

  final GlassBackdropTexture backdrop;
  final double borderRadius;
  final double refraction;
  final double specular;
  final double thickness;
  final double absorption;
  final Color tint;
  final ValueListenable<_Sheen> sheen;
  final GlassSpec spec;
  final bool focused;
  final Widget child;

  @override
  State<_RefractiveSurface> createState() => _RefractiveSurfaceState();
}

class _RefractiveSurfaceState extends State<_RefractiveSurface> {
  static Future<ui.FragmentProgram?>? _programFuture;

  final GlobalKey _key = GlobalKey(debugLabel: 'refractive-glass-surface');
  final ValueNotifier<Rect?> _globalRect = ValueNotifier(null);
  ui.FragmentShader? _shader;

  @override
  void initState() {
    super.initState();
    _loadShader();
    _scheduleGeometry();
  }

  Future<void> _loadShader() async {
    try {
      final program = await (_programFuture ??= ui.FragmentProgram.fromAsset(
        'shaders/liquid_glass_refraction.frag',
      ));
      if (!mounted || program == null) return;
      setState(() => _shader = program.fragmentShader());
      _scheduleGeometry();
    } catch (error) {
      debugPrint('FusionPlay glass shader unavailable, using blur: $error');
    }
  }

  @override
  void didUpdateWidget(covariant _RefractiveSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    _scheduleGeometry();
  }

  void _scheduleGeometry() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final box = _key.currentContext?.findRenderObject();
      if (box is! RenderBox || !box.hasSize) return;
      final origin = box.localToGlobal(Offset.zero);
      final next = origin & box.size;
      if (_globalRect.value != next) _globalRect.value = next;
    });
  }

  @override
  void dispose() {
    _shader?.dispose();
    _globalRect.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final shader = _shader;
    return Stack(
      fit: StackFit.passthrough,
      children: [
        Positioned.fill(
          child: SizedBox.expand(
            key: _key,
            child: RepaintBoundary(
              child: shader == null
                  ? BackdropFilter(
                      filter: ui.ImageFilter.blur(
                        sigmaX: widget.spec.blur,
                        sigmaY: widget.spec.blur,
                      ),
                      child: const SizedBox.expand(),
                    )
                  : ValueListenableBuilder<Rect?>(
                      valueListenable: _globalRect,
                      builder: (context, rect, _) => CustomPaint(
                        painter: _RefractionPainter(
                          shader: shader,
                          backdrop: widget.backdrop,
                          globalRect: rect,
                          borderRadius: widget.borderRadius,
                          refraction: widget.refraction,
                          specular: widget.specular,
                          thickness: widget.thickness,
                          absorption: widget.absorption,
                          tint: widget.tint,
                        ),
                      ),
                    ),
            ),
          ),
        ),
        Positioned.fill(
          child: IgnorePointer(
            child: RepaintBoundary(
              child: CustomPaint(
                painter: _GlassPaint(
                  sheen: widget.sheen,
                  spec: widget.spec,
                  tint: Colors.transparent,
                  gradient: null,
                  borderRadius: widget.borderRadius,
                  specular: widget.specular,
                  absorption: widget.absorption,
                  focused: widget.focused,
                ),
              ),
            ),
          ),
        ),
        widget.child,
      ],
    );
  }
}

class _RefractionPainter extends CustomPainter {
  _RefractionPainter({
    required this.shader,
    required this.backdrop,
    required this.globalRect,
    required this.borderRadius,
    required this.refraction,
    required this.specular,
    required this.thickness,
    required this.absorption,
    required this.tint,
  });

  final ui.FragmentShader shader;
  final GlassBackdropTexture backdrop;
  final Rect? globalRect;
  final double borderRadius;
  final double refraction;
  final double specular;
  final double thickness;
  final double absorption;
  final Color tint;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = globalRect;
    if (rect == null || size.isEmpty) return;
    final texture = backdrop;
    shader
      ..setFloat(0, size.width)
      ..setFloat(1, size.height)
      ..setFloat(2, rect.left - texture.origin.dx)
      ..setFloat(3, rect.top - texture.origin.dy)
      ..setFloat(4, texture.image.width.toDouble())
      ..setFloat(5, texture.image.height.toDouble())
      ..setFloat(6, texture.pixelRatio)
      ..setFloat(7, borderRadius)
      ..setFloat(8, refraction)
      ..setFloat(9, thickness)
      ..setFloat(10, specular)
      ..setFloat(11, absorption)
      ..setFloat(12, tint.r)
      ..setFloat(13, tint.g)
      ..setFloat(14, tint.b)
      ..setFloat(15, tint.a);
    shader.setImageSampler(0, texture.image);
    canvas.drawRect(Offset.zero & size, Paint()..shader = shader);
  }

  @override
  bool shouldRepaint(_RefractionPainter oldDelegate) =>
      oldDelegate.shader != shader ||
      oldDelegate.backdrop != backdrop ||
      oldDelegate.globalRect != globalRect ||
      oldDelegate.borderRadius != borderRadius ||
      oldDelegate.refraction != refraction ||
      oldDelegate.specular != specular ||
      oldDelegate.thickness != thickness ||
      oldDelegate.absorption != absorption ||
      oldDelegate.tint != tint;
}

class _GlassPaint extends CustomPainter {
  _GlassPaint({
    required this.sheen,
    required this.spec,
    required this.tint,
    required this.gradient,
    required this.borderRadius,
    required this.specular,
    required this.absorption,
    required this.focused,
  }) : super(repaint: sheen);

  final ValueListenable<_Sheen> sheen;
  final GlassSpec spec;
  final Color tint;
  final Gradient? gradient;
  final double borderRadius;
  final double specular;
  final double absorption;
  final bool focused;

  @override
  void paint(Canvas canvas, Size size) {
    if (size.isEmpty) return;
    final rect = Offset.zero & size;
    final rrect = RRect.fromRectAndRadius(rect, Radius.circular(borderRadius));
    final strength = (specular * (focused ? 1.25 : 1.0)).clamp(0.0, 2.0);

    // 1. Tint / gradient body.
    canvas.drawRRect(rrect, Paint()..color = tint);
    final gradient = this.gradient;
    if (gradient != null) {
      canvas.drawRRect(rrect, Paint()..shader = gradient.createShader(rect));
    }

    // 1b. Optical body shading: a soft top-left lift and bottom-right
    // absorption give the pane a thickness gradient before the rim is drawn.
    canvas.drawRRect(
      rrect,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Colors.white.withValues(alpha: .085 * strength),
            Colors.white.withValues(alpha: .012 * strength),
            Colors.black.withValues(alpha: .10 * absorption),
          ],
          stops: const [0.0, .48, 1.0],
        ).createShader(rect),
    );

    // 1c. Edge lens band. This is the high-quality fallback for real
    // screen-space displacement: a thin, bright inner band whose width and
    // opacity follow the glass thickness, like the compressed edge of a lens.
    final lensBand = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = (2.0 + 0.10 * (spec.blur - 14).clamp(0.0, 18.0)).clamp(
        1.6,
        4.0,
      )
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 1.4)
      ..shader = LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [
          Colors.white.withValues(alpha: .19 * strength),
          Colors.white.withValues(alpha: .035 * strength),
          Colors.white.withValues(alpha: .12 * strength),
        ],
        stops: const [0.0, .58, 1.0],
      ).createShader(rect);
    canvas.drawRRect(rrect.deflate(2.2), lensBand);

    // 1d. A faint refracted caustic just inside the bottom edge.
    final caustic = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.0
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 2.4)
      ..shader = LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: [
          Colors.transparent,
          Colors.white.withValues(alpha: .08 * strength),
        ],
      ).createShader(rect);
    canvas.drawRRect(rrect.deflate(4.0), caustic);

    // 2. Dynamic specular highlight: follows the pointer; brightens on press.
    final state = sheen.value;
    final pointer = state.pointer;
    final highlightCenter = pointer != null && pointer.dx >= 0
        ? pointer
        : Offset(size.width * .22, size.height * .06);
    final intensity =
        (spec.sheenAlpha * strength * (state.pressed ? 1.45 : 1.0)).clamp(
          0.0,
          .8,
        );
    if (intensity > 0.001) {
      final radius = size.longestSide * .95;
      final paint = Paint()
        ..shader =
            RadialGradient(
              colors: [
                Colors.white.withValues(alpha: intensity),
                Colors.white.withValues(alpha: intensity * .22),
                Colors.white.withValues(alpha: 0),
              ],
              stops: const [0.0, .45, 1.0],
            ).createShader(
              Rect.fromCircle(center: highlightCenter, radius: radius),
            );
      canvas.drawRRect(rrect, paint);
    }

    // 3. Rim: bright inner edge top/left, fading to a faint bottom/right.
    final rimPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.1
      ..shader = LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [
          Colors.white.withValues(alpha: .58 * strength),
          Colors.white.withValues(alpha: .10 * strength),
          Colors.white.withValues(alpha: .30 * strength),
        ],
        stops: const [0.0, .55, 1.0],
      ).createShader(rect);
    canvas.drawRRect(rrect.deflate(0.55), rimPaint);

    // 4. Subtle bottom/right inner dark edge (depth without a plastic shadow).
    final edgeAlpha = (spec.edgeAlpha * absorption).clamp(0.0, .5);
    final edgePaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.4
      ..shader = LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [
          Colors.transparent,
          Colors.transparent,
          Colors.black.withValues(alpha: edgeAlpha),
        ],
        stops: const [0.0, .55, 1.0],
      ).createShader(rect);
    canvas.drawRRect(rrect.deflate(0.9), edgePaint);
  }

  @override
  bool shouldRepaint(_GlassPaint oldDelegate) =>
      oldDelegate.spec != spec ||
      oldDelegate.tint != tint ||
      oldDelegate.gradient != gradient ||
      oldDelegate.borderRadius != borderRadius ||
      oldDelegate.specular != specular ||
      oldDelegate.absorption != absorption ||
      oldDelegate.focused != focused;
}

/// Token-based translucent glass surface **without** its own backdrop blur.
///
/// Use it for nested layers (headers, rows, chips) that already sit on top of a
/// [LiquidGlass]; stacking another [BackdropFilter] would double-blur and cost
/// performance.
class GlassSurface extends StatelessWidget {
  const GlassSurface({
    super.key,
    required this.child,
    this.level = GlassLevel.card,
    this.borderRadius = 20,
    this.tint,
    this.borderAlpha,
    this.raise = false,
    this.padding,
  });

  final Widget child;
  final GlassLevel level;
  final double borderRadius;
  final Color? tint;
  final double? borderAlpha;
  final bool raise;
  final EdgeInsetsGeometry? padding;

  @override
  Widget build(BuildContext context) {
    final spec = GlassSpec.of(level);
    final radius = BorderRadius.circular(borderRadius);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: tint ?? spec.tint.withValues(alpha: .10),
        borderRadius: radius,
        border: Border.all(
          color: Colors.white.withValues(alpha: borderAlpha ?? .10),
          width: 1,
        ),
        boxShadow: raise
            ? <BoxShadow>[
                BoxShadow(
                  color: Colors.black.withValues(alpha: .18),
                  blurRadius: 18,
                  offset: const Offset(0, 6),
                ),
              ]
            : null,
      ),
      child: padding == null ? child : Padding(padding: padding!, child: child),
    );
  }
}

/// Full-bleed glass dialog surface used by the about/update dialogs.
///
/// Only transform/opacity animate; the glass itself is static.
class GlassDialog extends StatelessWidget {
  const GlassDialog({
    super.key,
    required this.child,
    this.maxWidth = 400,
    this.padding = const EdgeInsets.fromLTRB(24, 22, 24, 18),
  });

  final Widget child;
  final double maxWidth;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) {
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    final screen = MediaQuery.maybeSizeOf(context) ?? const Size(400, 800);
    return Center(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: maxWidth,
          maxHeight: math.max(160, screen.height - 40),
        ),
        child: TweenAnimationBuilder<double>(
          tween: Tween(begin: 0, end: 1),
          duration: reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 180),
          curve: Curves.easeOutCubic,
          builder: (context, value, child) => Opacity(
            opacity: value,
            child: Transform.scale(scale: .97 + .03 * value, child: child),
          ),
          child: RepaintBoundary(
            child: LiquidGlass(
              level: GlassLevel.popup,
              interactive: true,
              borderRadius: 24,
              padding: padding,
              refraction: .85,
              specular: .18,
              thickness: 8,
              child: Material(
                color: Colors.transparent,
                child: DefaultTextStyle.merge(
                  style: const TextStyle(color: Color(0xEBFFFFFF)),
                  child: child,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Shows a [GlassDialog] with the app's glass styling.
Future<T?> showGlassDialog<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  Key? dialogKey,
  bool barrierDismissible = true,
}) => showDialog<T>(
  context: context,
  barrierDismissible: barrierDismissible,
  barrierColor: Colors.black.withValues(alpha: .42),
  builder: (dialogContext) => GlassDialog(
    key: dialogKey,
    child: Builder(builder: builder),
  ),
);
