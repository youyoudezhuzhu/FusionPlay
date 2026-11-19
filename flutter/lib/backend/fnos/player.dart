import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:flutter/material.dart';
import '../../models/app_state.dart';
import '../../theme/fusion_theme.dart';
import '../../ui/artwork.dart';
import '../../ui/app_menu.dart';
import 'lyrics.dart';
import '../../ui/glass_backdrop.dart';
import '../../ui/player_view.dart';

@JS('fpStatus')
external JSPromise<JSString> _readStatus();
@JS('fpControl')
external JSPromise<JSAny?> _control(
  JSString source,
  JSString command,
  JSNumber value,
);
@JS('fpManage')
external void _manage(JSString route);

void main() => runApp(
  MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: fusionTheme(),
    home: const _Receiver(),
  ),
);

class _Receiver extends StatefulWidget {
  const _Receiver();
  @override
  State<_Receiver> createState() => _ReceiverState();
}

class _ReceiverState extends State<_Receiver> {
  final backdrop = ValueNotifier<GlassBackdropTexture?>(null);
  AppState state = const AppState(controls: PlaybackControls.none);
  Timer? timer;
  bool offline = false;
  bool volume = false;

  @override
  void initState() {
    super.initState();
    refresh();
  }

  Future<void> refresh() async {
    try {
      final json = jsonDecode((await _readStatus().toDart).toDart) as Map;
      if (!mounted) return;
      final source = json['current_source'] as String?;
      final media = source == null ? const {} : json['media'] as Map;
      final protocol = json['protocols'][source];
      final art = media['cover_url'] as String?;
      final caps = (json['player_commands'] as List?) ?? const [];
      final uri = art == null || art.isEmpty
          ? null
          : Uri.base.resolve('../').resolve(art);
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
          'lyrics': receiverLyrics(media['lyrics_text'] as String?),
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
      if (mounted && !offline) setState(() => offline = true);
    } finally {
      if (mounted) timer = Timer(const Duration(seconds: 1), refresh);
    }
  }

  Future<void> send(String command, [int value = 0]) async {
    final source = state.activeMediaSource;
    if (source == null || offline) return;
    try {
      await _control(source.toJS, command.toJS, value.toJS).toDart;
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
  final menuNodes = List.generate(4, (_) => FocusNode());
  MorphStage? stage;
  Rect? anchor;
  String? destination;

  void menu() {
    final box = menuKey.currentContext?.findRenderObject();
    if (box is! RenderBox || stage != null) return;
    setState(() {
      anchor = box.localToGlobal(Offset.zero) & box.size;
      stage = MorphStage.menu;
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) menuNodes.first.requestFocus();
    });
  }

  void closeMenu([String? route]) {
    destination = route;
    setState(() => stage = MorphStage.closing);
  }

  void menuClosed() {
    setState(() => stage = null);
    if (destination case final String route) {
      _manage(route.toJS);
    } else {
      menuFocus.requestFocus();
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
              PlayerView(
                state: state,
                artworkSource: art,
                artworkIdentity: art,
                onOpenMenu: menu,
                menuButtonKey: menuKey,
                menuFocusNode: menuFocus,
                menuOpen: stage == MorphStage.menu,
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
              if (stage != null)
                FusionMorphOverlay(
                  stage: stage!,
                  anchorRect: anchor!,
                  menuFocusNodes: menuNodes,
                  menuItems: [
                    for (final item in const [
                      ('audio', '音箱与输出方案', Icons.speaker_group),
                      ('receive', '接收设置', Icons.settings),
                      ('overview', '服务概览', Icons.monitor_heart),
                      ('settings', '维护与诊断', Icons.info_outline),
                    ])
                      FusionMenuItem(
                        icon: Icon(item.$3),
                        label: item.$2,
                        onSelected: () => closeMenu(item.$1),
                      ),
                  ],
                  settingsTitle: '',
                  settingsChild: const SizedBox.shrink(),
                  onDismiss: closeMenu,
                  onSettingsClose: closeMenu,
                  onClosed: menuClosed,
                ),
              if (offline)
                const Align(
                  alignment: Alignment.bottomCenter,
                  child: Text(
                    '接收器连接中断，正在重连',
                    style: TextStyle(color: Colors.white),
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
    for (final node in menuNodes) {
      node.dispose();
    }
    backdrop.dispose();
    super.dispose();
  }
}
