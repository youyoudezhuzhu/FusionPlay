import 'dart:math' as math;
import 'dart:ui' as ui;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../theme/fusion_theme.dart';
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
    required this.solidColor,
    required this.blur,
    required this.radius,
    required this.borderAlpha,
    required this.rimAlpha,
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
  final Color solidColor;
  final double blur;
  final double radius;
  final double borderAlpha;

  /// Strength of the inner rim highlight along the top/left edge.
  final double rimAlpha;

  /// Dynamic pointer/press sheen strength.
  final double sheenAlpha;
  final double shadowAlpha;
  final double shadowBlur;
  final double shadowOffsetY;
  final Gradient? gradient;

  static const card = GlassSpec(
    tint: Color(0x17FFFFFF),
    solidColor: Color(0xFF33333E),
    blur: 18,
    radius: 22,
    borderAlpha: .14,
    rimAlpha: .34,
    sheenAlpha: .30,
    shadowAlpha: .035,
    shadowBlur: 20,
    shadowOffsetY: 6,
  );

  static const control = GlassSpec(
    tint: Color(0x1BFFFFFF),
    solidColor: Color(0xFF444450),
    blur: 16,
    radius: 22,
    borderAlpha: .22,
    rimAlpha: .52,
    sheenAlpha: .42,
    // Soft ambient diffusion only: no hard dark plinth under round controls.
    shadowAlpha: .025,
    shadowBlur: 22,
    shadowOffsetY: 3,
  );

  static const panel = GlassSpec(
    // Overlay chrome: tinted depth without a thick white 3D rim. The player
    // controls no longer use this spec, so it is tuned for menus/settings.
    tint: FusionGlassColors.popupTint,
    solidColor: FusionGlassColors.solidPanel,
    blur: FusionGlassColors.panelBlur,
    radius: 22,
    borderAlpha: .08,
    rimAlpha: .10,
    sheenAlpha: .12,
    shadowAlpha: .06,
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
/// Pointer pressure updates only the optical and sheen painters. A short
/// release animation settles the lens, then the material stays idle.
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
    this.lensPower = 0,
    this.animate = false,
    this.forceFallback = false,
    this.padding,
    this.clipBehavior = Clip.antiAlias,
  });

  const LiquidGlass.droplet({Key? key, Color tint = const Color(0x45FFFFFF)})
    : this(
        key: key,
        borderRadius: 999,
        refraction: .78,
        thickness: 4,
        specular: .42,
        lensPower: .57,
        tint: tint,
        child: const SizedBox.expand(),
      );

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

  /// Fraction of the distance toward the lens centre sampled through its dome.
  final double lensPower;

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
  const _Sheen([this.pointer, this.pressure = 0]);

  final Offset? pointer;
  final double pressure;
}

class _LiquidGlassState extends State<LiquidGlass>
    with SingleTickerProviderStateMixin {
  final ValueNotifier<_Sheen> _sheen = ValueNotifier(const _Sheen());
  AnimationController? _release;

  void _decay() {
    final pressure = _release!.value;
    _sheen.value = pressure == 0
        ? const _Sheen()
        : _Sheen(_sheen.value.pointer, pressure);
  }

  @override
  void dispose() {
    _release?.dispose();
    _sheen.dispose();
    super.dispose();
  }

  void _pointer(Offset local, double strength) {
    _release?.stop();
    _sheen.value = _Sheen(local, strength);
  }

  void _lift() {
    if (_sheen.value.pointer == null || (_release?.isAnimating ?? false)) {
      return;
    }
    if (MediaQuery.disableAnimationsOf(context)) {
      _sheen.value = const _Sheen();
      return;
    }
    final release = _release ??= (AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 150),
    )..addListener(_decay));
    release.value = _sheen.value.pressure;
    release.animateTo(0, curve: Curves.easeOutCubic);
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

    final enabled = GlassAppearance.enabledOf(context);
    final backdrop = enabled ? GlassBackdropScope.of(context) : null;
    final Widget body;
    if (!enabled) {
      body = ColoredBox(color: spec.solidColor);
    } else if (backdrop != null && !widget.forceFallback) {
      body = _RefractiveSurface(
        backdrop: backdrop,
        borderRadius: widget.borderRadius,
        refraction: widget.refraction ?? spec.refraction,
        specular: widget.specular ?? spec.specular,
        thickness: widget.thickness ?? spec.thickness,
        absorption: widget.absorption,
        blur: blur,
        frostLiveFace: spec == GlassSpec.panel,
        lensPower: widget.lensPower,
        contact: _sheen,
      );
    } else {
      body = BackdropFilter(
        filter: ui.ImageFilter.blur(sigmaX: blur, sigmaY: blur),
        child: const SizedBox.expand(),
      );
    }

    final surface = DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: radius,
        boxShadow: enabled
            ? [
                BoxShadow(
                  color: Colors.black.withValues(alpha: spec.shadowAlpha),
                  blurRadius: spec.shadowBlur,
                  offset: Offset(0, spec.shadowOffsetY),
                ),
              ]
            : null,
      ),
      child: ClipRRect(
        borderRadius: radius,
        clipBehavior: widget.clipBehavior,
        child: Stack(
          fit: StackFit.passthrough,
          children: [
            Positioned.fill(child: body),
            Positioned.fill(
              child: enabled
                  ? IgnorePointer(
                      child: RepaintBoundary(
                        child: CustomPaint(
                          painter: _GlassPaint(
                            sheen: _sheen,
                            spec: spec,
                            tint: tint,
                            gradient: widget.tintGradient ?? spec.gradient,
                            borderRadius: widget.borderRadius,
                            specular: widget.specular ?? spec.specular,
                            focused: focused,
                          ),
                        ),
                      ),
                    )
                  : const SizedBox.shrink(),
            ),
            // Keep the content mounted when the texture arrives or the mode
            // changes. Settings focus, text editing and scrolling live here.
            child,
          ],
        ),
      ),
    );

    if (!widget.interactive) return surface;

    return Listener(
      behavior: HitTestBehavior.translucent,
      onPointerDown: (event) => _pointer(event.localPosition, 1),
      onPointerMove: (event) => _pointer(event.localPosition, 1),
      onPointerUp: (_) => _lift(),
      onPointerCancel: (_) => _lift(),
      child: MouseRegion(
        onHover: (event) => _pointer(event.localPosition, .28),
        onExit: (_) => _lift(),
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
    required this.blur,
    required this.frostLiveFace,
    required this.lensPower,
    required this.contact,
  });

  final GlassBackdropTexture backdrop;
  final double borderRadius;
  final double refraction;
  final double specular;
  final double thickness;
  final double absorption;
  final double blur;
  final bool frostLiveFace;
  final double lensPower;
  final ValueListenable<_Sheen> contact;

  @override
  State<_RefractiveSurface> createState() => _RefractiveSurfaceState();
}

class _RefractiveSurfaceState extends State<_RefractiveSurface> {
  static Future<ui.FragmentProgram>? _programFuture;
  static ui.FragmentProgram? _program;

  final GlobalKey _key = GlobalKey(debugLabel: 'refractive-glass-surface');
  ui.FragmentShader? _shader;

  @override
  void initState() {
    super.initState();
    final ready = _program;
    if (ready != null) {
      _shader = ready.fragmentShader();
    } else {
      _loadShader();
    }
  }

  Future<void> _loadShader() async {
    try {
      final program = await (_programFuture ??= ui.FragmentProgram.fromAsset(
        'shaders/liquid_glass_refraction.frag',
      ));
      _program = program;
      if (!mounted) return;
      setState(() => _shader = program.fragmentShader());
    } catch (error) {
      debugPrint('FusionPlay glass shader unavailable, using blur: $error');
    }
  }

  @override
  void dispose() {
    _shader?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final shader = _shader;
    final needsBlur = shader == null || (kIsWeb && widget.frostLiveFace);
    return Stack(
      key: _key,
      fit: StackFit.expand,
      children: [
        if (needsBlur)
          BackdropFilter(
            filter: ui.ImageFilter.blur(
              sigmaX: widget.blur,
              sigmaY: widget.blur,
            ),
            child: const SizedBox.expand(),
          ),
        if (shader != null)
          CustomPaint(
            painter: _RefractionPainter(
              shader: shader,
              backdrop: widget.backdrop,
              surfaceKey: _key,
              lensPower: widget.lensPower,
              contact: widget.contact,
              borderRadius: widget.borderRadius,
              refraction: widget.refraction,
              specular: widget.specular,
              thickness: widget.thickness,
              absorption: widget.absorption,
            ),
          ),
      ],
    );
  }
}

class _RefractionPainter extends CustomPainter {
  _RefractionPainter({
    required this.shader,
    required this.backdrop,
    required this.surfaceKey,
    required this.lensPower,
    required this.contact,
    required this.borderRadius,
    required this.refraction,
    required this.specular,
    required this.thickness,
    required this.absorption,
  }) : super(repaint: contact);

  final ui.FragmentShader shader;
  final GlassBackdropTexture backdrop;
  final GlobalKey surfaceKey;
  final double lensPower;
  final ValueListenable<_Sheen> contact;
  final double borderRadius;
  final double refraction;
  final double specular;
  final double thickness;
  final double absorption;

  @override
  void paint(Canvas canvas, Size size) {
    final box = surfaceKey.currentContext?.findRenderObject();
    if (box is! RenderBox || size.isEmpty) return;
    final origin = box.localToGlobal(Offset.zero);
    final axisX = box.localToGlobal(const Offset(1, 0)) - origin;
    final axisY = box.localToGlobal(const Offset(0, 1)) - origin;
    if (!origin.isFinite || !axisX.isFinite || !axisY.isFinite) return;
    final texture = backdrop;
    final state = contact.value;
    final pointer = state.pointer ?? Offset.zero;
    shader
      ..setFloat(0, size.width)
      ..setFloat(1, size.height)
      ..setFloat(2, origin.dx - texture.origin.dx)
      ..setFloat(3, origin.dy - texture.origin.dy)
      ..setFloat(4, texture.image.width.toDouble())
      ..setFloat(5, texture.image.height.toDouble())
      ..setFloat(6, texture.pixelRatio)
      ..setFloat(7, borderRadius)
      ..setFloat(8, refraction)
      ..setFloat(9, thickness)
      ..setFloat(10, specular)
      ..setFloat(11, absorption)
      ..setFloat(12, axisX.dx)
      ..setFloat(13, axisX.dy)
      ..setFloat(14, axisY.dx)
      ..setFloat(15, axisY.dy)
      ..setFloat(16, kIsWeb ? 1 : 0)
      ..setFloat(17, pointer.dx)
      ..setFloat(18, pointer.dy)
      ..setFloat(19, state.pressure)
      ..setFloat(20, lensPower);
    shader.setImageSampler(0, texture.image);
    canvas.drawRect(Offset.zero & size, Paint()..shader = shader);
  }

  @override
  bool shouldRepaint(_RefractionPainter oldDelegate) =>
      oldDelegate.lensPower != lensPower ||
      oldDelegate.shader != shader ||
      oldDelegate.backdrop != backdrop ||
      oldDelegate.surfaceKey != surfaceKey ||
      oldDelegate.borderRadius != borderRadius ||
      oldDelegate.refraction != refraction ||
      oldDelegate.specular != specular ||
      oldDelegate.thickness != thickness ||
      oldDelegate.absorption != absorption;
}

class _GlassPaint extends CustomPainter {
  _GlassPaint({
    required this.sheen,
    required this.spec,
    required this.tint,
    required this.gradient,
    required this.borderRadius,
    required this.specular,
    required this.focused,
  }) : super(repaint: sheen);

  final ValueListenable<_Sheen> sheen;
  final GlassSpec spec;
  final Color tint;
  final Gradient? gradient;
  final double borderRadius;
  final double specular;
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
    final intensity =
        (spec.sheenAlpha * strength * (.35 + .45 * state.pressure)).clamp(
          0.0,
          .15,
        );
    if (pointer != null && intensity > .001) {
      final radius = size.shortestSide * 1.25;
      final paint = Paint()
        ..shader = RadialGradient(
          colors: [
            Colors.white.withValues(alpha: intensity),
            Colors.white.withValues(alpha: intensity * .22),
            Colors.white.withValues(alpha: 0),
          ],
          stops: const [0.0, .45, 1.0],
        ).createShader(Rect.fromCircle(center: pointer, radius: radius));
      canvas.drawRRect(rrect, paint);
    }

    // 3. Rim: bright inner edge top/left, fading to a faint bottom/right.
    final rimPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = .8
      ..shader = LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [
          Colors.white.withValues(
            alpha: spec.borderAlpha * .45 + spec.rimAlpha * strength * .45,
          ),
          Colors.white.withValues(alpha: spec.borderAlpha * .28),
          Colors.white.withValues(
            alpha: spec.borderAlpha * .36 + strength * .10,
          ),
        ],
        stops: const [0.0, .55, 1.0],
      ).createShader(rect);
    canvas.drawRRect(rrect.deflate(.4), rimPaint);
  }

  @override
  bool shouldRepaint(_GlassPaint oldDelegate) =>
      oldDelegate.spec != spec ||
      oldDelegate.tint != tint ||
      oldDelegate.gradient != gradient ||
      oldDelegate.borderRadius != borderRadius ||
      oldDelegate.specular != specular ||
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
    final enabled = GlassAppearance.enabledOf(context);
    final radius = BorderRadius.circular(borderRadius);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: enabled
            ? tint ?? spec.tint.withValues(alpha: .10)
            : spec.solidColor,
        borderRadius: radius,
        border: Border.all(
          color: Colors.white.withValues(alpha: borderAlpha ?? .10),
          width: 1,
        ),
        boxShadow: enabled && raise
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
    final theme = Theme.of(context);
    final surfaceTheme = theme.copyWith(
      canvasColor: Colors.transparent,
      colorScheme: const ColorScheme.dark(
        primary: Color(0xFFF2F2F5),
        onPrimary: Color(0xFF17191D),
        surface: FusionGlassColors.solidPanel,
        onSurface: Colors.white,
        secondaryContainer: Color(0xFFE4E4EB),
        onSecondaryContainer: Color(0xFF17191D),
      ),
      textTheme: theme.textTheme.apply(
        bodyColor: Colors.white,
        displayColor: Colors.white,
      ),
    );
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
              child: Theme(
                data: surfaceTheme,
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
}) {
  final source = context
      .getInheritedWidgetOfExactType<GlassBackdropProvider>()
      ?.notifier;
  return showDialog<T>(
    context: context,
    barrierDismissible: barrierDismissible,
    barrierColor: Colors.black.withValues(alpha: .42),
    builder: (dialogContext) {
      final dialog = GlassDialog(
        key: dialogKey,
        child: Builder(builder: builder),
      );
      return source == null
          ? dialog
          : GlassBackdropProvider(notifier: source, child: dialog);
    },
  );
}
