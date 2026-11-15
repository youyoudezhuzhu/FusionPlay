import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../backend/update_service.dart';
import '../models/app_state.dart';
import '../theme/fusion_theme.dart';
import 'g2_shape.dart';
import 'remote_focus.dart';
import 'update_dialogs.dart';
import 'vector_icon.dart';

const fusionPlayVersion = String.fromEnvironment(
  'FUSIONPLAY_VERSION',
  defaultValue: '1.3.6',
);

/// Dark liquid-glass palette used by the settings panel. The outer
/// [LiquidGlass] surface provides the actual glass; everything here is a
/// translucent surface/fill painted on top of it.
abstract final class _GlassStyle {
  static const cardRadius = 22.0;
  static const rowRadius = 16.0;
  static const surface = Color(0xB315181D);
  static const surfaceBorder = Color(0x1FFFFFFF);
  static const rowHighlight = Color(0x1AFFFFFF);
  static const divider = Color(0x0DFFFFFF);
  static const textPrimary = Color(0xEBFFFFFF);
  static const textSecondary = Color(0x8CFFFFFF);
  static const textDisabled = Color(0x59FFFFFF);
  static const icon = Color(0xB8FFFFFF);
  static const iconMuted = Color(0x94FFFFFF);
  // Monochrome glass accent: the selected device identity uses the same
  // white/black language as the player controls instead of a green tint.
  static const accent = Color(0xF2FFFFFF);
  static const switchOffThumb = Color(0xCCFFFFFF);
  static const switchOnThumb = Color(0xFF17191D);
  static const inputFill = Color(0x14FFFFFF);
  static const inputBorder = Color(0x1FFFFFFF);
  static const inputFocusBorder = Color(0x73FFFFFF);
  static const hint = Color(0x73FFFFFF);
  static const actionFill = Color(0x1AFFFFFF);
  static const actionFillDisabled = Color(0x0DFFFFFF);
  static const actionPrimaryFill = Color(0xE6FFFFFF);
  static const actionPrimaryText = Color(0xFF16181B);
  static const switchOffTrack = Color(0x2EFFFFFF);
}

class SettingsView extends StatefulWidget {
  const SettingsView({
    super.key,
    required this.state,
    required this.initialFocusNode,
    required this.onClose,
    required this.onReceiverName,
    required this.onStartup,
    required this.onAutoWake,
    required this.onAdvancedEffects,
    required this.onMiPlay,
    required this.onMiPlayIdentity,
    required this.onAirPlay,
    required this.onDlna,
    required this.onExportLogs,
    this.nameFocusNode,
    this.miPlayFocusNode,
    this.isAndroid,
    this.onCheckUpdate,
    this.onInstallUpdate,
    this.showHeader = true,
  });

  final AppState state;
  final FocusNode initialFocusNode;

  /// Focus node for the receiver-name field (defaults to an internal
  /// `settings-name` node). The host requests it for non-TV initial focus.
  final FocusNode? nameFocusNode;

  /// Focus node for the MiPlay row (defaults to [initialFocusNode]). Android TV
  /// requests this row for initial focus.
  final FocusNode? miPlayFocusNode;
  final VoidCallback onClose;
  final ValueChanged<String> onReceiverName;
  final ValueChanged<bool> onStartup;
  final ValueChanged<bool> onAutoWake;
  final ValueChanged<bool> onAdvancedEffects;
  final ValueChanged<bool> onMiPlay;
  final ValueChanged<MiPlayDeviceIdentity> onMiPlayIdentity;
  final ValueChanged<bool> onAirPlay;
  final ValueChanged<bool> onDlna;
  final Future<void> Function() onExportLogs;

  /// Overrides the platform detection so widget tests can render the update
  /// row for Android and hide it for Windows without touching the host OS.
  final bool? isAndroid;

  /// Update check triggered by the "检查更新" row (Android only).
  final Future<UpdateCheckResult> Function()? onCheckUpdate;

  /// Download + system installer triggered from the update dialog.
  final Future<UpdateInstallResult> Function()? onInstallUpdate;

  /// When false the page title is omitted (hosted inside the glass overlay).
  final bool showHeader;

  @override
  State<SettingsView> createState() => _SettingsViewState();
}

class _SettingsViewState extends State<SettingsView> {
  late final TextEditingController _name;
  late final FocusNode _nameFocus;
  late final bool _ownsNameFocus;
  late String _savedName;
  bool _exportingLogs = false;
  bool _checkingUpdate = false;
  bool _downloadingUpdate = false;

  @override
  void initState() {
    super.initState();
    _ownsNameFocus = widget.nameFocusNode == null;
    _nameFocus = widget.nameFocusNode ?? FocusNode(debugLabel: 'settings-name');
    _savedName = widget.state.settings.receiverName ?? '';
    _name = TextEditingController(text: _savedName)..addListener(_refresh);
  }

  FocusNode get _miPlayFocus =>
      widget.miPlayFocusNode ?? widget.initialFocusNode;

  @override
  void didUpdateWidget(covariant SettingsView oldWidget) {
    super.didUpdateWidget(oldWidget);
    final incoming = widget.state.settings.receiverName ?? '';
    if (incoming != _savedName && _name.text == _savedName) {
      _savedName = incoming;
      _name.text = incoming;
    }
  }

  void _refresh() => setState(() {});

  @override
  void dispose() {
    _name.removeListener(_refresh);
    _name.dispose();
    if (_ownsNameFocus) _nameFocus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final settings = widget.state.settings;
    final padding = MediaQuery.paddingOf(context);
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    return LayoutBuilder(
      builder: (context, constraints) {
        final narrow = constraints.maxWidth < 460;
        final horizontalPadding = narrow ? 16.0 : 22.0;
        final cardSpacing = narrow ? 12.0 : 16.0;
        return ColoredBox(
          color: Colors.transparent,
          child: DefaultTextStyle.merge(
            style: const TextStyle(color: _GlassStyle.textPrimary),
            child: IconTheme.merge(
              data: const IconThemeData(color: _GlassStyle.icon, size: 22),
              child: FocusTraversalGroup(
                policy: ReadingOrderTraversalPolicy(),
                child: SingleChildScrollView(
                  padding: EdgeInsets.fromLTRB(
                    horizontalPadding,
                    padding.top + (narrow ? 8 : 16),
                    horizontalPadding,
                    24,
                  ),
                  child: Column(
                    children: [
                      if (widget.showHeader) ...[
                        const SizedBox(
                          height: 48,
                          child: Center(
                            child: Text(
                              '设置',
                              maxLines: 1,
                              style: TextStyle(
                                fontSize: 24,
                                color: _GlassStyle.textPrimary,
                                fontWeight: FusionTypography.boldFontWeight,
                                fontVariations: FusionTypography.bold,
                              ),
                            ),
                          ),
                        ),
                        SizedBox(height: cardSpacing),
                      ],
                      _card([_nameEditor()]),
                      SizedBox(height: cardSpacing),
                      _card([
                        _switchRow(
                          asset: 'assets/miplay-logo.svg',
                          preserveAssetColors: true,
                          title: 'MiPlay',
                          value: settings.miPlayEnabled,
                          focusNode: _miPlayFocus,
                          focusNameOnUp: true,
                          onChanged: widget.onMiPlay,
                        ),
                        _switchRow(
                          asset: 'assets/airplay-broadcast.svg',
                          title: 'AirPlay',
                          value: settings.airPlayEnabled,
                          onChanged: widget.onAirPlay,
                        ),
                        _switchRow(
                          asset: FusionIcons.dlna,
                          title: 'DLNA',
                          value: settings.dlnaEnabled,
                          onChanged: widget.onDlna,
                        ),
                      ]),
                      SizedBox(height: cardSpacing),
                      _card(dividers: false, [
                        for (final identity in MiPlayDeviceIdentity.values)
                          _identityRow(
                            identity,
                            settings.miPlayDeviceIdentity,
                            reduceMotion: reduceMotion,
                          ),
                        // Xiaomi's device picker is version dependent: some
                        // HyperOS builds hide "speaker" receivers from the
                        // MiPlay list while "television" works. Surface that
                        // instead of letting users think the receiver vanished.
                        const Padding(
                          padding: EdgeInsets.fromLTRB(20, 2, 20, 14),
                          child: Text(
                            '设备类型影响小米手机妙播列表中的图标与可见性；'
                            '若选「音响」后手机里找不到设备，请改用「电视」。',
                            style: TextStyle(
                              fontSize: 12,
                              height: 1.4,
                              color: _GlassStyle.textSecondary,
                            ),
                          ),
                        ),
                      ]),
                      SizedBox(height: cardSpacing),
                      _card([
                        _switchRow(
                          asset: 'assets/icon-power-settings-new.svg',
                          title: '开机自启',
                          value: settings.startupEnabled,
                          onChanged: widget.onStartup,
                        ),
                        _switchRow(
                          asset: 'assets/auto-wake.svg',
                          title: '自动唤起',
                          value: settings.autoWakeEnabled,
                          onChanged: widget.onAutoWake,
                        ),
                        _switchRow(
                          icon: Icons.auto_awesome_rounded,
                          title: '高级效果',
                          value: settings.advancedEffectsEnabled,
                          onChanged: widget.onAdvancedEffects,
                        ),
                      ]),
                      SizedBox(height: cardSpacing),
                      _card([
                        _aboutRow(),
                        if (_showUpdateRow) _checkUpdateRow(),
                        _exportLogsRow(),
                      ]),
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  // ------------------------------------------------------------------
  // Rows
  // ------------------------------------------------------------------

  Widget _rowContent({
    required Widget leading,
    required String title,
    String? subtitle,
    Widget? trailing,
    bool enabled = true,
  }) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 9),
    child: Row(
      children: [
        SizedBox(width: 42, height: 42, child: Center(child: leading)),
        const SizedBox(width: 14),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 16,
                  color: enabled
                      ? _GlassStyle.textPrimary
                      : _GlassStyle.textDisabled,
                  fontWeight: FusionTypography.semiBoldFontWeight,
                  fontVariations: FusionTypography.semiBold,
                ),
              ),
              if (subtitle != null) ...[
                const SizedBox(height: 2),
                Text(
                  subtitle,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: 12,
                    height: 1.35,
                    color: _GlassStyle.textSecondary,
                  ),
                ),
              ],
            ],
          ),
        ),
        if (trailing != null) ...[const SizedBox(width: 12), trailing],
      ],
    ),
  );

  Widget _row({
    required Widget leading,
    required String title,
    String? subtitle,
    Widget? trailing,
    VoidCallback? onTap,
    Key? key,
    bool enabled = true,
  }) => RemoteFocusFrame(
    enabled: enabled && onTap != null,
    onActivate: onTap,
    shape: const G2ContinuousBorder(radius: _GlassStyle.rowRadius),
    child: InkWell(
      key: key,
      onTap: enabled ? onTap : null,
      child: _rowContent(
        leading: leading,
        title: title,
        subtitle: subtitle,
        trailing: trailing,
        enabled: enabled,
      ),
    ),
  );

  Widget _glassIcon(String asset, {bool preserveColors = false}) =>
      FusionVectorIcon(
        asset,
        size: 22,
        color: preserveColors ? null : _GlassStyle.icon,
      );

  Widget _aboutRow() => _row(
    leading: _glassIcon(FusionIcons.about),
    title: '关于应用',
    trailing: const Text(
      fusionPlayVersion,
      style: TextStyle(fontSize: 13, color: _GlassStyle.textSecondary),
    ),
  );

  Widget _exportLogsRow() {
    final enabled = !widget.state.busy && !_exportingLogs;
    void export() {
      if (enabled) _exportLogs();
    }

    return _row(
      key: const ValueKey('export-logs'),
      enabled: enabled,
      onTap: export,
      leading: _glassIcon(FusionIcons.exportLogs),
      title: '导出日志',
      trailing: _exportingLogs
          ? const SizedBox.square(
              dimension: 20,
              child: CircularProgressIndicator(
                strokeWidth: 2.5,
                color: _GlassStyle.icon,
              ),
            )
          : const Icon(
              Icons.ios_share_rounded,
              size: 20,
              color: _GlassStyle.iconMuted,
            ),
    );
  }

  Future<void> _exportLogs() async {
    if (_exportingLogs) return;
    setState(() => _exportingLogs = true);
    try {
      await widget.onExportLogs();
    } finally {
      if (mounted) setState(() => _exportingLogs = false);
    }
  }

  bool get _showUpdateRow =>
      (widget.isAndroid ?? defaultTargetPlatform == TargetPlatform.android) &&
      widget.onCheckUpdate != null &&
      widget.onInstallUpdate != null;

  Widget _checkUpdateRow() {
    final busy = _checkingUpdate || _downloadingUpdate;
    final enabled = !busy && !widget.state.busy;
    void check() {
      if (enabled) unawaited(_checkForUpdate());
    }

    return _row(
      key: const ValueKey('check-update'),
      enabled: enabled,
      onTap: check,
      leading: const Icon(
        Icons.system_update_rounded,
        size: 22,
        color: _GlassStyle.icon,
      ),
      title: '检查更新',
      trailing: busy
          ? const SizedBox.square(
              dimension: 20,
              child: CircularProgressIndicator(
                strokeWidth: 2.5,
                color: _GlassStyle.icon,
              ),
            )
          : const Icon(
              Icons.chevron_right_rounded,
              size: 20,
              color: _GlassStyle.iconMuted,
            ),
    );
  }

  Future<void> _checkForUpdate() async {
    if (_checkingUpdate || _downloadingUpdate) return;
    final onCheck = widget.onCheckUpdate;
    final onInstall = widget.onInstallUpdate;
    if (onCheck == null || onInstall == null) return;

    await runUpdateFlow(
      context: context,
      onCheckUpdate: onCheck,
      onInstallUpdate: onInstall,
      onCheckingChanged: (checking) {
        if (mounted) setState(() => _checkingUpdate = checking);
      },
      onDownloadingChanged: (downloading) {
        if (mounted) setState(() => _downloadingUpdate = downloading);
      },
    );
  }

  // ------------------------------------------------------------------
  // Receiver name editor
  // ------------------------------------------------------------------

  Widget _nameEditor() {
    final dirty = _name.text.trim() != _savedName.trim();
    final restoreEnabled =
        !widget.state.busy && (_savedName.isNotEmpty || dirty);
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const _SectionTitle(asset: FusionIcons.customName, text: '自定义名称'),
          const SizedBox(height: 12),
          Focus(
            canRequestFocus: false,
            skipTraversal: true,
            onKeyEvent: (_, event) {
              if (event is KeyDownEvent &&
                  event.logicalKey == LogicalKeyboardKey.arrowDown) {
                _miPlayFocus.requestFocus();
                return KeyEventResult.handled;
              }
              return KeyEventResult.ignored;
            },
            child: TextField(
              key: const ValueKey('receiver-name'),
              focusNode: _nameFocus,
              controller: _name,
              maxLength: 63,
              textInputAction: TextInputAction.done,
              cursorColor: Colors.white,
              style: const TextStyle(
                fontSize: 16,
                color: _GlassStyle.textPrimary,
              ),
              decoration: InputDecoration(
                counterText: '',
                hintText: widget.state.receiverName,
                hintStyle: const TextStyle(color: _GlassStyle.hint),
                filled: true,
                fillColor: _GlassStyle.inputFill,
                border: const OutlineInputBorder(
                  borderRadius: BorderRadius.all(Radius.circular(16)),
                  borderSide: BorderSide(color: _GlassStyle.inputBorder),
                ),
                enabledBorder: const OutlineInputBorder(
                  borderRadius: BorderRadius.all(Radius.circular(16)),
                  borderSide: BorderSide(color: _GlassStyle.inputBorder),
                ),
                focusedBorder: const OutlineInputBorder(
                  borderRadius: BorderRadius.all(Radius.circular(16)),
                  borderSide: BorderSide(
                    color: _GlassStyle.inputFocusBorder,
                    width: 1.5,
                  ),
                ),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 14,
                ),
              ),
            ),
          ),
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              _actionButton(
                label: '恢复默认',
                primary: false,
                active: restoreEnabled,
                onPressed: restoreEnabled
                    ? () {
                        _name.clear();
                        widget.onReceiverName('');
                        setState(() => _savedName = '');
                      }
                    : null,
              ),
              const SizedBox(width: 10),
              _actionButton(
                label: '应用',
                primary: true,
                active: !widget.state.busy && dirty,
                onPressed: !widget.state.busy && dirty
                    ? () {
                        final value = _name.text.trim();
                        widget.onReceiverName(value);
                        setState(() => _savedName = value);
                      }
                    : null,
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _actionButton({
    required String label,
    required bool active,
    required bool primary,
    required VoidCallback? onPressed,
  }) => RemoteFocusFrame(
    enabled: active,
    onActivate: onPressed,
    shape: const G2ContinuousBorder(radius: 999),
    child: AnimatedContainer(
      duration: MediaQuery.maybeOf(context)?.disableAnimations ?? false
          ? Duration.zero
          : SettingsStyle.actionColorDuration,
      decoration: ShapeDecoration(
        color: !active
            ? _GlassStyle.actionFillDisabled
            : primary
            ? _GlassStyle.actionPrimaryFill
            : _GlassStyle.actionFill,
        shape: const G2ContinuousBorder(radius: 999),
      ),
      child: Material(
        color: Colors.transparent,
        shape: const G2ContinuousBorder(radius: 999),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onPressed,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
            child: Text(
              label,
              style: TextStyle(
                fontSize: 14,
                color: !active
                    ? _GlassStyle.textDisabled
                    : primary
                    ? _GlassStyle.actionPrimaryText
                    : _GlassStyle.textPrimary,
                fontWeight: FusionTypography.mediumFontWeight,
                fontVariations: FusionTypography.medium,
              ),
            ),
          ),
        ),
      ),
    ),
  );

  // ------------------------------------------------------------------
  // Device identity
  // ------------------------------------------------------------------

  Widget _identityRow(
    MiPlayDeviceIdentity identity,
    MiPlayDeviceIdentity selected, {
    required bool reduceMotion,
  }) {
    final isSelected = identity == selected;
    final enabled = !widget.state.busy;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      child: RemoteFocusFrame(
        enabled: enabled,
        onActivate: () => widget.onMiPlayIdentity(identity),
        shape: const G2ContinuousBorder(radius: 14),
        child: InkWell(
          onTap: enabled ? () => widget.onMiPlayIdentity(identity) : null,
          child: AnimatedContainer(
            duration: reduceMotion
                ? Duration.zero
                : const Duration(milliseconds: 160),
            decoration: ShapeDecoration(
              color: isSelected ? _GlassStyle.rowHighlight : Colors.transparent,
              shape: const G2ContinuousBorder(radius: 14),
            ),
            child: _rowContent(
              leading: _identityIcon(identity),
              title: identity.label,
              enabled: enabled,
              trailing: ExcludeFocus(
                child: SizedBox.square(
                  dimension: 36,
                  child: Center(
                    child: Icon(
                      key: reduceMotion
                          ? ValueKey(
                              'identity-${identity.name}-${selected.name}',
                            )
                          : ValueKey('identity-${identity.name}'),
                      isSelected
                          ? Icons.check_circle_rounded
                          : Icons.circle_outlined,
                      size: isSelected ? 22 : 20,
                      color: isSelected
                          ? _GlassStyle.accent
                          : _GlassStyle.iconMuted.withValues(alpha: .45),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _identityIcon(MiPlayDeviceIdentity identity) {
    const white = ColorFilter.mode(_GlassStyle.icon, BlendMode.srcIn);
    return switch (identity) {
      MiPlayDeviceIdentity.television => SvgPicture.asset(
        'assets/device-television.svg',
        width: 24,
        height: 24,
        colorFilter: white,
      ),
      MiPlayDeviceIdentity.speaker => SvgPicture.asset(
        'assets/device-speaker.svg',
        width: 24,
        height: 24,
        colorFilter: white,
      ),
      MiPlayDeviceIdentity.vehicle => SvgPicture.asset(
        'assets/device-vehicle.svg',
        width: 24,
        height: 24,
        colorFilter: white,
      ),
      MiPlayDeviceIdentity.tablet => SvgPicture.asset(
        'assets/device-tablet.svg',
        width: 24,
        height: 24,
        colorFilter: white,
      ),
    };
  }

  // ------------------------------------------------------------------
  // Toggles
  // ------------------------------------------------------------------

  Widget _switchRow({
    String? asset,
    IconData? icon,
    bool preserveAssetColors = false,
    FocusNode? focusNode,
    bool focusNameOnUp = false,
    required String title,
    required bool value,
    required ValueChanged<bool> onChanged,
  }) {
    final enabled = !widget.state.busy;
    final leading = asset != null
        ? SvgPicture.asset(
            asset,
            width: 22,
            height: 22,
            colorFilter: preserveAssetColors
                ? null
                : const ColorFilter.mode(_GlassStyle.icon, BlendMode.srcIn),
          )
        : Icon(icon, size: 22, color: _GlassStyle.icon);
    return RemoteFocusFrame(
      enabled: enabled,
      focusNode: focusNode,
      onActivate: () => onChanged(!value),
      shortcuts: focusNameOnUp
          ? const <ShortcutActivator, Intent>{
              SingleActivator(LogicalKeyboardKey.arrowUp):
                  _FocusReceiverNameIntent(),
            }
          : const <ShortcutActivator, Intent>{},
      actions: focusNameOnUp
          ? <Type, Action<Intent>>{
              _FocusReceiverNameIntent:
                  CallbackAction<_FocusReceiverNameIntent>(
                    onInvoke: (_) {
                      _nameFocus.requestFocus();
                      return null;
                    },
                  ),
            }
          : const <Type, Action<Intent>>{},
      shape: const G2ContinuousBorder(radius: _GlassStyle.rowRadius),
      child: InkWell(
        onTap: enabled ? () => onChanged(!value) : null,
        child: _rowContent(
          leading: leading,
          title: title,
          enabled: enabled,
          trailing: ExcludeFocus(
            child: SizedBox(
              width: SettingsStyle.switchWidth,
              height: SettingsStyle.switchHeight,
              child: _G2SettingsSwitch(
                value: value,
                onChanged: enabled ? onChanged : null,
              ),
            ),
          ),
        ),
      ),
    );
  }

  // ------------------------------------------------------------------
  // Surfaces
  // ------------------------------------------------------------------

  Widget _card(List<Widget> children, {bool dividers = true}) {
    final rows = <Widget>[];
    for (var index = 0; index < children.length; index++) {
      rows.add(children[index]);
      if (dividers && index != children.length - 1) {
        rows.add(
          const Divider(
            height: 1,
            thickness: 1,
            indent: 76,
            endIndent: 18,
            color: _GlassStyle.divider,
          ),
        );
      }
    }
    return DecoratedBox(
      decoration: const ShapeDecoration(
        color: _GlassStyle.surface,
        shape: G2ContinuousBorder(
          radius: _GlassStyle.cardRadius,
          side: BorderSide(color: _GlassStyle.surfaceBorder),
        ),
      ),
      child: Column(children: rows),
    );
  }
}

class _G2SettingsSwitch extends StatelessWidget {
  const _G2SettingsSwitch({required this.value, required this.onChanged});

  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    return Semantics(
      button: true,
      enabled: onChanged != null,
      toggled: value,
      onTap: onChanged == null ? null : () => onChanged!(!value),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onChanged == null ? null : () => onChanged!(!value),
        child: Opacity(
          opacity: onChanged == null ? .38 : 1,
          child: TweenAnimationBuilder<double>(
            tween: Tween(end: value ? 1 : 0),
            duration: reduceMotion ? Duration.zero : FusionMotion.stateChange,
            curve: FusionMotion.easeOut,
            builder: (context, progress, _) {
              final trackColor = Color.lerp(
                _GlassStyle.switchOffTrack,
                SettingsStyle.switchOnTrack,
                progress,
              )!;
              return DecoratedBox(
                decoration: ShapeDecoration(
                  color: trackColor,
                  shape: const G2ContinuousBorder(
                    radius: SettingsStyle.switchHeight / 2,
                  ),
                ),
                child: Stack(
                  children: [
                    Positioned(
                      left: 4 + 20 * progress,
                      top: 4,
                      child: DecoratedBox(
                        decoration: ShapeDecoration(
                          color: Color.lerp(
                            _GlassStyle.switchOffThumb,
                            _GlassStyle.switchOnThumb,
                            progress,
                          ),
                          shape: const CircleBorder(),
                        ),
                        child: const SizedBox.square(dimension: 24),
                      ),
                    ),
                  ],
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}

class _FocusReceiverNameIntent extends Intent {
  const _FocusReceiverNameIntent();
}

class _SectionTitle extends StatelessWidget {
  const _SectionTitle({required this.asset, required this.text});
  final String asset;
  final String text;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      SizedBox(
        width: 42,
        height: 42,
        child: Center(
          child: FusionVectorIcon(asset, size: 22, color: _GlassStyle.icon),
        ),
      ),
      const SizedBox(width: 14),
      Text(
        text,
        style: const TextStyle(
          fontSize: 16,
          color: _GlassStyle.textPrimary,
          fontWeight: FusionTypography.semiBoldFontWeight,
          fontVariations: FusionTypography.semiBold,
        ),
      ),
    ],
  );
}
