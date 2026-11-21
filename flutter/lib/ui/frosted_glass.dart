import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../theme/fusion_theme.dart';
import 'glass_appearance.dart';

enum GlassLevel { card, control, panel, popup }

/// Neutral materials inspired by iOS 18. The tint preserves contrast while
/// the clipped backdrop filter blurs whatever is visible behind the surface.
@immutable
class GlassSpec {
  const GlassSpec(this.tint, this.solidColor, this.blur);

  final Color tint;
  final Color solidColor;
  final double blur;

  static const card = GlassSpec(Color(0x383A3A43), Color(0xFF33333E), 12);
  static const control = GlassSpec(Color(0x4044444E), Color(0xFF444450), 14);
  static const panel = GlassSpec(
    FusionGlassColors.settingsTint,
    FusionGlassColors.solidPanel,
    FusionGlassColors.panelBlur,
  );
  static const popup = GlassSpec(
    FusionGlassColors.popupTint,
    FusionGlassColors.solidPanel,
    20,
  );

  static GlassSpec of(GlassLevel level) => switch (level) {
    GlassLevel.card => card,
    GlassLevel.control => control,
    GlassLevel.panel => panel,
    GlassLevel.popup => popup,
  };
}

class FrostedGlass extends StatelessWidget {
  const FrostedGlass({
    super.key,
    required this.child,
    this.level = GlassLevel.control,
    this.borderRadius = 24,
    this.blurSigma,
    this.tint,
    this.padding,
  });

  final Widget child;
  final GlassLevel level;
  final double borderRadius;
  final double? blurSigma;
  final Color? tint;
  final EdgeInsetsGeometry? padding;

  @override
  Widget build(BuildContext context) {
    final spec = GlassSpec.of(level);
    final enabled = GlassAppearance.enabledOf(context);
    final radius = BorderRadius.circular(borderRadius);
    final content = padding == null
        ? child
        : Padding(padding: padding!, child: child);
    final fill = ColoredBox(
      color: enabled ? tint ?? spec.tint : spec.solidColor,
      child: content,
    );
    final sigma = blurSigma ?? spec.blur;
    return ClipRRect(
      borderRadius: radius,
      child: DecoratedBox(
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          borderRadius: radius,
          border: Border.all(color: const Color(0x24FFFFFF), width: .75),
        ),
        child: BackdropFilter(
          enabled: enabled,
          filter: ui.ImageFilter.blur(sigmaX: sigma, sigmaY: sigma),
          child: fill,
        ),
      ),
    );
  }
}

/// Shared presentation for update, about and platform-specific dialogs.
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
          builder: (context, value, child) =>
              Transform.scale(scale: .97 + .03 * value, child: child),
          child: RepaintBoundary(
            child: FrostedGlass(
              level: GlassLevel.popup,
              borderRadius: 24,
              padding: padding,
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

Future<T?> showGlassDialog<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  Key? dialogKey,
  bool barrierDismissible = true,
}) => showDialog<T>(
  context: context,
  barrierDismissible: barrierDismissible,
  barrierColor: Colors.black.withValues(
    alpha: GlassAppearance.enabledOf(context) ? .16 : .42,
  ),
  builder: (_) => GlassDialog(
    key: dialogKey,
    child: Builder(builder: builder),
  ),
);
