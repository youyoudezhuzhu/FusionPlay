import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'g2_shape.dart';
import 'glass_backdrop.dart';
import 'liquid_glass.dart';
import 'remote_focus.dart';

class RoundedProgressBar extends StatefulWidget {
  static const hitHeight = 40.0;

  const RoundedProgressBar({
    super.key,
    required this.value,
    required this.enabled,
    required this.activeColor,
    required this.inactiveColor,
    required this.onChanged,
    this.seekEnabled,
    this.showThumb = true,
    this.refractiveThumb = true,
    this.onChangeCancel,
    this.focusBorderColor = Colors.black,
    this.focusNode,
    this.upFocusNode,
    this.downFocusNode,
    this.onChangeStart,
    this.onChangeEnd,
  });
  final double value;
  final bool enabled;
  final Color activeColor;
  final Color inactiveColor;
  final ValueChanged<double> onChanged;
  final bool? seekEnabled;
  final bool showThumb;
  final bool refractiveThumb;
  final VoidCallback? onChangeCancel;
  final Color focusBorderColor;
  final FocusNode? focusNode;
  final FocusNode? upFocusNode;
  final FocusNode? downFocusNode;
  final ValueChanged<double>? onChangeStart;
  final ValueChanged<double>? onChangeEnd;

  @override
  State<RoundedProgressBar> createState() => _RoundedProgressBarState();
}

class _RoundedProgressBarState extends State<RoundedProgressBar> {
  double? dragging;
  bool focused = false;
  bool touching = false;

  bool get interactionEnabled => widget.enabled && (widget.seekEnabled ?? true);

  void setTouching(bool value) {
    if (touching != value) setState(() => touching = value);
  }

  @override
  void didUpdateWidget(covariant RoundedProgressBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!interactionEnabled) touching = false;
  }

  void update(Offset local, double width, double inset) {
    if (!interactionEnabled || width <= 0) return;
    final usableWidth = math.max(1.0, width - inset * 2);
    final next = ((local.dx - inset) / usableWidth).clamp(0, 1).toDouble();
    setState(() => dragging = next);
    widget.onChanged(next);
  }

  void cancelDrag() {
    if (dragging == null) return;
    setState(() => dragging = null);
    widget.onChangeCancel?.call();
  }

  void adjust(double delta) {
    if (!interactionEnabled) return;
    final current = (dragging ?? widget.value).clamp(0.0, 1.0);
    final next = (current + delta).clamp(0.0, 1.0).toDouble();
    widget.onChangeStart?.call(current);
    widget.onChanged(next);
    widget.onChangeEnd?.call(next);
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final value = (dragging ?? widget.value).clamp(0.0, 1.0);
      final highlighted = focused || touching;
      final glassThumb =
          widget.showThumb &&
          widget.refractiveThumb &&
          GlassAppearance.enabledOf(context) &&
          GlassBackdropScope.of(context) != null;
      final inset = math.min(glassThumb ? 14.0 : 8.0, constraints.maxWidth / 2);
      final active = widget.enabled
          ? widget.activeColor
          : widget.activeColor.withValues(alpha: widget.activeColor.a * .38);
      final inactive = widget.enabled
          ? widget.inactiveColor
          : widget.inactiveColor.withValues(alpha: widget.inactiveColor.a * .5);
      final reduceMotion =
          MediaQuery.maybeOf(context)?.disableAnimations ?? false;
      return RemoteFocusFrame(
        enabled: interactionEnabled,
        focusNode: widget.focusNode,
        showBorder: false,
        onFocusChange: (value) {
          if (focused != value) setState(() => focused = value);
        },
        borderColor: widget.focusBorderColor,
        shape: const G2ContinuousBorder(radius: 20),
        shortcuts: <ShortcutActivator, Intent>{
          const SingleActivator(LogicalKeyboardKey.arrowLeft):
              const RemoteAdjustIntent(-.02),
          const SingleActivator(LogicalKeyboardKey.arrowRight):
              const RemoteAdjustIntent(.02),
          if (widget.upFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowUp):
                const DirectionalFocusIntent(TraversalDirection.up),
          if (widget.downFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowDown):
                const DirectionalFocusIntent(TraversalDirection.down),
        },
        actions: <Type, Action<Intent>>{
          RemoteAdjustIntent: CallbackAction<RemoteAdjustIntent>(
            onInvoke: (intent) {
              adjust(intent.delta);
              return null;
            },
          ),
          if (widget.upFocusNode != null || widget.downFocusNode != null)
            DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
              onInvoke: (intent) {
                if (intent.direction == TraversalDirection.up) {
                  widget.upFocusNode?.requestFocus();
                } else if (intent.direction == TraversalDirection.down) {
                  widget.downFocusNode?.requestFocus();
                }
                return null;
              },
            ),
        },
        child: TweenAnimationBuilder<double>(
          duration: reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 100),
          curve: Curves.easeOutCubic,
          tween: Tween(end: highlighted ? 1 : 0),
          builder: (context, focusProgress, _) => Listener(
            onPointerDown: interactionEnabled ? (_) => setTouching(true) : null,
            onPointerUp: interactionEnabled ? (_) => setTouching(false) : null,
            onPointerCancel: interactionEnabled
                ? (_) => setTouching(false)
                : null,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: interactionEnabled
                  ? (e) {
                      update(e.localPosition, constraints.maxWidth, inset);
                      widget.onChangeStart?.call(dragging ?? value);
                    }
                  : null,
              onTapUp: interactionEnabled
                  ? (_) {
                      widget.onChangeEnd?.call(dragging ?? value);
                      setState(() => dragging = null);
                    }
                  : null,
              onHorizontalDragStart: interactionEnabled
                  ? (e) {
                      update(e.localPosition, constraints.maxWidth, inset);
                      widget.onChangeStart?.call(dragging ?? value);
                    }
                  : null,
              onHorizontalDragUpdate: interactionEnabled
                  ? (e) => update(e.localPosition, constraints.maxWidth, inset)
                  : null,
              onTapCancel: cancelDrag,
              onHorizontalDragCancel: cancelDrag,
              onHorizontalDragEnd: interactionEnabled
                  ? (_) {
                      widget.onChangeEnd?.call(dragging ?? value);
                      setState(() => dragging = null);
                    }
                  : null,
              child: SizedBox(
                height: RoundedProgressBar.hitHeight,
                child: LayoutBuilder(
                  builder: (context, bounds) {
                    final span = bounds.maxWidth - 2 * inset;
                    final travel = span * value;
                    final width = math.min(
                      bounds.maxWidth,
                      20 + 8 * focusProgress,
                    );
                    final height = math.min(
                      bounds.maxHeight,
                      17 + 5 * focusProgress,
                    );
                    final trackHeight = 6 + 2 * focusProgress;
                    final trackColor = Color.lerp(
                      inactive,
                      active,
                      .24 * focusProgress,
                    )!;
                    return Stack(
                      fit: StackFit.expand,
                      children: [
                        CustomPaint(
                          painter: _ProgressPainter(
                            value: value,
                            active: active,
                            inactive: trackColor,
                            showThumb: widget.showThumb && !glassThumb,
                            inset: inset,
                            focusProgress: focusProgress,
                          ),
                        ),
                        if (glassThumb)
                          Positioned(
                            left: inset + travel - width / 2,
                            top: (bounds.maxHeight - height) / 2,
                            width: width,
                            height: height,
                            child: IgnorePointer(
                              child: LiquidGlass.droplet(
                                key: const ValueKey('volume-droplet'),
                                rail: (
                                  start: -travel,
                                  end: span - travel,
                                  height: trackHeight,
                                  active: active,
                                  inactive: trackColor,
                                ),
                              ),
                            ),
                          ),
                      ],
                    );
                  },
                ),
              ),
            ),
          ),
        ),
      );
    },
  );
}

class _ProgressPainter extends CustomPainter {
  const _ProgressPainter({
    required this.value,
    required this.active,
    required this.inactive,
    required this.showThumb,
    required this.focusProgress,
    required this.inset,
  });
  final double value;
  final Color active;
  final Color inactive;
  final bool showThumb;
  final double focusProgress;
  final double inset;

  @override
  void paint(Canvas canvas, Size size) {
    final y = size.height / 2;
    final thumbRadius = 8 + 2 * focusProgress;
    final trackStart = inset;
    final trackEnd = math.max(trackStart, size.width - inset);
    final trackWidth = trackEnd - trackStart;
    final trackHeight = 6 + 2 * focusProgress;
    final inactivePaint = Paint()
      ..color = inactive
      ..strokeWidth = trackHeight
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(Offset(trackStart, y), Offset(trackEnd, y), inactivePaint);
    final activeWidth = math.max(0.0, trackWidth * value);
    if (activeWidth > 0) {
      canvas.drawLine(
        Offset(trackStart, y),
        Offset(trackStart + activeWidth, y),
        Paint()
          ..color = active
          ..strokeWidth = trackHeight
          ..strokeCap = StrokeCap.round,
      );
    }
    if (showThumb) {
      canvas.drawCircle(
        Offset(trackStart + activeWidth, y),
        thumbRadius,
        Paint()..color = active,
      );
    }
  }

  @override
  bool shouldRepaint(_ProgressPainter old) =>
      old.inset != inset ||
      old.value != value ||
      old.active != active ||
      old.inactive != inactive ||
      old.showThumb != showThumb ||
      old.focusProgress != focusProgress;
}
