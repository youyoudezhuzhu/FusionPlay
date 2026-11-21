import 'dart:async';
import 'dart:convert';
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
import '../ui_diagnostics.dart';

@JS('fpRecordBrowserError')
external void _recordError(JSString message);

@JS('fpRead')
external JSPromise<JSString> _read(JSString url);
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

Future<Map<String, dynamic>> _readMap(Uri url) async {
  final result = await _read(url.toString().toJS).toDart;
  return jsonDecode(result.toDart) as Map<String, dynamic>;
}

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

  final backdrop = ValueNotifier<GlassBackdropTexture?>(null);
  AppState state = const AppState(controls: PlaybackControls.none);
  Timer? timer;
  bool offline = false;
  bool volume = false;

  @override
  void initState() {
    super.initState();
    if (location.pinned) refreshSpeakers();
    refresh();
  }

  Future<void> refreshSpeakers() async {
    if (loadingSpeakers) return;
    loadingSpeakers = true;
    try {
      final catalog = await _readMap(location.catalog);
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
      final json = await _readMap(location.api('status'));
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
        ).showSnackBar(const SnackBar(content: Text('操作未完成，请检查发送端连接')));
      }
    }
  }

  final menuKey = GlobalKey();
  final menuFocus = FocusNode();
  final speakerKey = GlobalKey();
  final speakerFocus = FocusNode();
  bool speakerMenu = false;
  final menuNodes = List.generate(2, (_) => FocusNode());
  MorphStage? stage;
  Rect? anchor;
  Uri? destination;

  void menu({bool forSpeakers = false}) {
    final trigger = forSpeakers ? speakerKey : menuKey;
    final box = trigger.currentContext?.findRenderObject();
    if (box is! RenderBox || stage != null) return;
    refreshSpeakers();
    setState(() {
      speakerMenu = forSpeakers;
      anchor = box.localToGlobal(Offset.zero) & box.size;
      stage = MorphStage.menu;
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) menuNodes.first.requestFocus();
    });
  }

  void closeMenu([Uri? route]) {
    if (stage == MorphStage.closing) return;
    destination = route;
    setState(() => stage = MorphStage.closing);
  }

  void menuClosed() {
    setState(() => stage = null);
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
      body: GlassBackdropProvider(
        notifier: backdrop,
        child: GlassBackdropScope(
          background: PlayerBackdrop(
            source: art,
            hasPlayback: state.playback.streamActive,
          ),
          revision: art,
          onTextureChanged: (image) => backdrop.value = image,
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
                            child: IconButton(
                              tooltip: '飞牛音箱',
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
              if (stage != null)
                CallbackShortcuts(
                  bindings: {
                    const SingleActivator(LogicalKeyboardKey.escape): () {
                      if (stage == MorphStage.settings) {
                        setState(() => stage = MorphStage.menu);
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
                          onSelected: () => closeMenu(location.createInstance),
                        ),
                      ] else ...[
                        FusionMenuItem(
                          icon: const FusionVectorIcon(
                            FusionIcons.settings,
                            size: 19,
                            color: Colors.white,
                          ),
                          label: '设置',
                          onSelected: () => closeMenu(location.settings),
                        ),
                        FusionMenuItem(
                          icon: const Icon(Icons.system_update_rounded),
                          label: '检查更新',
                          onSelected: () => closeMenu(
                            location.root
                                .resolve('update.html')
                                .replace(
                                  queryParameters:
                                      location.page.queryParameters,
                                ),
                          ),
                        ),
                      ],
                    ],
                    settingsTitle: '设置',
                    settingsChild: const SizedBox.shrink(),
                    onDismiss: closeMenu,
                    onSettingsClose: closeMenu,
                    onTransitionCompleted: (s) {
                      if (s == MorphStage.menu) menuNodes.first.requestFocus();
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
    );
  }

  @override
  void dispose() {
    timer?.cancel();
    menuFocus.dispose();
    speakerFocus.dispose();
    for (final node in menuNodes) {
      node.dispose();
    }
    backdrop.dispose();
    super.dispose();
  }
}
