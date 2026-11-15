import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'g2_shape.dart';

/// Host-wide remote-focus styling. Android TV uses a thicker ring and a slight
/// focus scale; phones/desktop keep the defaults.
class FusionFocusStyle extends InheritedWidget {
  const FusionFocusStyle({
    super.key,
    this.borderWidth = 3,
    this.focusScale = 1,
    required super.child,
  });

  final double borderWidth;
  final double focusScale;

  static FusionFocusStyle? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<FusionFocusStyle>();

  @override
  bool updateShouldNotify(FusionFocusStyle oldWidget) =>
      borderWidth != oldWidget.borderWidth ||
      focusScale != oldWidget.focusScale;
}

/// Exposes the remote-focus highlight state to descendants (e.g. glass
/// surfaces can brighten their rim when the containing control is focused).
class FusionFocusHighlight extends InheritedWidget {
  const FusionFocusHighlight({
    super.key,
    required this.focused,
    required super.child,
  });

  final bool focused;

  static bool of(BuildContext context) =>
      context
          .dependOnInheritedWidgetOfExactType<FusionFocusHighlight>()
          ?.focused ??
      false;

  @override
  bool updateShouldNotify(FusionFocusHighlight oldWidget) =>
      focused != oldWidget.focused;
}

class RemoteFocusFrame extends StatefulWidget {
  const RemoteFocusFrame({
    super.key,
    required this.child,
    this.enabled = true,
    this.autofocus = false,
    this.focusNode,
    this.onActivate,
    this.onPressChange,
    this.onFocusChange,
    this.borderColor = Colors.black,
    this.borderWidth,
    this.shape = const G2ContinuousBorder(radius: 20),
    this.showBorder = true,
    this.shortcuts = const <ShortcutActivator, Intent>{},
    this.actions = const <Type, Action<Intent>>{},
  });

  final Widget child;
  final bool enabled;
  final bool autofocus;
  final FocusNode? focusNode;
  final VoidCallback? onActivate;
  final ValueChanged<bool>? onPressChange;
  final ValueChanged<bool>? onFocusChange;
  final Color borderColor;

  /// Focus-ring width override; defaults to [FusionFocusStyle.borderWidth].
  final double? borderWidth;

  final OutlinedBorder shape;
  final bool showBorder;
  final Map<ShortcutActivator, Intent> shortcuts;
  final Map<Type, Action<Intent>> actions;

  @override
  State<RemoteFocusFrame> createState() => _RemoteFocusFrameState();
}

class _RemoteFocusFrameState extends State<RemoteFocusFrame> {
  bool _showFocus = false;
  bool _focused = false;
  bool _activationPressed = false;

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_handleHardwareKey);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_handleHardwareKey);
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant RemoteFocusFrame oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.enabled && !widget.enabled) _setActivationPressed(false);
  }

  bool _isActivationKey(LogicalKeyboardKey key) =>
      key == LogicalKeyboardKey.select ||
      key == LogicalKeyboardKey.enter ||
      key == LogicalKeyboardKey.space;

  bool _handleHardwareKey(KeyEvent event) {
    if (!_focused || !_isActivationKey(event.logicalKey)) {
      return false;
    }
    if (event is KeyDownEvent && widget.enabled) {
      _setActivationPressed(true);
    } else if (event is KeyUpEvent) {
      _setActivationPressed(false);
    }
    return false;
  }

  void _setActivationPressed(bool pressed) {
    if (_activationPressed == pressed) return;
    _activationPressed = pressed;
    widget.onPressChange?.call(pressed);
  }

  @override
  Widget build(BuildContext context) {
    final focusStyle = FusionFocusStyle.maybeOf(context);
    final ringWidth = widget.borderWidth ?? focusStyle?.borderWidth ?? 3;
    final focusScale = focusStyle?.focusScale ?? 1;
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    final shortcuts = <ShortcutActivator, Intent>{
      const SingleActivator(LogicalKeyboardKey.select): const ActivateIntent(),
      const SingleActivator(LogicalKeyboardKey.enter): const ActivateIntent(),
      const SingleActivator(LogicalKeyboardKey.space): const ActivateIntent(),
      ...widget.shortcuts,
    };
    final actions = <Type, Action<Intent>>{
      if (widget.onActivate != null)
        ActivateIntent: CallbackAction<ActivateIntent>(
          onInvoke: (_) {
            widget.onActivate!();
            return null;
          },
        ),
      ...widget.actions,
    };

    return FocusableActionDetector(
      enabled: widget.enabled,
      autofocus: widget.autofocus,
      focusNode: widget.focusNode,
      descendantsAreFocusable: false,
      descendantsAreTraversable: false,
      shortcuts: shortcuts,
      actions: actions,
      onShowFocusHighlight: (show) {
        if (_showFocus != show) setState(() => _showFocus = show);
      },
      onFocusChange: (focused) {
        _focused = focused;
        if (!focused) _setActivationPressed(false);
        widget.onFocusChange?.call(focused);
        if (!focused && _showFocus) setState(() => _showFocus = false);
        if (!focused) return;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!mounted) return;
          final reduceMotion =
              MediaQuery.maybeOf(context)?.disableAnimations ?? false;
          Scrollable.ensureVisible(
            context,
            duration: reduceMotion
                ? Duration.zero
                : const Duration(milliseconds: 120),
            curve: Curves.easeOutCubic,
            alignment: .5,
          );
        });
      },
      child: FusionFocusHighlight(
        focused: _showFocus,
        child: focusScale == 1
            ? _focusSurface(ringWidth)
            : AnimatedScale(
                scale: _showFocus ? focusScale : 1,
                duration: reduceMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 120),
                curve: Curves.easeOutCubic,
                child: _focusSurface(ringWidth),
              ),
      ),
    );
  }

  Widget _focusSurface(double ringWidth) => AnimatedContainer(
    duration: MediaQuery.maybeOf(context)?.disableAnimations ?? false
        ? Duration.zero
        : const Duration(milliseconds: 100),
    curve: Curves.easeOut,
    foregroundDecoration: ShapeDecoration(
      shape: widget.shape.copyWith(
        side: widget.showBorder
            ? BorderSide(
                color: widget.borderColor.withValues(
                  alpha: _showFocus ? widget.borderColor.a : 0,
                ),
                width: ringWidth,
              )
            : BorderSide.none,
      ),
    ),
    child: widget.child,
  );
}

class RemoteAdjustIntent extends Intent {
  const RemoteAdjustIntent(this.delta);

  final double delta;
}
