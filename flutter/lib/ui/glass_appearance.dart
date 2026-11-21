import 'package:flutter/material.dart';

/// User preference for frosted backgrounds and opaque surfaces.
class GlassAppearance extends InheritedTheme {
  const GlassAppearance({
    super.key,
    required this.enabled,
    required super.child,
  });

  final bool enabled;

  static bool enabledOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<GlassAppearance>()?.enabled ??
      true;

  @override
  bool updateShouldNotify(GlassAppearance oldWidget) =>
      enabled != oldWidget.enabled;

  @override
  Widget wrap(BuildContext context, Widget child) =>
      GlassAppearance(enabled: enabled, child: child);
}

/// Composites the artwork and foreground as live siblings on every platform.
class FrostedScene extends StatelessWidget {
  const FrostedScene({
    super.key,
    required this.background,
    required this.child,
  });

  final Widget background;
  final Widget child;

  @override
  Widget build(BuildContext context) =>
      Stack(fit: StackFit.expand, children: [background, child]);
}
