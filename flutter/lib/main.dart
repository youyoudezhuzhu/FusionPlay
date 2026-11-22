import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'backend/app_controller.dart';
import 'backend/android/native_app_controller.dart';
import 'backend/windows/windows_app_controller.dart';
import 'backend/ui_diagnostics.dart';
import 'backend/windows/player_log.dart';
import 'models/app_state.dart';
import 'theme/fusion_theme.dart';
import 'ui/app_menu.dart';
import 'ui/glass_appearance.dart';
import 'ui/artwork.dart';
import 'ui/player_view.dart';
import 'ui/remote_focus.dart';
import 'ui/frosted_glass.dart';
import 'ui/settings_view.dart';
import 'ui/update_dialogs.dart';
import 'ui/vector_icon.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isWindows) {
    installUiDiagnostics(recordWindowsPlayerError);
  } else if (Platform.isAndroid) {
    installUiDiagnostics(
      (message) => const MethodChannel(
        'com.fusionplay.android/runtime',
      ).invokeMethod<void>('reportPlayerError', {'value': message}),
    );
  }
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
  });

  final FusionPlayController controller;
  final AppState state;

  /// Overrides platform update availability (tests).
  final bool? isAndroid;

  @override
  State<FusionPlayHome> createState() => _FusionPlayHomeState();
}

class _FusionPlayHomeState extends State<FusionPlayHome> {
  static const _navigationKeyChannel = MethodChannel(
    'com.fusionplay.android/navigation_keys',
  );

  MorphStage? _morphStage;
  Rect? _anchorRect;
  VoidCallback? _pendingMorphAction;
  final GlobalKey _menuButtonKey = GlobalKey(debugLabel: 'player-menu-button');
  final GlobalKey _playerStackKey = GlobalKey(debugLabel: 'player-host-stack');

  bool get menuOpen => _morphStage == MorphStage.menu;
  bool get settingsOpen => _morphStage == MorphStage.settings;
  bool get _morphOpen => _morphStage != null;
  final FocusNode _menuButtonFocus = FocusNode(debugLabel: 'player-menu');
  final FocusNode _playerProgressFocus = FocusNode(
    debugLabel: 'player-progress',
  );
  final FocusNode _settingsInitialFocus = FocusNode(
    debugLabel: 'settings-miplay',
  );
  final FocusNode _settingsTvInitialFocus = FocusNode(
    debugLabel: 'settings-miplay',
  );
  final FocusNode _settingsNameFocus = FocusNode(debugLabel: 'settings-name');
  final List<FocusNode> _menuItemFocus = List<FocusNode>.generate(
    4,
    (index) => FocusNode(debugLabel: 'menu-item-$index'),
  );
  String? _displayedArtworkSource;
  String? _requestedArtworkSource;
  String? _displayedArtworkIdentity;
  String? _requestedArtworkIdentity;
  int _artworkRequest = 0;
  bool _artworkPreparationStarted = false;
  Timer? _artworkClearTimer;
  DateTime? _artworkRetryAfter;
  Timer? _errorToastTimer;
  String? _errorToastMessage;
  String? _visibleError;

  /// Top error toasts auto-dismiss after this long. The timer must not be
  /// restarted while the same error keeps being reported (AirPlay volume sync
  /// failures used to repeat on every state update, leaving the toast forever).
  static const errorToastDuration = Duration(seconds: 3);

  bool get _isAndroid => widget.isAndroid ?? Platform.isAndroid;

  @override
  void initState() {
    super.initState();
    _requestedArtworkSource = _normalizedArtworkSource(
      widget.state.playback.coverArt ?? widget.state.playback.embeddedArtwork,
    );
    _requestedArtworkIdentity = widget.state.playback.artworkTransitionIdentity;
    _navigationKeyChannel.setMethodCallHandler((call) async {
      if (call.method == 'menu' && mounted) _handleMenuShortcut();
    });
    _syncErrorToast(widget.state.lastError);
  }

  void _syncErrorToast(String? raw) {
    final message = raw?.trim();
    final normalized = message == null || message.isEmpty ? null : message;
    if (normalized == _errorToastMessage) return; // do not restart the timer
    _errorToastTimer?.cancel();
    _errorToastTimer = null;
    _errorToastMessage = normalized;
    _visibleError = normalized;
    if (normalized == null) return;
    _errorToastTimer = Timer(errorToastDuration, () {
      if (!mounted || _errorToastMessage != normalized) return;
      _errorToastTimer = null;
      // Hide locally first: the toast must disappear even if the runtime keeps
      // reporting the same error (AirPlay volume sync), otherwise it would sit
      // on screen forever. clearError is still sent so the native state clears.
      setState(() => _visibleError = null);
      unawaited(widget.controller.clearError());
    });
  }

  void _dismissErrorToast() {
    _errorToastTimer?.cancel();
    _errorToastTimer = null;
    if (mounted) setState(() => _visibleError = null);
    unawaited(widget.controller.clearError());
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

  (String?, int?, String?) _artworkOwner(AppState state) {
    final source = (state.playback.protocol ?? state.activeMediaSource)
        ?.toLowerCase();
    final dlna = source == 'dlna';
    return (
      source,
      dlna ? state.playback.sourceEpoch : null,
      dlna ? state.playback.mediaUrl : null,
    );
  }

  @override
  void didUpdateWidget(covariant FusionPlayHome oldWidget) {
    super.didUpdateWidget(oldWidget);
    _syncErrorToast(widget.state.lastError);
    final nextArtwork = _normalizedArtworkSource(
      widget.state.playback.coverArt ?? widget.state.playback.embeddedArtwork,
    );
    final nextIdentity = widget.state.playback.artworkTransitionIdentity;
    final ownerChanged =
        _artworkOwner(oldWidget.state) != _artworkOwner(widget.state);
    if (ownerChanged) {
      _displayedArtworkSource = null;
      _displayedArtworkIdentity = null;
    }
    if (!ownerChanged &&
        oldWidget.state.playback.embeddedArtwork ==
            widget.state.playback.embeddedArtwork &&
        nextArtwork == _requestedArtworkSource) {
      // Same (or still absent) cover image: now-playing metadata/track identity
      // churn must not restart the artwork flip. Keep the displayed identity
      // until the actual image source changes.
      _requestedArtworkIdentity = nextIdentity;
      final retryAfter = _artworkRetryAfter;
      if (retryAfter != null && !DateTime.now().isBefore(retryAfter)) {
        unawaited(_prepareArtwork(nextArtwork, nextIdentity));
      }
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
    _errorToastTimer?.cancel();
    _menuButtonFocus.dispose();
    _playerProgressFocus.dispose();
    _settingsInitialFocus.dispose();
    _settingsTvInitialFocus.dispose();
    _settingsNameFocus.dispose();
    for (final node in _menuItemFocus) {
      node.dispose();
    }
    super.dispose();
  }

  Future<void> _prepareArtwork(String? source, String? identity) async {
    final request = ++_artworkRequest;
    _artworkClearTimer?.cancel();
    _artworkRetryAfter = null;
    if (source == null) {
      if (_displayedArtworkSource == null) return;
      _artworkClearTimer = Timer(artworkClearGracePeriod, () {
        if (mounted && _requestedArtworkSource == null) {
          setState(() {
            _displayedArtworkSource = null;
            _displayedArtworkIdentity = null;
          });
        }
      });
      return;
    }
    final dlna = _artworkOwner(widget.state).$1 == 'dlna';
    final epoch = widget.state.playback.sourceEpoch;
    final fallback = dlna ? widget.state.playback.embeddedArtwork : null;
    void report(String stage, String outcome) {
      if (!dlna || !Platform.isAndroid) return;
      unawaited(
        const MethodChannel('com.fusionplay.android/runtime')
            .invokeMethod<void>('reportDlnaArtwork', {
              'stage': stage,
              'outcome': outcome,
              'epoch': epoch,
              'fallbackAvailable': fallback != null,
            })
            .catchError((Object _) {}),
      );
    }

    String failureCode(Object error) => error is NetworkImageLoadException
        ? 'http_${error.statusCode}'
        : '${error.runtimeType}';

    Future<void> load(String candidate) async {
      final provider = artworkProvider(candidate);
      if (provider == null) {
        throw const FormatException('Invalid artwork source');
      }
      // The same live cover URL may refer to successive DLNA tracks.
      if (dlna &&
          (candidate.startsWith('http://') ||
              candidate.startsWith('https://'))) {
        await provider.evict();
        if (!mounted || request != _artworkRequest) return;
      }
      Object? failure;
      await precacheImage(
        provider,
        context,
        onError: (error, _) => failure = error,
      );
      if (failure case final Object error) throw error;
    }

    var displayed = source;
    try {
      try {
        await load(source);
      } catch (error) {
        if (mounted && request == _artworkRequest) {
          report(
            source == fallback ? 'embedded' : 'primary',
            failureCode(error),
          );
        }
        if (!mounted ||
            request != _artworkRequest ||
            fallback == null ||
            fallback == source) {
          rethrow;
        }
        try {
          await load(fallback);
          displayed = fallback;
        } catch (error) {
          if (mounted && request == _artworkRequest) {
            report('embedded', failureCode(error));
          }
          rethrow;
        }
      }
      if (!mounted || request != _artworkRequest) return;
      setState(() {
        _displayedArtworkIdentity = _displayedArtworkSource == displayed
            ? _displayedArtworkIdentity
            : identity;
        _displayedArtworkSource = displayed;
      });
      report('display', displayed == fallback ? 'decoded_embedded' : 'decoded');
    } catch (error) {
      if (!mounted || request != _artworkRequest) return;
      if (error is! NetworkImageLoadException || error.statusCode != 403) {
        _artworkRetryAfter = DateTime.now().add(const Duration(seconds: 5));
      }
      report('display', 'unavailable');
    }
  }

  String? _normalizedArtworkSource(String? source) {
    final value = source?.trim();
    return value == null || value.isEmpty ? null : value;
  }

  // ------------------------------------------------------------------
  // Menu / settings / update actions
  // ------------------------------------------------------------------

  Rect? _computeAnchorRect() {
    final buttonBox = _menuButtonKey.currentContext?.findRenderObject();
    final stackBox = _playerStackKey.currentContext?.findRenderObject();
    if (buttonBox is! RenderBox ||
        !buttonBox.hasSize ||
        stackBox is! RenderBox ||
        !stackBox.hasSize) {
      return null;
    }
    final offset =
        buttonBox.localToGlobal(Offset.zero) -
        stackBox.localToGlobal(Offset.zero);
    return offset & buttonBox.size;
  }

  void _openMenu() {
    if (_morphStage != null) return;
    final anchor =
        _computeAnchorRect() ??
        Rect.fromLTWH(
          MediaQuery.sizeOf(context).width - 78,
          MediaQuery.sizeOf(context).height - 78,
          46,
          46,
        );
    setState(() {
      _anchorRect = anchor;
      _morphStage = MorphStage.menu;
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _menuItemFocus.first.requestFocus();
    });
  }

  void _beginClose({VoidCallback? then}) {
    if (_morphStage == null || _morphStage == MorphStage.closing) return;
    _pendingMorphAction = then;
    setState(() => _morphStage = MorphStage.closing);
  }

  void _finishMorphClose() {
    if (!mounted || _morphStage != MorphStage.closing) return;
    final action = _pendingMorphAction;
    _pendingMorphAction = null;
    setState(() {
      _morphStage = null;
      _anchorRect = null;
    });
    _returnFocusToMenuButton();
    if (action != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) action();
      });
    }
  }

  void _openSettings() {
    if (_morphStage != MorphStage.menu) return;
    setState(() => _morphStage = MorphStage.settings);
  }

  void _closeSettings() {
    if (_morphStage != MorphStage.settings) return;
    setState(() => _morphStage = MorphStage.menu);
  }

  void _handleMorphTransitionCompleted(MorphStage stage) {
    if (!mounted) return;
    if (stage == MorphStage.settings) {
      // Never focus the receiver-name field automatically: that would open the
      // keyboard and select text on entry. TV and phone both start on MiPlay.
      if (widget.state.isTelevision) {
        _settingsTvInitialFocus.requestFocus();
      } else {
        _settingsInitialFocus.requestFocus();
      }
    } else if (stage == MorphStage.menu) {
      _menuItemFocus.first.requestFocus();
    }
  }

  void _dismissOverlay() {
    if (_morphStage == MorphStage.settings) {
      _closeSettings();
    } else if (_morphStage == MorphStage.menu) {
      _beginClose();
    }
  }

  void _settingsAction() {
    _openSettings();
  }

  void _exportLogsAction() {
    _beginClose(then: () => unawaited(widget.controller.exportLogs()));
  }

  void _checkForUpdatesAction() {
    _beginClose(
      then: () => unawaited(
        runUpdateFlow(
          context: _playerStackKey.currentContext!,
          onCheckUpdate: widget.controller.checkForUpdate,
          onDownloadUpdate: widget.controller.downloadUpdate,
          onInstallUpdate: widget.controller.installDownloadedUpdate,
          saveUpdate: widget.controller.saveUpdate,
        ),
      ),
    );
  }

  void _showAboutAction() {
    _beginClose(then: () => unawaited(_showAboutDialog()));
  }

  Future<void> _showAboutDialog() async {
    await showGlassDialog<void>(
      context: _playerStackKey.currentContext!,
      dialogKey: const ValueKey('about-dialog'),
      builder: (dialogContext) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'FusionPlay',
            style: TextStyle(
              fontSize: 20,
              fontWeight: FusionTypography.semiBoldFontWeight,
              fontVariations: FusionTypography.semiBold,
              color: Color(0xEBFFFFFF),
            ),
          ),
          const SizedBox(height: 14),
          _aboutRow('版本', fusionPlayVersion),
          _aboutRow(
            '协议',
            playbackProtocolLabel(widget.state.playback.protocol) ?? '—',
          ),
          _aboutRow('设备名', widget.state.receiverName),
          const SizedBox(height: 14),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              FilledButton(
                style: FilledButton.styleFrom(
                  backgroundColor: const Color(0xE6FFFFFF),
                  foregroundColor: const Color(0xFF16181B),
                  padding: const EdgeInsets.symmetric(
                    horizontal: 20,
                    vertical: 12,
                  ),
                  shape: const StadiumBorder(),
                ),
                onPressed: () => Navigator.of(dialogContext).pop(),
                child: const Text(
                  '好',
                  style: TextStyle(
                    fontSize: 14,
                    fontWeight: FusionTypography.mediumFontWeight,
                    fontVariations: FusionTypography.medium,
                  ),
                ),
              ),
            ],
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
              color: FusionGlassColors.supportingText,
            ),
          ),
        ),
        Expanded(
          child: Text(
            value,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 13, color: Color(0xEBFFFFFF)),
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
      _beginClose();
    }
  }

  void _handleMenuShortcut() {
    if (settingsOpen) {
      _closeSettings();
    } else if (menuOpen) {
      _beginClose();
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
      if (_isAndroid || widget.controller.saveUpdate != null)
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
    final artwork = _displayedArtworkSource;
    final playback = state.playback;
    final hasPlayback =
        playback.streamActive ||
        (playback.protocol?.trim().isNotEmpty ?? false) ||
        (playback.mediaUrl?.trim().isNotEmpty ?? false) ||
        (playback.title?.trim().isNotEmpty ?? false) ||
        (playback.artist?.trim().isNotEmpty ?? false) ||
        playback.durationMs != null;
    final isTelevision = state.isTelevision;

    final playerBackdrop = PlayerBackdrop(
      key: ValueKey(_artworkOwner(state)),
      source: artwork,
      hasPlayback: hasPlayback,
    );

    final playerSurface = Stack(
      children: [
        SafeArea(
          child: Padding(
            key: const ValueKey('player-overscan'),
            padding: EdgeInsets.symmetric(
              horizontal: isTelevision ? 28 : 0,
              vertical: isTelevision ? 14 : 0,
            ),
            child: Stack(
              key: _playerStackKey,
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
                      menuButtonKey: _menuButtonKey,
                      onOpenMenu: _openMenu,
                      menuOpen: menuOpen,
                      overlayOpen: _morphOpen,
                      onPlayback: widget.controller.playback,
                      onSeek: widget.controller.seek,
                      onVolume: widget.controller.volume,
                    ),
                  ),
                ),
                if (_morphOpen && _anchorRect != null)
                  Positioned.fill(
                    child: SafeArea(
                      child: FusionMorphOverlay(
                        key: const ValueKey('morph-overlay'),
                        stage: _morphStage!,
                        anchorRect: _anchorRect!,
                        menuItems: _menuItems(),
                        menuFocusNodes: _menuItemFocus,
                        settingsTitle: '设置',
                        settingsChild: SettingsView(
                          state: state,
                          showHeader: false,
                          showMenuActions: false,
                          scrollable: false,
                          initialFocusNode: _settingsInitialFocus,
                          nameFocusNode: _settingsNameFocus,
                          miPlayFocusNode: isTelevision
                              ? _settingsTvInitialFocus
                              : _settingsInitialFocus,
                          onClose: _closeSettings,
                          onReceiverName: widget.controller.setReceiverName,
                          onStartup: widget.controller.setStartupEnabled,
                          onAutoWake: widget.controller.setAutoWakeEnabled,
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
                        ),
                        onDismiss: _dismissOverlay,
                        onSettingsClose: _closeSettings,
                        onClosed: _finishMorphClose,
                        onTransitionCompleted: _handleMorphTransitionCompleted,
                        settingsMaxWidth: isTelevision ? 720 : 560,
                      ),
                    ),
                  ),
                if (_visibleError != null)
                  SafeArea(
                    child: Align(
                      alignment: Alignment.topCenter,
                      child: Padding(
                        padding: const EdgeInsets.all(16),
                        child: FrostedGlass(
                          key: const ValueKey('error-toast'),
                          level: GlassLevel.panel,
                          blurSigma: 9,
                          borderRadius: 22,
                          padding: const EdgeInsets.fromLTRB(18, 8, 6, 8),

                          child: DefaultTextStyle.merge(
                            style: const TextStyle(
                              color: Color(0xEBFFFFFF),
                              fontSize: 14,
                            ),
                            child: IconTheme.merge(
                              data: const IconThemeData(
                                color: Color(0xDEFFFFFF),
                              ),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  Flexible(
                                    child: Text(
                                      _visibleError!,
                                      maxLines: 2,
                                      overflow: TextOverflow.ellipsis,
                                    ),
                                  ),
                                  RemoteFocusFrame(
                                    onActivate: _dismissErrorToast,
                                    borderColor: Colors.white,
                                    shape: const CircleBorder(),
                                    child: IconButton(
                                      onPressed: _dismissErrorToast,
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
                  ),
              ],
            ),
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
        canPop: _morphStage == null,
        onPopInvokedWithResult: (didPop, _) {
          if (didPop) return;
          _handleBackShortcut();
        },
        child: Scaffold(
          backgroundColor: Colors.transparent,
          body: playerSurface,
        ),
      ),
    );
    return GlassAppearance(
      enabled: state.settings.advancedEffectsEnabled,
      child: FrostedScene(
        background: playerBackdrop,
        child: MediaQuery(
          data: mediaQuery.copyWith(
            disableAnimations:
                mediaQuery.disableAnimations ||
                !state.settings.advancedEffectsEnabled,
          ),
          child: FusionFocusStyle(
            borderWidth: isTelevision ? 4 : 3,
            focusScale: isTelevision ? 1.02 : 1,
            child: content,
          ),
        ),
      ),
    );
  }
}
