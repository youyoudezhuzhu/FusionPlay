import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'backend/app_controller.dart';
import 'backend/native_app_controller.dart';
import 'backend/windows_app_controller.dart';
import 'models/app_state.dart';
import 'theme/fusion_theme.dart';
import 'ui/app_menu.dart';
import 'ui/artwork.dart';
import 'ui/player_view.dart';
import 'ui/remote_focus.dart';
import 'ui/settings_view.dart';
import 'ui/update_dialogs.dart';
import 'ui/vector_icon.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final imageCache = PaintingBinding.instance.imageCache;
  imageCache.maximumSize = 8;
  imageCache.maximumSizeBytes = 32 * 1024 * 1024;
  if (Platform.isAndroid) {
    await SystemChrome.setPreferredOrientations(const [
      DeviceOrientation.landscapeLeft,
      DeviceOrientation.landscapeRight,
    ]);
    await SystemChrome.setEnabledSystemUIMode(SystemUiMode.immersiveSticky);
    SystemChrome.setSystemUIOverlayStyle(
      const SystemUiOverlayStyle(
        statusBarColor: Colors.transparent,
        systemNavigationBarColor: Colors.transparent,
        systemNavigationBarDividerColor: Colors.transparent,
        statusBarIconBrightness: Brightness.light,
        systemNavigationBarIconBrightness: Brightness.light,
        systemNavigationBarContrastEnforced: false,
      ),
    );
  }
  runApp(const FusionPlayApplication());
}

class FusionPlayApplication extends StatefulWidget {
  const FusionPlayApplication({super.key});

  @override
  State<FusionPlayApplication> createState() => _FusionPlayApplicationState();
}

class _FusionPlayApplicationState extends State<FusionPlayApplication> {
  late final FusionPlayController controller = _createController();

  /// Android keeps the Kotlin backend; every other desktop target uses the
  /// Rust sidecar over NDJSON.
  FusionPlayController _createController() =>
      Platform.isWindows ? WindowsAppController() : NativeAppController();

  @override
  void initState() {
    super.initState();
    controller.initialize();
  }

  @override
  void dispose() {
    controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    debugShowCheckedModeBanner: false,
    title: 'FusionPlay',
    theme: fusionTheme(),
    builder: (context, child) =>
        FusionTypographyScope(child: child ?? const SizedBox.shrink()),
    home: ListenableBuilder(
      listenable: controller,
      builder: (context, _) =>
          FusionPlayHome(controller: controller, state: controller.state),
    ),
  );
}

class FusionPlayHome extends StatefulWidget {
  const FusionPlayHome({
    super.key,
    required this.controller,
    required this.state,
    this.isAndroid,
    this.forceGlassFallback = false,
  });

  final FusionPlayController controller;
  final AppState state;

  /// Overrides platform update availability (tests).
  final bool? isAndroid;

  /// Test hook: render LiquidGlass surfaces as the frosted fallback.
  final bool forceGlassFallback;

  @override
  State<FusionPlayHome> createState() => _FusionPlayHomeState();
}

class _FusionPlayHomeState extends State<FusionPlayHome> {
  static const _navigationKeyChannel = MethodChannel(
    'com.fusionplay.android/navigation_keys',
  );

  bool menuOpen = false;
  bool settingsOpen = false;
  final FocusNode _menuButtonFocus = FocusNode(debugLabel: 'player-menu');
  final FocusNode _playerProgressFocus = FocusNode(
    debugLabel: 'player-progress',
  );
  final FocusNode _settingsInitialFocus = FocusNode(
    debugLabel: 'settings-miplay',
  );
  final List<FocusNode> _menuItemFocus = List<FocusNode>.generate(
    4,
    (index) => FocusNode(debugLabel: 'menu-item-$index'),
  );
  String? _displayedArtworkSource;
  String? _requestedArtworkSource;
  String? _displayedArtworkIdentity;
  String? _requestedArtworkIdentity;
  bool _artworkPreparationStarted = false;
  Timer? _artworkClearTimer;

  bool get _isAndroid => widget.isAndroid ?? Platform.isAndroid;

  @override
  void initState() {
    super.initState();
    _requestedArtworkSource = _normalizedArtworkSource(
      widget.state.playback.coverArt,
    );
    _requestedArtworkIdentity = widget.state.playback.artworkTransitionIdentity;
    _navigationKeyChannel.setMethodCallHandler((call) async {
      if (call.method == 'menu' && mounted) _handleMenuShortcut();
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_artworkPreparationStarted) return;
    _artworkPreparationStarted = true;
    unawaited(
      _prepareArtwork(_requestedArtworkSource, _requestedArtworkIdentity),
    );
  }

  @override
  void didUpdateWidget(covariant FusionPlayHome oldWidget) {
    super.didUpdateWidget(oldWidget);
    final nextArtwork = _normalizedArtworkSource(
      widget.state.playback.coverArt,
    );
    final nextIdentity = widget.state.playback.artworkTransitionIdentity;
    if (nextArtwork == _requestedArtworkSource &&
        nextIdentity == _requestedArtworkIdentity) {
      return;
    }
    _requestedArtworkSource = nextArtwork;
    _requestedArtworkIdentity = nextIdentity;
    unawaited(_prepareArtwork(nextArtwork, nextIdentity));
  }

  @override
  void dispose() {
    _navigationKeyChannel.setMethodCallHandler(null);
    _artworkClearTimer?.cancel();
    _menuButtonFocus.dispose();
    _playerProgressFocus.dispose();
    _settingsInitialFocus.dispose();
    for (final node in _menuItemFocus) {
      node.dispose();
    }
    super.dispose();
  }

  Future<void> _prepareArtwork(String? source, String? identity) async {
    _artworkClearTimer?.cancel();
    if (source == null) {
      if (_displayedArtworkSource == null) return;
      _artworkClearTimer = Timer(artworkClearGracePeriod, () {
        if (mounted &&
            _requestedArtworkSource == source &&
            _requestedArtworkIdentity == identity) {
          setState(() {
            _displayedArtworkSource = null;
            _displayedArtworkIdentity = identity;
          });
        }
      });
      return;
    }
    final provider = artworkProvider(source);
    if (provider == null) return;
    try {
      await precacheImage(provider, context);
      if (mounted &&
          _requestedArtworkSource == source &&
          _requestedArtworkIdentity == identity) {
        setState(() {
          _displayedArtworkSource = source;
          _displayedArtworkIdentity = identity;
        });
      }
    } catch (_) {
      // Preserve the decoded previous cover through transient load failures.
    }
  }

  String? _normalizedArtworkSource(String? source) {
    final value = source?.trim();
    return value == null || value.isEmpty ? null : value;
  }

  // ------------------------------------------------------------------
  // Menu / settings / update actions
  // ------------------------------------------------------------------

  void _openMenu() {
    if (menuOpen || settingsOpen) return;
    setState(() => menuOpen = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _menuItemFocus.first.requestFocus();
    });
  }

  void _closeMenu({bool returnFocus = true}) {
    if (!menuOpen) return;
    setState(() => menuOpen = false);
    if (!returnFocus) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _menuButtonFocus.requestFocus();
    });
  }

  void _openSettings() {
    _closeMenu(returnFocus: false);
    setState(() => settingsOpen = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _settingsInitialFocus.requestFocus();
    });
  }

  void _closeSettings() {
    if (!settingsOpen) return;
    setState(() => settingsOpen = false);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _menuButtonFocus.requestFocus();
    });
  }

  void _settingsAction() {
    _openSettings();
  }

  void _exportLogsAction() {
    _closeMenu(returnFocus: false);
    unawaited(widget.controller.exportLogs());
    _returnFocusToMenuButton();
  }

  Future<void> _checkForUpdatesAction() async {
    _closeMenu(returnFocus: false);
    await runUpdateFlow(
      context: context,
      onCheckUpdate: widget.controller.checkForUpdate,
      onInstallUpdate: widget.controller.downloadAndInstallUpdate,
    );
    _returnFocusToMenuButton();
  }

  Future<void> _showAboutAction() async {
    _closeMenu(returnFocus: false);
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: Colors.white,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(24)),
        title: const Text(
          'FusionPlay',
          style: TextStyle(
            fontSize: 20,
            fontWeight: FusionTypography.semiBoldFontWeight,
            fontVariations: FusionTypography.semiBold,
            color: SettingsStyle.primaryText,
          ),
        ),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _aboutRow('版本', fusionPlayVersion),
            _aboutRow(
              '协议',
              playbackProtocolLabel(widget.state.playback.protocol) ?? '—',
            ),
            _aboutRow('设备名', widget.state.receiverName),
          ],
        ),
        actions: [
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: Colors.black,
              foregroundColor: Colors.white,
            ),
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: const Text('好'),
          ),
        ],
      ),
    );
    _returnFocusToMenuButton();
  }

  Widget _aboutRow(String label, String value) => Padding(
    padding: const EdgeInsets.only(bottom: 6),
    child: Row(
      children: [
        SizedBox(
          width: 64,
          child: Text(
            label,
            style: const TextStyle(
              fontSize: 13,
              color: SettingsStyle.secondaryText,
            ),
          ),
        ),
        Expanded(
          child: Text(
            value,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
              fontSize: 13,
              color: SettingsStyle.primaryText,
            ),
          ),
        ),
      ],
    ),
  );

  void _returnFocusToMenuButton() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _menuButtonFocus.requestFocus();
    });
  }

  void _handleBackShortcut() {
    if (settingsOpen) {
      _closeSettings();
    } else if (menuOpen) {
      _closeMenu();
    }
  }

  void _handleMenuShortcut() {
    if (settingsOpen) {
      _closeSettings();
    } else if (menuOpen) {
      _closeMenu();
    } else {
      _openMenu();
    }
  }

  List<FusionMenuItem> _menuItems() {
    final items = <FusionMenuItem>[
      FusionMenuItem(
        icon: const FusionVectorIcon(
          FusionIcons.settings,
          size: 19,
          color: Colors.white,
        ),
        label: '设置',
        onSelected: _settingsAction,
      ),
      if (_isAndroid)
        FusionMenuItem(
          icon: const Icon(Icons.system_update_rounded),
          label: '检查更新',
          onSelected: _checkForUpdatesAction,
        ),
      FusionMenuItem(
        icon: const FusionVectorIcon(
          FusionIcons.exportLogs,
          size: 19,
          color: Colors.white,
        ),
        label: '导出日志',
        onSelected: _exportLogsAction,
      ),
      FusionMenuItem(
        icon: const FusionVectorIcon(
          FusionIcons.about,
          size: 19,
          color: Colors.white,
        ),
        label: '关于',
        onSelected: _showAboutAction,
      ),
    ];
    return items;
  }

  // ------------------------------------------------------------------
  // Build
  // ------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final mediaQuery = MediaQuery.of(context);
    final state = widget.state;
    final advancedEffectsEnabled =
        state.settings.advancedEffectsEnabled && !mediaQuery.disableAnimations;
    final artwork = _displayedArtworkSource;
    final hasArtwork = artwork != null && artwork.isNotEmpty;
    final playback = state.playback;
    final hasPlayback =
        playback.streamActive ||
        (playback.protocol?.trim().isNotEmpty ?? false) ||
        (playback.mediaUrl?.trim().isNotEmpty ?? false) ||
        (playback.title?.trim().isNotEmpty ?? false) ||
        (playback.artist?.trim().isNotEmpty ?? false) ||
        playback.durationMs != null;

    final playerSurface = Stack(
      children: [
        Positioned.fill(
          child: ArtworkBackground(source: artwork, hasPlayback: hasPlayback),
        ),
        // Dark readability gradient over the artwork (YAQMC-style restraint).
        Positioned.fill(
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: hasArtwork
                    ? const [Color(0xA6101316), Color(0xE6101316)]
                    : const [Color(0xCC101316), Color(0xF0101316)],
              ),
            ),
          ),
        ),
        SafeArea(
          child: Stack(
            children: [
              Positioned.fill(
                child: ExcludeFocus(
                  excluding: menuOpen || settingsOpen,
                  child: PlayerView(
                    state: state,
                    artworkSource: artwork,
                    artworkIdentity: _displayedArtworkIdentity,
                    progressFocusNode: _playerProgressFocus,
                    menuFocusNode: _menuButtonFocus,
                    onOpenMenu: _openMenu,
                    menuOpen: menuOpen,
                    forceGlassFallback: widget.forceGlassFallback,
                    onPlayback: widget.controller.playback,
                    onSeek: widget.controller.seek,
                    onVolume: widget.controller.volume,
                  ),
                ),
              ),
              if (menuOpen)
                Positioned.fill(
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () => _closeMenu(),
                  ),
                ),
              Positioned(
                top: 70,
                right: 22,
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 200),
                  reverseDuration: const Duration(milliseconds: 140),
                  switchInCurve: Curves.easeOutCubic,
                  switchOutCurve: Curves.easeInCubic,
                  transitionBuilder: (child, animation) {
                    if (mediaQuery.disableAnimations) return child;
                    final curved = CurvedAnimation(
                      parent: animation,
                      curve: Curves.easeOutCubic,
                    );
                    return FadeTransition(
                      opacity: curved,
                      child: ScaleTransition(
                        alignment: Alignment.topRight,
                        scale: Tween<double>(
                          begin: .96,
                          end: 1,
                        ).animate(curved),
                        child: SlideTransition(
                          position: Tween<Offset>(
                            begin: const Offset(0, .03),
                            end: Offset.zero,
                          ).animate(curved),
                          child: child,
                        ),
                      ),
                    );
                  },
                  child: menuOpen
                      ? FusionAppMenu(
                          key: const ValueKey('app-menu'),
                          items: _menuItems(),
                          focusNodes: _menuItemFocus,
                          onDismiss: () => _closeMenu(),
                          forceFallback: widget.forceGlassFallback,
                        )
                      : const SizedBox.shrink(key: ValueKey('app-menu-closed')),
                ),
              ),
              if (settingsOpen)
                Positioned.fill(
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: _closeSettings,
                    child: ColoredBox(
                      color: Colors.black.withValues(alpha: .38),
                    ),
                  ),
                ),
              Positioned.fill(
                child: IgnorePointer(
                  ignoring: !settingsOpen,
                  child: ExcludeFocus(
                    excluding: !settingsOpen,
                    child: AnimatedSwitcher(
                      duration: advancedEffectsEnabled
                          ? const Duration(milliseconds: 220)
                          : Duration.zero,
                      reverseDuration: advancedEffectsEnabled
                          ? const Duration(milliseconds: 160)
                          : Duration.zero,
                      switchInCurve: Curves.easeOutCubic,
                      switchOutCurve: Curves.easeInCubic,
                      transitionBuilder: (child, animation) {
                        if (mediaQuery.disableAnimations) return child;
                        final curved = CurvedAnimation(
                          parent: animation,
                          curve: Curves.easeOutCubic,
                        );
                        return FadeTransition(
                          opacity: curved,
                          child: ScaleTransition(
                            scale: Tween<double>(
                              begin: .97,
                              end: 1,
                            ).animate(curved),
                            child: child,
                          ),
                        );
                      },
                      child: settingsOpen
                          ? FusionSettingsSheet(
                              key: const ValueKey('settings-sheet'),
                              title: '设置',
                              forceFallback: widget.forceGlassFallback,
                              onClose: _closeSettings,
                              child: SettingsView(
                                state: state,
                                showHeader: false,
                                initialFocusNode: _settingsInitialFocus,
                                onClose: _closeSettings,
                                onReceiverName:
                                    widget.controller.setReceiverName,
                                onStartup: widget.controller.setStartupEnabled,
                                onAutoWake:
                                    widget.controller.setAutoWakeEnabled,
                                onAdvancedEffects:
                                    widget.controller.setAdvancedEffectsEnabled,
                                onMiPlay: (value) => widget.controller
                                    .setProtocolEnabled('miplay', value),
                                onMiPlayIdentity:
                                    widget.controller.setMiPlayDeviceIdentity,
                                onAirPlay: (value) => widget.controller
                                    .setProtocolEnabled('airplay', value),
                                onDlna: (value) => widget.controller
                                    .setProtocolEnabled('dlna', value),
                                onExportLogs: widget.controller.exportLogs,
                                isAndroid: _isAndroid,
                                onCheckUpdate: widget.controller.checkForUpdate,
                                onInstallUpdate:
                                    widget.controller.downloadAndInstallUpdate,
                              ),
                            )
                          : const SizedBox.shrink(
                              key: ValueKey('settings-sheet-closed'),
                            ),
                    ),
                  ),
                ),
              ),
              if (state.lastError != null)
                SafeArea(
                  child: Align(
                    alignment: Alignment.topCenter,
                    child: Padding(
                      padding: const EdgeInsets.all(16),
                      child: Material(
                        color: const Color(0xFFE3E3E3),
                        borderRadius: BorderRadius.circular(22),
                        child: Padding(
                          padding: const EdgeInsets.only(
                            left: 18,
                            right: 6,
                            top: 6,
                            bottom: 6,
                          ),
                          child: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Flexible(
                                child: Text(
                                  state.lastError!,
                                  maxLines: 2,
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                              RemoteFocusFrame(
                                onActivate: widget.controller.clearError,
                                shape: const CircleBorder(),
                                child: IconButton(
                                  onPressed: widget.controller.clearError,
                                  icon: const FusionVectorIcon(
                                    FusionIcons.close,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ],
    );

    final content = CallbackShortcuts(
      bindings: <ShortcutActivator, VoidCallback>{
        const SingleActivator(LogicalKeyboardKey.escape): _handleBackShortcut,
        const SingleActivator(LogicalKeyboardKey.browserBack):
            _handleBackShortcut,
        const SingleActivator(LogicalKeyboardKey.goBack): _handleBackShortcut,
        const SingleActivator(LogicalKeyboardKey.contextMenu):
            _handleMenuShortcut,
      },
      child: PopScope(
        canPop: !settingsOpen && !menuOpen,
        onPopInvokedWithResult: (didPop, _) {
          if (didPop) return;
          if (settingsOpen) {
            _closeSettings();
          } else if (menuOpen) {
            _closeMenu();
          }
        },
        child: Scaffold(
          backgroundColor: const Color(0xFF101316),
          body: playerSurface,
        ),
      ),
    );
    return MediaQuery(
      data: mediaQuery.copyWith(
        disableAnimations:
            mediaQuery.disableAnimations ||
            !state.settings.advancedEffectsEnabled,
      ),
      child: content,
    );
  }
}
