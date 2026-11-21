import 'dart:async';
import 'dart:js_interop';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../../models/app_state.dart';
import '../../theme/fusion_theme.dart';
import '../../ui/app_menu.dart';
import '../../ui/artwork.dart';
import '../../ui/glass_backdrop.dart';
import '../../ui/liquid_glass.dart';
import '../../ui/player_view.dart';
import '../../ui/vector_icon.dart';
import 'player_location.dart';
import 'settings.dart';
import '../ui_diagnostics.dart';

@JS('fpRecordBrowserError')
external void _recordError(JSString message);

@JS('fpGetEffects')
external bool _getEffects();
@JS('fpSetEffects')
external void _setEffects(bool enabled);

@JS('fpControl')
external JSPromise<JSAny?> _control(
  JSString url,
  JSString source,
  JSString command,
  JSNumber value,
);
@JS('fpNavigate')
external void _navigate(JSString url);
@JS('fpPin')
external void _pin(JSString url);
@JS('fpSpeakerName')
external void _setTabName(JSString name);

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  installUiDiagnostics((message) => _recordError(message.toJS));
  runApp(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: fusionTheme(),
      home: const _Receiver(),
    ),
  );
}

class _Receiver extends StatefulWidget {
  const _Receiver();
  @override
  State<_Receiver> createState() => _ReceiverState();
}

class _ReceiverState extends State<_Receiver> {
  var location = SpeakerPlayerLocation(Uri.base);
  Map<String, String> speakers = const {};
  bool loadingSpeakers = false;
  bool get missing => speakers.isNotEmpty && !speakers.containsKey(location.id);

  AppState state = const AppState(controls: PlaybackControls.none);
  Timer? timer;
  bool offline = false;
  bool volume = false;
  bool advancedEffects = _getEffects();

  @override
  void initState() {
    super.initState();
    if (location.pinned) refreshSpeakers();
    refresh();
    if (location.page.fragment == '/settings') {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        final page = location.page.removeFragment();
        location = SpeakerPlayerLocation(page);
        _pin(page.toString().toJS);
        menu(showSettings: true);
      });
    }
  }

  Future<void> refreshSpeakers() async {
    if (loadingSpeakers) return;
    loadingSpeakers = true;
    try {
      final catalog = await fnosRequest(location.catalog);
      final next = {
        for (final item in catalog['instances'] as List)
          item['id'] as String: item['name'] as String,
      };
      if (!mounted) return;
      final resolved = location.resolveDefault(
        catalog['default'] as String?,
        next.keys,
      );
      if (!location.pinned && resolved.pinned) {
        _pin(resolved.page.toString().toJS);
      }
      setState(() {
        location = resolved;
        speakers = next;
      });
      _setTabName((speakers[location.id] ?? '音箱不可用').toJS);
    } catch (_) {
      // Opening the menu retries discovery. The pinned receiver can still play.
    } finally {
      loadingSpeakers = false;
    }
  }

  Future<void> refresh() async {
    try {
      if (!location.pinned) await refreshSpeakers();
      if (!mounted) return;
      final json = await fnosRequest(location.api('status'));
      if (!mounted) return;
      final source = json['current_source'] as String?;
      final media = source == null ? const {} : json['media'] as Map;
      final protocol = json['protocols'][source];
      final art = media['cover_url'] as String?;
      final caps = (json['player_commands'] as List?) ?? const [];
      final uri = location.artwork(art);
      final next = AppState.fromMap({
        'initialized': true,
        'receiverReady': true,
        'activeMediaSource': source,
        'receiverName': json['receiver_name'],
        'controls': {
          'playPause': caps.contains('play_pause'),
          'previous': caps.contains('previous_track'),
          'next': caps.contains('next_track'),
          'seek': caps.contains('seek'),
        },
        'playback': {
          'title': media['title'],
          'artist': media['artist'],
          'album': media['album'],
          'durationMs': media['duration_ms'],
          'positionMs': media['position_ms'],
          'lyrics': media['lyrics_text'],
          'qualityText': media['detail_codec'],
          'streamActive': source != null,
          'protocol': source,
          'isPlaying': ['streaming', 'playing'].contains(protocol?['state']),
          'coverArt': uri?.toString(),
          'volumePercent': json['sender_volume']['${source}_percent'],
        },
      });
      if (state != next || offline || volume != caps.contains('set_volume')) {
        setState(() {
          state = next;
          offline = false;
          volume = caps.contains('set_volume');
        });
      }
    } catch (_) {
      if (mounted && !offline) {
        setState(() {
          offline = true;
          volume = false;
          state = const AppState(controls: PlaybackControls.none);
        });
        refreshSpeakers();
      }
    } finally {
      if (mounted) timer = Timer(const Duration(seconds: 1), refresh);
    }
  }

  Future<void> send(String command, [int value = 0]) async {
    final source = state.activeMediaSource;
    if (source == null || offline) return;
    try {
      await _control(
        location.api('player').toString().toJS,
        source.toJS,
        command.toJS,
        value.toJS,
      ).toDart;
    } catch (_) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(
          SnackBar(
            behavior: SnackBarBehavior.floating,
            backgroundColor: Colors.transparent,
            elevation: 0,
            padding: EdgeInsets.zero,
            content: GlassAppearance(
              enabled: advancedEffects,
              child: const LiquidGlass(
                level: GlassLevel.popup,
                blurSigma: 9,
                padding: EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                child: Text(
                  '操作未完成，请检查发送端连接',
                  style: TextStyle(color: Colors.white),
                ),
              ),
            ),
          ),
        );
      }
    }
  }

  final menuKey = GlobalKey();
  final menuFocus = FocusNode();
  final settingsFocus = FocusNode();
  final speakerKey = GlobalKey();
  final speakerFocus = FocusNode();
  bool speakerMenu = false;
  final menuNodes = List.generate(2, (_) => FocusNode());
  MorphStage? stage;
  Rect? anchor;
  bool focusesSettings = false;
  Uri? destination;

  void menu({bool forSpeakers = false, bool showSettings = false}) {
    final trigger = forSpeakers ? speakerKey : menuKey;
    final box = trigger.currentContext?.findRenderObject();
    if (box is! RenderBox || stage != null) return;
    refreshSpeakers();
    setState(() {
      speakerMenu = forSpeakers;
      anchor = box.localToGlobal(Offset.zero) & box.size;
      focusesSettings = showSettings;
      stage = showSettings ? MorphStage.settings : MorphStage.menu;
    });
    if (!showSettings) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) menuNodes.first.requestFocus();
      });
    }
  }

  Uri get updatePage => location.root
      .resolve('update.html')
      .replace(queryParameters: location.page.queryParameters);

  void openSettings() {
    setState(() {
      focusesSettings = true;
      stage = MorphStage.settings;
    });
  }

  void closeSettings() => setState(() => stage = MorphStage.menu);

  void closeMenu([Uri? route]) {
    if (stage == MorphStage.closing) return;
    destination = route;
    setState(() => stage = MorphStage.closing);
  }

  void menuClosed() {
    setState(() {
      stage = null;
      focusesSettings = false;
    });
    if (destination case final Uri route) {
      _navigate(route.toString().toJS);
    } else {
      final trigger = speakerMenu ? speakerFocus : menuFocus;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && stage == null) trigger.requestFocus();
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final art = state.playback.coverArt;
    return Scaffold(
      body: GlassAppearance(
        enabled: advancedEffects,
        child: GlassBackdropScope(
          enabled: advancedEffects,
          background: PlayerBackdrop(
            source: art,
            hasPlayback: state.playback.streamActive,
          ),
          revision: art,
          child: MediaQuery(
            data: MediaQuery.of(context).copyWith(
              disableAnimations:
                  !advancedEffects || MediaQuery.disableAnimationsOf(context),
            ),
            child: Stack(
              fit: StackFit.expand,
              children: [
                ExcludeFocus(
                  excluding: stage != null,
                  child: PlayerView(
                    state: state,
                    artworkSource: art,
                    artworkIdentity: art,
                    onOpenMenu: menu,
                    menuButtonKey: menuKey,
                    menuFocusNode: menuFocus,
                    menuOpen: !speakerMenu && stage == MorphStage.menu,
                    overlayOpen: stage != null,
                    volumeEnabled: volume && !offline,
                    onPlayback: (action) => send(
                      const {
                        PlaybackCommand.previous: 'previous_track',
                        PlaybackCommand.playPause: 'play_pause',
                        PlaybackCommand.next: 'next_track',
                      }[action]!,
                    ),
                    onSeek: (ms) => send('seek', ms),
                    onVolume: (percent) => send('set_volume', percent),
                  ),
                ),
                Positioned(
                  right: 8,
                  bottom: 8,
                  child: SafeArea(
                    child: ExcludeFocus(
                      excluding: stage != null,
                      child: IgnorePointer(
                        ignoring: stage != null,
                        child: Opacity(
                          opacity: speakerMenu && stage != null ? 0 : 1,
                          child: SizedBox.square(
                            key: speakerKey,
                            dimension: 44,
                            child: LiquidGlass(
                              borderRadius: 22,
                              interactive: true,
                              child: Tooltip(
                                padding: EdgeInsets.zero,
                                decoration: const BoxDecoration(),
                                richMessage: WidgetSpan(
                                  child: GlassAppearance(
                                    enabled: advancedEffects,
                                    child: const LiquidGlass(
                                      level: GlassLevel.popup,
                                      borderRadius: 10,
                                      blurSigma: 6,
                                      padding: EdgeInsets.symmetric(
                                        horizontal: 10,
                                        vertical: 6,
                                      ),
                                      child: Text(
                                        '飞牛音箱',
                                        style: TextStyle(color: Colors.white),
                                      ),
                                    ),
                                  ),
                                ),
                                child: IconButton(
                                  focusNode: speakerFocus,
                                  onPressed: () => menu(forSpeakers: true),
                                  icon: Image.network(
                                    Uri.base.resolve('fnos-mark.png').toString(),
                                    width: 23,
                                    height: 23,
                                    excludeFromSemantics: true,
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                if (stage != null)
                  CallbackShortcuts(
                    bindings: {
                      const SingleActivator(LogicalKeyboardKey.escape): () {
                        if (stage == MorphStage.settings) {
                          closeSettings();
                        } else {
                          closeMenu();
                        }
                      },
                    },
                    child: FusionMorphOverlay(
                      stage: stage!,
                      anchorRect: anchor!,
                      menuFocusNodes: menuNodes,
                      menuItems: [
                        if (speakerMenu) ...[
                          FusionMenuItem(
                            icon: const Icon(Icons.speaker_group_rounded),
                            label: '切换音箱',
                            onSelected: () => closeMenu(location.speakerPicker),
                          ),
                          FusionMenuItem(
                            icon: const Icon(Icons.add_rounded),
                            label: '新建实例',
                            onSelected: () =>
                                closeMenu(location.createInstance),
                          ),
                        ] else ...[
                          FusionMenuItem(
                            icon: const FusionVectorIcon(
                              FusionIcons.settings,
                              size: 19,
                              color: Colors.white,
                            ),
                            label: '设置',
                            onSelected: openSettings,
                          ),
                          FusionMenuItem(
                            icon: const Icon(Icons.system_update_rounded),
                            label: '检查更新',
                            onSelected: () => closeMenu(updatePage),
                          ),
                        ],
                      ],
                      settingsTitle: '设置',
                      settingsChild: focusesSettings
                          ? FnosSettings(
                              location: location,
                              initialFocus: settingsFocus,
                              advancedEffects: advancedEffects,
                              onAdvancedEffects: (enabled) {
                                _setEffects(enabled);
                                setState(() => advancedEffects = enabled);
                              },
                              onClose: closeSettings,
                              onOpenUpdate: () => closeMenu(updatePage),
                            )
                          : const SizedBox.shrink(),
                      onDismiss: closeMenu,
                      onSettingsClose: closeSettings,
                      onTransitionCompleted: (s) {
                        if (s == MorphStage.menu) {
                          menuNodes.first.requestFocus();
                        }
                        if (s == MorphStage.settings &&
                            settingsFocus.context != null) {
                          settingsFocus.requestFocus();
                        }
                      },
                      onClosed: menuClosed,
                    ),
                  ),
                if (offline)
                  Align(
                    alignment: Alignment.bottomCenter,
                    child: Text(
                      missing ? '此音箱已移除，请在右下角切换音箱' : '接收器连接中断，正在重连',
                      style: const TextStyle(color: Colors.white),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  @override
  void dispose() {
    timer?.cancel();
    menuFocus.dispose();
    settingsFocus.dispose();
    speakerFocus.dispose();
    for (final node in menuNodes) {
      node.dispose();
    }
    super.dispose();
  }
}
