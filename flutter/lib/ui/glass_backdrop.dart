import 'dart:async';
import 'dart:ui' as ui;
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

/// User-selected material. A temporarily missing texture only changes the
/// advanced renderer's fallback, never the user's choice of opacity.
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

/// A captured, glass-free backdrop texture.
///
/// The texture comes from a dedicated [RepaintBoundary] that contains only the
/// background (artwork/gradient/grid). Glass surfaces are painted *outside*
/// that boundary, so sampling the texture can never feed a glass's own output
/// back into itself. This is the safe replacement for the old
/// `ImageFilter.shader` feedback path.
@immutable
class GlassBackdropTexture {
  const GlassBackdropTexture({
    required this.image,
    required this.logicalSize,
    required this.pixelRatio,
    this.origin = Offset.zero,
  });

  final ui.Image image;
  final Size logicalSize;
  final double pixelRatio;
  final Offset origin;
}

/// Provides a real backdrop texture to every descendant glass surface.
///
/// Wrap the app (or a page) with this widget: [background] is captured and
/// displayed below [child]. Because the capture boundary excludes [child],
/// dialogs/menus and their glass do not contaminate the sampled image.
class GlassBackdropScope extends StatefulWidget {
  const GlassBackdropScope({
    super.key,
    required this.background,
    required this.child,
    this.revision,
    this.onTextureChanged,
    this.enabled = true,
    this.captureInterval = const Duration(milliseconds: 33),
  });

  final Widget background;
  final Widget child;

  /// Change this when the background content changes. The default app passes
  /// the artwork identity; animated demos can pass a frame counter.
  final Object? revision;

  final ValueChanged<GlassBackdropTexture?>? onTextureChanged;
  final bool enabled;

  /// Minimum time between captures while [revision] keeps changing.
  final Duration captureInterval;

  /// Returns the nearest backdrop texture, or null when no scope is available
  /// (or it has not captured yet). Null must always select the stable fallback.
  static GlassBackdropTexture? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<GlassBackdropProvider>()
      ?.texture;

  @override
  State<GlassBackdropScope> createState() => _GlassBackdropScopeState();
}

class _GlassBackdropScopeState extends State<GlassBackdropScope> {
  final GlobalKey _boundaryKey = GlobalKey(
    debugLabel: 'glass-backdrop-boundary',
  );
  final ValueNotifier<GlassBackdropTexture?> _texture =
      ValueNotifier<GlassBackdropTexture?>(null);
  bool _capturing = false;
  bool _captureScheduled = false;
  Timer? _delay;
  bool _dirty = false;
  DateTime? _lastCaptureAt;

  @override
  void initState() {
    super.initState();
    _scheduleCapture();
  }

  @override
  void didUpdateWidget(covariant GlassBackdropScope oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!widget.enabled && oldWidget.enabled) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || widget.enabled) return;
        final stale = _texture.value;
        _texture.value = null;
        widget.onTextureChanged?.call(null);
        WidgetsBinding.instance.addPostFrameCallback(
          (_) => stale?.image.dispose(),
        );
      });
    }
    if (widget.enabled != oldWidget.enabled ||
        widget.revision != oldWidget.revision) {
      _scheduleCapture();
    }
  }

  @override
  void dispose() {
    _delay?.cancel();
    _texture.value?.image.dispose();
    _texture.dispose();
    super.dispose();
  }

  void _scheduleCapture() {
    if (!widget.enabled) return;
    _dirty = true;
    if (_captureScheduled || _capturing || _delay != null) return;
    _captureScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _captureScheduled = false;
      if (!mounted || !widget.enabled) return;
      if (_capturing) return;
      final last = _lastCaptureAt;
      final now = DateTime.now();
      final delay = last == null
          ? Duration.zero
          : widget.captureInterval - now.difference(last);
      if (delay > Duration.zero) {
        _delay = Timer(delay, () {
          _delay = null;
          _capture();
        });
      } else {
        _capture();
      }
    });
  }

  Future<void> _capture() async {
    if (!mounted || _capturing || !widget.enabled) return;
    final boundary = _boundaryKey.currentContext?.findRenderObject();
    if (boundary is! RenderRepaintBoundary || !boundary.hasSize) return;
    _dirty = false;
    _capturing = true;
    _lastCaptureAt = DateTime.now();
    try {
      final logicalSize = boundary.size;
      final origin = boundary.localToGlobal(Offset.zero);
      if (logicalSize.isEmpty) return;
      final displayRatio = MediaQuery.maybeDevicePixelRatioOf(context) ?? 1.0;
      // Bound a single RGBA texture to roughly 8 MiB, including on 4K TVs.
      final budgetRatio = math.sqrt(
        2097152 / (logicalSize.width * logicalSize.height),
      );
      final ratio = math.min(math.min(displayRatio, 2.0), budgetRatio);
      final image = await boundary.toImage(pixelRatio: ratio);
      if (!mounted || !widget.enabled) {
        image.dispose();
        return;
      }
      final next = GlassBackdropTexture(
        image: image,
        logicalSize: logicalSize,
        pixelRatio: ratio,
        origin: origin,
      );
      final old = _texture.value;
      _texture.value = next;
      widget.onTextureChanged?.call(next);
      // Keep the old image alive for the frame currently being painted.
      if (old != null) {
        WidgetsBinding.instance.addPostFrameCallback(
          (_) => old.image.dispose(),
        );
      }
    } catch (error) {
      debugPrint('FusionPlay glass backdrop capture failed: $error');
      if (mounted) {
        final stale = _texture.value;
        _texture.value = null;
        widget.onTextureChanged?.call(null);
        WidgetsBinding.instance.addPostFrameCallback(
          (_) => stale?.image.dispose(),
        );
      }
    } finally {
      _capturing = false;
      if (mounted && _dirty) {
        _scheduleCapture();
        WidgetsBinding.instance.ensureVisualUpdate();
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return GlassBackdropProvider(
      notifier: _texture,
      child: Stack(
        fit: StackFit.expand,
        children: [
          _BackdropBoundary(
            key: _boundaryKey,
            onPaint: _scheduleCapture,
            child: widget.background,
          ),
          widget.child,
        ],
      ),
    );
  }
}

class GlassBackdropProvider
    extends InheritedNotifier<ValueNotifier<GlassBackdropTexture?>> {
  const GlassBackdropProvider({
    super.key,
    required ValueNotifier<GlassBackdropTexture?> notifier,
    required super.child,
  }) : super(notifier: notifier);

  GlassBackdropTexture? get texture => notifier?.value;
}

class _BackdropBoundary extends SingleChildRenderObjectWidget {
  const _BackdropBoundary({
    super.key,
    required this.onPaint,
    required super.child,
  });
  final VoidCallback onPaint;

  @override
  RenderRepaintBoundary createRenderObject(BuildContext context) =>
      _PaintBoundary(onPaint);
}

class _PaintBoundary extends RenderRepaintBoundary {
  _PaintBoundary(this.onPaint);
  final VoidCallback onPaint;

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    onPaint();
  }
}
