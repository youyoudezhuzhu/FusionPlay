import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

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
  });

  final ui.Image image;
  final Size logicalSize;
  final double pixelRatio;
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
    this.enabled = true,
    this.captureInterval = const Duration(milliseconds: 120),
  });

  final Widget background;
  final Widget child;

  /// Change this when the background content changes. The default app passes
  /// the artwork identity; animated demos can pass a frame counter.
  final Object? revision;

  final bool enabled;

  /// Minimum time between captures while [revision] keeps changing.
  final Duration captureInterval;

  /// Returns the nearest backdrop texture, or null when no scope is available
  /// (or it has not captured yet). Null must always select the stable fallback.
  static GlassBackdropTexture? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<_GlassBackdropInherited>()
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
      final ratio = (MediaQuery.maybeDevicePixelRatioOf(context) ?? 1.0)
          .clamp(1.0, 2.0)
          .toDouble();
      final image = await boundary.toImage(pixelRatio: ratio);
      if (!mounted || !widget.enabled) {
        image.dispose();
        return;
      }
      final next = GlassBackdropTexture(
        image: image,
        logicalSize: logicalSize,
        pixelRatio: ratio,
      );
      final old = _texture.value;
      _texture.value = next;
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
    return _GlassBackdropInherited(
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

class _GlassBackdropInherited
    extends InheritedNotifier<ValueNotifier<GlassBackdropTexture?>> {
  const _GlassBackdropInherited({
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
