import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../theme/fusion_theme.dart';
import 'g2_shape.dart';
import 'liquid_glass.dart';
import 'remote_focus.dart';
import 'vector_icon.dart';

/// One entry of the three-dot action menu.
class FusionMenuItem {
  const FusionMenuItem({
    required this.icon,
    required this.label,
    required this.onSelected,
    this.semanticLabel,
  });

  final Widget icon;
  final String label;
  final VoidCallback onSelected;
  final String? semanticLabel;
}

/// Menu content shown inside the morphing glass container.
///
/// The host owns visibility, morph animation, focus nodes and dismissal; this
/// widget is presentation-only.
class FusionAppMenu extends StatelessWidget {
  const FusionAppMenu({
    super.key,
    required this.items,
    required this.focusNodes,
  });

  final List<FusionMenuItem> items;
  final List<FocusNode> focusNodes;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.all(7),
    child: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var index = 0; index < items.length; index++)
          _MenuItemRow(
            item: items[index],
            focusNode: index < focusNodes.length ? focusNodes[index] : null,
            autofocus: index == 0,
            onSelected: items[index].onSelected,
          ),
      ],
    ),
  );
}

class _MenuItemRow extends StatelessWidget {
  const _MenuItemRow({
    required this.item,
    required this.focusNode,
    required this.autofocus,
    required this.onSelected,
  });

  final FusionMenuItem item;
  final FocusNode? focusNode;
  final bool autofocus;
  final VoidCallback onSelected;

  @override
  Widget build(BuildContext context) => RemoteFocusFrame(
    focusNode: focusNode,
    autofocus: autofocus,
    onActivate: onSelected,
    borderColor: Colors.white,
    shape: const G2ContinuousBorder(radius: 14),
    child: InkWell(
      borderRadius: BorderRadius.circular(14),
      onTap: onSelected,
      child: Semantics(
        label: item.semanticLabel ?? item.label,
        button: true,
        child: SizedBox(
          height: 46,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Row(
              children: [
                SizedBox.square(
                  dimension: 22,
                  child: Center(
                    child: IconTheme.merge(
                      data: IconThemeData(
                        size: 19,
                        color: Colors.white.withValues(alpha: .86),
                      ),
                      child: item.icon,
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    item.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 15,
                      color: Colors.white.withValues(alpha: .94),
                      fontWeight: FusionTypography.mediumFontWeight,
                      fontVariations: FusionTypography.medium,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
}

/// Right-half glass settings panel content. The morph overlay provides the
/// animated glass container/rect; this widget just fills it.
class FusionSettingsSheet extends StatelessWidget {
  const FusionSettingsSheet({
    super.key,
    required this.title,
    required this.child,
    required this.onClose,
  });

  final String title;
  final Widget child;
  final VoidCallback onClose;

  /// Height reserved for the pinned header placeholder. The real header is an
  /// overlay above the scroll view so the body really scrolls underneath it.
  static const headerHeight = 58.0;

  /// Extra distance below the header over which content fades out.
  static const topFadeHeight = 30.0;

  @override
  Widget build(BuildContext context) => ClipRect(
    child: Stack(
      children: [
        Positioned.fill(
          child: ShaderMask(
            // Fade only the top of the scrolling body; the header overlay is
            // painted above the mask so the title/close button stay crisp.
            shaderCallback: (bounds) => LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: const [
                Color(0x00FFFFFF),
                Color(0x00FFFFFF),
                Color(0xFFFFFFFF),
              ],
              stops: [
                0.0,
                (headerHeight / bounds.height).clamp(0.0, .9),
                ((headerHeight + topFadeHeight) / bounds.height).clamp(.01, 1),
              ],
            ).createShader(bounds),
            blendMode: BlendMode.dstIn,
            child: CustomScrollView(
              key: const ValueKey('settings-scroll'),
              slivers: [
                // Layout spacer only: no background, no black gradient. The
                // content slivers scroll behind it and the overlay header.
                const PinnedHeaderSliver(
                  key: ValueKey('settings-pinned-header'),
                  child: SizedBox(height: headerHeight),
                ),
                SliverList.list(children: [child]),
              ],
            ),
          ),
        ),
        Positioned(
          top: 0,
          left: 0,
          right: 0,
          height: headerHeight,
          child: _GlassSettingsHeader(title: title, onClose: onClose),
        ),
      ],
    ),
  );
}

/// Real header: token-based translucent surface (no backdrop filter of its own,
/// the outer morph glass already blurs) with a soft top sheen and no hard
/// divider or opaque black band.
class _GlassSettingsHeader extends StatelessWidget {
  const _GlassSettingsHeader({required this.title, required this.onClose});

  final String title;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => DecoratedBox(
    key: const ValueKey('settings-glass-header'),
    decoration: const BoxDecoration(
      gradient: LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: [Color(0x1FFFFFFF), Color(0x0AFFFFFF), Color(0x00FFFFFF)],
        stops: [0.0, .55, 1.0],
      ),
    ),
    child: Padding(
      padding: const EdgeInsets.fromLTRB(20, 10, 10, 8),
      child: Row(
        children: [
          const FusionVectorIcon(
            FusionIcons.settings,
            size: 20,
            color: Color(0xB8FFFFFF),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                fontSize: 20,
                color: Color(0xEBFFFFFF),
                fontWeight: FusionTypography.semiBoldFontWeight,
                fontVariations: FusionTypography.semiBold,
              ),
            ),
          ),
          _CloseSettingsButton(onClose: onClose),
        ],
      ),
    ),
  );
}

class _CloseSettingsButton extends StatelessWidget {
  const _CloseSettingsButton({required this.onClose});

  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => RemoteFocusFrame(
    onActivate: onClose,
    borderColor: Colors.white,
    shape: const CircleBorder(),
    child: InkWell(
      customBorder: const CircleBorder(),
      onTap: onClose,
      child: Semantics(
        label: '关闭设置',
        button: true,
        child: SizedBox.square(
          dimension: 40,
          child: Center(
            child: Icon(
              Icons.close_rounded,
              size: 20,
              color: Colors.white.withValues(alpha: .86),
            ),
          ),
        ),
      ),
    ),
  );
}

/// Stage of the morphing overlay used by the home page.
enum MorphStage { menu, settings, closing }

/// Container-transform overlay: a single liquid-glass surface morphs from the
/// three-dot trigger (circle) to the anchored menu, then to the right-half
/// settings panel, and back.
class FusionMorphOverlay extends StatefulWidget {
  const FusionMorphOverlay({
    super.key,
    required this.stage,
    required this.anchorRect,
    required this.menuItems,
    required this.menuFocusNodes,
    required this.settingsChild,
    required this.settingsTitle,
    required this.onDismiss,
    required this.onSettingsClose,
    required this.onClosed,
    this.onTransitionCompleted,
    this.settingsMaxWidth = 560,
    this.forceFallback = false,
  });

  final MorphStage stage;

  /// Global (overlay-local) rect of the three-dot trigger when opened.
  final Rect anchorRect;
  final List<FusionMenuItem> menuItems;
  final List<FocusNode> menuFocusNodes;
  final Widget settingsChild;
  final String settingsTitle;
  final VoidCallback onDismiss;
  final VoidCallback onSettingsClose;
  final VoidCallback onClosed;
  final ValueChanged<MorphStage>? onTransitionCompleted;

  /// Upper bound of the settings panel width (still clamped to half screen).
  final double settingsMaxWidth;
  final bool forceFallback;

  @override
  State<FusionMorphOverlay> createState() => _FusionMorphOverlayState();
}

class _FusionMorphOverlayState extends State<FusionMorphOverlay>
    with SingleTickerProviderStateMixin {
  static const settingsRadius = 28.0;
  static const menuTint = Color(0xC014171C);
  static const settingsTint = Color(0xD014191F);

  late final AnimationController _controller = AnimationController(vsync: this);

  late MorphStage _fromStage = widget.stage;
  late MorphStage _toStage = widget.stage;
  late Rect _fromRect = widget.anchorRect;
  late Rect _toRect = widget.anchorRect;
  late double _fromRadius = _circleRadius(widget.anchorRect);
  late double _toRadius = _circleRadius(widget.anchorRect);
  Size _size = Size.zero;
  bool _closingNotified = false;

  static double _circleRadius(Rect rect) =>
      math.min(rect.width, rect.height) / 2;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _animateTo(widget.stage);
    });
  }

  @override
  void didUpdateWidget(covariant FusionMorphOverlay oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.stage != widget.stage ||
        oldWidget.anchorRect != widget.anchorRect) {
      final target = widget.stage;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || widget.stage != target) return;
        _animateTo(target);
      });
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  bool get _reduceMotion =>
      MediaQuery.maybeOf(context)?.disableAnimations ?? false;

  double get _eased =>
      Curves.easeOutCubic.transform(_controller.value.clamp(0.0, 1.0));

  Rect get _currentRect => Rect.lerp(_fromRect, _toRect, _eased) ?? _toRect;

  double get _currentRadius => _fromRadius + (_toRadius - _fromRadius) * _eased;

  double get _settingsAmount {
    if (_toStage == MorphStage.settings) return _controller.value;
    if (_fromStage == MorphStage.settings) return 1 - _controller.value;
    return 0;
  }

  double get _menuOpacity {
    if (_fromStage == MorphStage.settings || _toStage == MorphStage.settings) {
      return (1 - _settingsAmount / .35).clamp(0.0, 1.0);
    }
    if (_toStage == MorphStage.closing) {
      return (1 - _controller.value).clamp(0.0, 1.0);
    }
    return (_controller.value / .35).clamp(0.0, 1.0);
  }

  double get _settingsOpacity =>
      ((_settingsAmount - .35) / .65).clamp(0.0, 1.0);

  /// The menu grows out of the trigger without moving its top-right corner:
  /// the trigger is anchored to the top-right corner of the screen, so the
  /// expanded panel keeps the same top-right inset and top edge. Its corner
  /// radius is the trigger circle radius, which keeps the arc the user sees on
  /// the three-dot button exactly the same when the circle becomes the panel.
  Rect _menuRect(Size size) {
    final width = size.width < 480 ? 220.0 : 240.0;
    final height = widget.menuItems.length * 46.0 + 14.0;
    final anchor = widget.anchorRect;
    var left = anchor.right - width;
    left = left.clamp(8.0, math.max(8.0, size.width - width - 8)).toDouble();
    var top = anchor.top;
    if (top + height > size.height - 8) {
      top = size.height - height - 8;
    }
    top = math.max(0.0, top);
    return Rect.fromLTWH(left, top, width, height);
  }

  Rect _settingsRect(Size size) {
    const side = 16.0;
    final width = math.min(widget.settingsMaxWidth, size.width * .5);
    final height = math.max(120.0, size.height - side * 2);
    return Rect.fromLTWH(size.width - side - width, side, width, height);
  }

  Rect _targetRect(MorphStage stage) => switch (stage) {
    MorphStage.menu => _menuRect(_size),
    MorphStage.settings => _settingsRect(_size),
    MorphStage.closing => widget.anchorRect,
  };

  void _animateTo(MorphStage stage) {
    if (_size.isEmpty) {
      // Wait for the first layout to know the viewport.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && _size.isEmpty) {
          _size = Size(
            MediaQuery.sizeOf(context).width,
            MediaQuery.sizeOf(context).height,
          );
        }
        if (mounted) _animateTo(stage);
      });
      return;
    }
    final currentRect = _currentRect;
    final currentRadius = _currentRadius;
    final fromStage = _toStage;
    _fromRect = currentRect;
    _toRect = _targetRect(stage);
    _fromRadius = currentRadius;
    _toRadius = switch (stage) {
      // Keep the trigger's arc: the top-right corner of the expanded menu has
      // exactly the circle radius of the three-dot button.
      MorphStage.menu => _circleRadius(widget.anchorRect),
      MorphStage.settings => settingsRadius,
      MorphStage.closing => _circleRadius(widget.anchorRect),
    };
    _fromStage = fromStage;
    _toStage = stage;
    _closingNotified = false;
    if (_reduceMotion) {
      _controller.duration = Duration.zero;
      _controller.value = 1;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || widget.stage != stage) return;
        if (stage == MorphStage.closing) {
          _notifyClosed();
        } else {
          widget.onTransitionCompleted?.call(stage);
        }
      });
      return;
    }
    _controller.duration = switch (stage) {
      MorphStage.menu => const Duration(milliseconds: 260),
      MorphStage.settings => const Duration(milliseconds: 320),
      MorphStage.closing => const Duration(milliseconds: 240),
    };
    _controller.forward(from: 0).whenComplete(() {
      if (!mounted || widget.stage != stage) return;
      if (stage == MorphStage.closing) {
        _notifyClosed();
      } else {
        widget.onTransitionCompleted?.call(stage);
      }
    });
  }

  void _notifyClosed() {
    if (_closingNotified) return;
    _closingNotified = true;
    widget.onClosed();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      _size = Size(constraints.maxWidth, constraints.maxHeight);
      // Local rebuild: only the morph layer rebuilds per tick; the settings
      // subtree is passed by identity so Flutter skips rebuilding it, and the
      // glass surface is isolated with a RepaintBoundary.
      return AnimatedBuilder(
        animation: _controller,
        builder: (context, _) {
          final rect = _currentRect;
          final radius = _currentRadius;
          final settingsAmount = _settingsAmount;
          final menuOpacity = _menuOpacity;
          final settingsOpacity = _settingsOpacity;
          final dim =
              (.18 * math.max(menuOpacity, settingsOpacity) +
                      .12 * settingsOpacity)
                  .clamp(0.0, .42);
          final menuSize = _menuRect(_size).size;
          final settingsSize = _settingsRect(_size).size;
          final tint = Color.lerp(menuTint, settingsTint, settingsAmount)!;
          final glass = RepaintBoundary(
            child: LiquidGlass(
              key: const ValueKey('morph-container'),
              level: GlassLevel.panel,
              interactive: false,
              borderRadius: radius,
              blurSigma: settingsAmount > .5 ? 24 : 20,
              tint: tint,
              tintGradient: settingsAmount > .35
                  ? const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [Color(0x1A000000), Color(0x04000000)],
                    )
                  : null,
              animate: false,
              forceFallback: widget.forceFallback,
              child: ClipRect(
                child: Stack(
                  children: [
                    Positioned(
                      left: 0,
                      top: 0,
                      width: menuSize.width,
                      height: menuSize.height,
                      child: Offstage(
                        offstage: menuOpacity <= .001,
                        child: IgnorePointer(
                          ignoring: settingsAmount > .05,
                          child: ExcludeFocus(
                            excluding: settingsAmount > .05,
                            child: Opacity(
                              opacity: menuOpacity,
                              child: RepaintBoundary(
                                child: FusionAppMenu(
                                  key: const ValueKey('app-menu'),
                                  items: widget.menuItems,
                                  focusNodes: widget.menuFocusNodes,
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                    Positioned(
                      left: 0,
                      top: 0,
                      width: settingsSize.width,
                      height: settingsSize.height,
                      child: Offstage(
                        offstage: settingsOpacity <= .001,
                        child: IgnorePointer(
                          ignoring: settingsOpacity < .5,
                          child: ExcludeFocus(
                            excluding: settingsOpacity < .5,
                            child: Opacity(
                              opacity: settingsOpacity,
                              child: RepaintBoundary(
                                child: FusionSettingsSheet(
                                  key: const ValueKey('settings-sheet'),
                                  title: widget.settingsTitle,
                                  onClose: widget.onSettingsClose,
                                  child: widget.settingsChild,
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          );
          return Stack(
            children: [
              Positioned.fill(
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: widget.onDismiss,
                  child: ColoredBox(color: Colors.black.withValues(alpha: dim)),
                ),
              ),
              Positioned.fromRect(
                rect: rect,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () {},
                  child: glass,
                ),
              ),
            ],
          );
        },
      );
    },
  );
}
