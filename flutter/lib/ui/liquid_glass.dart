import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'remote_focus.dart';

/// Unified liquid-glass material levels.
///
/// All glass surfaces in the app resolve their tint/blur/edge/sheen parameters
/// from these tokens so pages no longer hard-code their own glass colours.
///
/// - [card]    Level 1: plain content cards / small chips (light tint, soft rim)
/// - [control] Level 2: interactive controls (buttons, toggles, dock controls)
/// - [panel]   Level 3: menus, sheets, dialogs (deep tint, stronger rim)
enum GlassLevel { card, control, panel }

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
  });

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
    shadowAlpha: .22,
    shadowBlur: 24,
    shadowOffsetY: 8,
  );

  static const panel = GlassSpec(
    tint: Color(0xE014191F),
    blur: 24,
    radius: 26,
    borderAlpha: .20,
    rimAlpha: .40,
    edgeAlpha: .22,
    sheenAlpha: .30,
    shadowAlpha: .34,
    shadowBlur: 30,
    shadowOffsetY: 12,
    gradient: LinearGradient(
      begin: Alignment.topLeft,
      end: Alignment.bottomRight,
      colors: [Color(0x16FFFFFF), Color(0x04000000)],
    ),
  );

  static GlassSpec of(GlassLevel level) => switch (level) {
    GlassLevel.card => card,
    GlassLevel.control => control,
    GlassLevel.panel => panel,
  };
}

/// Stable frosted liquid-glass surface.
///
/// Implementation note: this intentionally uses a single [BackdropFilter]
/// (Gaussian blur) plus painted tint/rim/sheen — **not** `ImageFilter.shader`
/// refraction. The experimental shader path caused white-frame feedback on
/// Android Impeller (flutter/flutter#187820, #170820) and stays disabled; this
/// widget must not be described as real screen-space refraction.
///
/// The dynamic highlight is pointer/press driven only: pointer events update a
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
    this.refraction = 1.0,
    this.specular = 1.0,
    this.thickness = 16.0,
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

  /// Kept for API compatibility with the (disabled) shader prototype.
  final double refraction;

  /// Rim/sheen strength multiplier, 0 disables highlights.
  final double specular;

  /// Kept for API compatibility with the (disabled) shader prototype.
  final double thickness;

  /// Beer-Lambert-style bottom/right edge darkening, 0..1.
  final double absorption;

  /// Kept for API compatibility; glass is static (no ticker) by design.
  final bool animate;

  /// Kept for API compatibility: stable blur fallback is the only path now.
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
        child: Stack(
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
                      specular: widget.specular,
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
              level: GlassLevel.panel,
              interactive: true,
              borderRadius: 26,
              padding: padding,
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
