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

/// Anchored liquid-glass action menu shown below the three-dot trigger.
///
/// The host owns visibility/animation, focus nodes (for returning focus to the
/// trigger) and outside-click dismissal so this widget stays presentation-only.
class FusionAppMenu extends StatelessWidget {
  const FusionAppMenu({
    super.key,
    required this.items,
    required this.focusNodes,
    required this.onDismiss,
    this.width = 240,
    this.forceFallback = false,
  });

  final List<FusionMenuItem> items;
  final List<FocusNode> focusNodes;
  final VoidCallback onDismiss;
  final double width;
  final bool forceFallback;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: width,
    child: LiquidGlass(
      borderRadius: 24,
      blurSigma: 20,
      tint: const Color(0xE014171C),
      forceFallback: forceFallback,
      padding: const EdgeInsets.all(7),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var index = 0; index < items.length; index++)
            _MenuItemRow(
              item: items[index],
              focusNode: index < focusNodes.length ? focusNodes[index] : null,
              autofocus: index == 0,
              onSelected: () {
                onDismiss();
                items[index].onSelected();
              },
            ),
        ],
      ),
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
  Widget build(BuildContext context) {
    return RemoteFocusFrame(
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
}

/// Centered liquid-glass settings sheet that hosts the existing [SettingsView]
/// content inside a light surface for readability.
class FusionSettingsSheet extends StatelessWidget {
  const FusionSettingsSheet({
    super.key,
    required this.title,
    required this.child,
    required this.onClose,
    this.forceFallback = false,
  });

  final String title;
  final Widget child;
  final VoidCallback onClose;
  final bool forceFallback;

  @override
  Widget build(BuildContext context) => Center(
    child: ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 720),
      child: FractionallySizedBox(
        heightFactor: .86,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: LiquidGlass(
            borderRadius: 28,
            blurSigma: 20,
            tint: const Color(0xE614171C),
            forceFallback: forceFallback,
            child: Column(
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(22, 14, 12, 10),
                  child: Row(
                    children: [
                      const FusionVectorIcon(
                        FusionIcons.settings,
                        size: 20,
                        color: Colors.white,
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 20,
                            color: Colors.white,
                            fontWeight: FusionTypography.semiBoldFontWeight,
                            fontVariations: FusionTypography.semiBold,
                          ),
                        ),
                      ),
                      _CloseSettingsButton(onClose: onClose),
                    ],
                  ),
                ),
                Divider(
                  height: 1,
                  thickness: 1,
                  color: Colors.white.withValues(alpha: .12),
                ),
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(10, 10, 10, 10),
                    child: ClipRRect(
                      borderRadius: BorderRadius.circular(22),
                      child: ColoredBox(color: Colors.white, child: child),
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
