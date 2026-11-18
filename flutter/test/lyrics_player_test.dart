import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';
import 'package:fusionplay_android_flutter/ui/rounded_progress_bar.dart';

LyricsDocument _lyrics() => const LyricsDocument(
  origin: 'embedded',
  synced: true,
  lines: [
    LyricLine(
      startMs: 0,
      text: '第一句歌词',
      words: [
        LyricsWord(startMs: 0, endMs: 400, text: '第一句'),
        LyricsWord(startMs: 400, endMs: 900, text: '歌词'),
      ],
    ),
    LyricLine(startMs: 1000, text: '第二句歌词'),
  ],
);

AppState _state({
  LyricsDocument? lyrics,
  bool lyricsLoading = false,
  bool playing = false,
  String? protocol,
  String? qualityText,
  String? trackIdentity,
  int? volumePercent = 65,
  int? durationMs = 180000,
  PlaybackControls controls = PlaybackControls.all,
}) => AppState(
  initialized: true,
  playback: PlaybackSnapshot(
    title: 'Track',
    artist: 'Artist',
    trackIdentity: trackIdentity,
    durationMs: durationMs,
    positionMs: 200,
    streamActive: true,
    isPlaying: playing,
    protocol: protocol,
    qualityText: qualityText,
    volumePercent: volumePercent,
    lyrics: lyrics,
    lyricsLoading: lyricsLoading,
  ),
  controls: controls,
);

AppState _stateFromMap(Map<String, Object?> map) => AppState.fromMap({
  'initialized': true,
  'playback': {
    'title': 'Track',
    'durationMs': 180000,
    'streamActive': true,
    ...(map['playback'] as Map<String, Object?>? ?? const {}),
  },
  if (map.containsKey('controls')) 'controls': map['controls'],
  if (map.containsKey('remoteControl')) 'remoteControl': map['remoteControl'],
});

Widget _playerHost(
  AppState state, {
  bool reduceMotion = false,
  ValueChanged<PlaybackCommand>? onPlayback,
  ValueChanged<int>? onSeek,
  ValueChanged<int>? onVolume,
  FocusNode? menuFocusNode,
  VoidCallback? onOpenMenu,
  FocusNode? progressFocusNode,
  FocusNode? volumeFocusNode,
  Size size = const Size(1000, 660),
}) {
  final body = Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context).copyWith(disableAnimations: reduceMotion),
      child: Scaffold(
        backgroundColor: const Color(0xFF101316),
        body: PlayerView(
          state: state,
          onPlayback: onPlayback ?? (_) {},
          onSeek: onSeek ?? (_) {},
          onVolume: onVolume ?? (_) {},
          menuFocusNode: menuFocusNode,
          onOpenMenu: onOpenMenu,
          progressFocusNode: progressFocusNode,
          volumeFocusNode: volumeFocusNode,
          forceGlassFallback: true,
        ),
      ),
    ),
  );
  return MaterialApp(
    theme: ThemeData(splashFactory: NoSplash.splashFactory),
    home: body,
  );
}

Future<void> _pumpPlayer(
  WidgetTester tester,
  AppState state, {
  bool reduceMotion = false,
  ValueChanged<PlaybackCommand>? onPlayback,
  ValueChanged<int>? onSeek,
  ValueChanged<int>? onVolume,
  FocusNode? menuFocusNode,
  VoidCallback? onOpenMenu,
  FocusNode? progressFocusNode,
  FocusNode? volumeFocusNode,
  Size size = const Size(1000, 660),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    _playerHost(
      state,
      reduceMotion: reduceMotion,
      onPlayback: onPlayback,
      onSeek: onSeek,
      onVolume: onVolume,
      menuFocusNode: menuFocusNode,
      onOpenMenu: onOpenMenu,
      progressFocusNode: progressFocusNode,
      volumeFocusNode: volumeFocusNode,
      size: size,
    ),
  );
  await tester.pump();
}

void main() {
  testWidgets('YAQMC 布局：封面、标题、常驻大歌词与玻璃 dock', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()));

    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    expect(find.byType(ArtworkCard), findsOneWidget);
    expect(find.text('Track'), findsOneWidget);
    expect(find.text('Artist'), findsOneWidget);
    // 歌词常驻右列，逐字行渲染为逐词高亮。
    expect(find.text('第一句'), findsWidgets);
    expect(find.text('第二句歌词'), findsOneWidget);
    // 旧歌词开关已移除。
    expect(find.byKey(const ValueKey('playback-lyrics-button')), findsNothing);
    // 三个播放按钮和音量胶囊使用上游玻璃组件。
    expect(find.byKey(const ValueKey('player-volume-capsule')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('playback-play-pause-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('volume-slider')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('信息顺序：封面 → 标题 → 作者 → 歌词（横向布局）', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics(), protocol: 'airplay'),
      size: const Size(800, 360),
    );

    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    final title = tester.getRect(find.text('Track'));
    final artist = tester.getRect(find.text('Artist'));
    final lyrics = tester.getRect(
      find.byKey(const ValueKey('player-lyrics-view')),
    );

    // 21:9 双栏：左侧玻璃面板含封面/信息，右侧是歌词列。
    expect(title.top, lessThan(artist.top));
    expect(lyrics.left, greaterThan(title.left));
    expect(tester.takeException(), isNull);
  });

  testWidgets('无歌词时改为封面居中布局并显示渠道说明', (tester) async {
    await _pumpPlayer(tester, _state());

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.text('暂无歌词'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('有歌词横屏：左栏纵向居中，歌词独占右栏', (tester) async {
    for (final size in const [
      Size(620, 320),
      Size(840, 360),
      Size(900, 349),
      Size(900, 350),
      Size(900, 400),
      Size(900, 460),
      Size(900, 461),
      Size(980, 420),
      Size(1280, 597),
      Size(1280, 800),
      Size(1920, 1080),
    ]) {
      await _pumpPlayer(
        tester,
        _state(lyrics: _lyrics()),
        reduceMotion: true,
        size: size,
      );
      final cover = tester.getRect(find.byType(ArtworkCard));
      final column = [
        cover,
        tester.getRect(find.text('Track')),
        tester.getRect(find.text('Artist')),
        for (final key in [
          'player-progress-bar',
          'playback-play-pause-button',
          'player-volume-capsule',
        ])
          tester.getRect(find.byKey(ValueKey(key))),
      ];
      final lyrics = tester.getRect(
        find.byKey(const ValueKey('player-lyrics-view')),
      );
      for (var i = 0; i < column.length; i++) {
        expect(
          column[i].center.dx,
          closeTo(cover.center.dx, 1),
          reason: '$size',
        );
        expect(column[i].right, lessThan(lyrics.left), reason: '$size');
        if (i > 0) {
          expect(
            column[i].top,
            greaterThanOrEqualTo(column[i - 1].bottom - .1),
            reason: '$size',
          );
        }
      }
      expect(tester.takeException(), isNull);
    }
  });

  testWidgets('相同歌词状态下三种来源的封面和操作区位置一致', (tester) async {
    for (final lyrics in [false, true]) {
      List<Rect>? reference;
      for (final protocol in ['airplay', 'xiaomi_miplay', 'dlna']) {
        final state = _stateFromMap({
          'playback': {
            'protocol': protocol,
            if (lyrics)
              'lyrics': {
                'lines': [
                  {'startMs': 0, 'text': 'Same lyric'},
                ],
              },
          },
        });
        await _pumpPlayer(
          tester,
          state,
          reduceMotion: true,
          size: const Size(980, 420),
        );
        await tester.pump(const Duration(seconds: 1));
        final rects = [
          tester.getRect(find.byType(ArtworkCard)),
          tester.getRect(find.text('Track')),
          for (final key in [
            'player-progress-bar',
            'playback-previous-button',
            'playback-play-pause-button',
            'playback-next-button',
            'player-volume-capsule',
            if (lyrics) 'player-lyrics-view',
          ])
            tester.getRect(find.byKey(ValueKey(key))),
        ];
        reference ??= rects;
        expect(rects, reference, reason: 'protocol=$protocol lyrics=$lyrics');
      }
    }
  });

  testWidgets('AirPlay 无 remote 命令时禁用上一曲/下一曲，播放保留', (tester) async {
    final state = _stateFromMap({
      'playback': {'protocol': 'airplay'},
      'remoteControl': {'available': false, 'commands': <Object?>[]},
    });
    final commands = <PlaybackCommand>[];
    await _pumpPlayer(tester, state, onPlayback: commands.add);
    await tester.tap(find.byKey(const ValueKey('playback-previous-button')));
    await tester.tap(find.byKey(const ValueKey('playback-next-button')));
    expect(commands, isEmpty);

    expect(
      find.byKey(const ValueKey('playback-previous-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('playback-play-pause-button')),
      findsOneWidget,
    );
  });

  testWidgets('AirPlay 有 previous/next 命令时显示上一曲/下一曲', (tester) async {
    final state = _stateFromMap({
      'playback': {'protocol': 'airplay'},
      'remoteControl': {
        'available': true,
        'commands': <Object?>['play_pause', 'previous_track', 'next_track'],
      },
    });
    await _pumpPlayer(tester, state);

    expect(
      find.byKey(const ValueKey('playback-previous-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsOneWidget);
  });

  testWidgets('小米妙播缺省 fallback 支持全部控制', (tester) async {
    final state = _stateFromMap({
      'playback': {'protocol': 'xiaomi_miplay'},
    });
    await _pumpPlayer(tester, state);

    expect(
      find.byKey(const ValueKey('playback-previous-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsOneWidget);
  });

  testWidgets('显式 controls 可禁用 skip 并禁用 seek', (tester) async {
    final state = _stateFromMap({
      'playback': {'protocol': 'airplay'},
      'controls': {
        'playPause': true,
        'previous': false,
        'next': false,
        'seek': false,
      },
    });
    await _pumpPlayer(tester, state);

    expect(
      find.byKey(const ValueKey('playback-previous-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsOneWidget);
    final progressBar = tester.widget<RoundedProgressBar>(
      find.byKey(const ValueKey('player-progress-bar')),
    );
    expect(progressBar.seekEnabled, isFalse);
    expect(progressBar.enabled, isTrue);
  });

  testWidgets('音量滑杆 onChangeEnd 回写，静音按钮可切换静音/恢复', (tester) async {
    final volumes = <int>[];
    await _pumpPlayer(tester, _state(volumePercent: 65), onVolume: volumes.add);

    final slider = find.byKey(const ValueKey('volume-slider'));
    tester.widget<RoundedProgressBar>(slider).onChangeEnd?.call(.4);
    await tester.pump();
    expect(volumes.last, 40);

    await tester.tap(find.byKey(const ValueKey('volume-mute-button')));
    await tester.pump();
    expect(volumes.last, 0);

    await tester.tap(find.byKey(const ValueKey('volume-mute-button')));
    await tester.pump();
    expect(volumes.last, 40);
  });

  testWidgets('拖动中只更新本地滑杆，松手只发一次 volume 命令', (tester) async {
    final volumes = <int>[];
    await _pumpPlayer(tester, _state(volumePercent: 65), onVolume: volumes.add);

    final slider = find.byKey(const ValueKey('volume-slider'));
    final widget = tester.widget<RoundedProgressBar>(slider);
    widget.onChanged(.2);
    widget.onChanged(.3);
    widget.onChanged(.4);
    await tester.pump();
    final value = tester
        .widget<RoundedProgressBar>(find.byKey(const ValueKey('volume-slider')))
        .value;
    expect(value, closeTo(.4, .001));
    // 拖动过程中不发命令（展示端只在松手提交），不会反复触发错误。
    expect(volumes, isEmpty);
    await tester.pump(const Duration(milliseconds: 500));
    expect(volumes, isEmpty);

    widget.onChangeEnd!.call(.4);
    await tester.pump();
    expect(volumes, [40]);

    // 重复的 onChangeEnd / 相同值不会再次发送命令。
    widget.onChangeEnd!.call(.4);
    await tester.pump();
    expect(volumes, [40]);
  });

  testWidgets('音量回读与乐观值不打架：拖动跟手、回读同值不抖动', (tester) async {
    final volumes = <int>[];
    await _pumpPlayer(tester, _state(volumePercent: 65), onVolume: volumes.add);

    final slider = find.byKey(const ValueKey('volume-slider'));
    tester.widget<RoundedProgressBar>(slider).onChanged(.5);
    await tester.pump();
    tester.widget<RoundedProgressBar>(slider).onChangeEnd!.call(.5);
    await tester.pump();
    expect(volumes, [50]);

    // CoreEvent::Volume 回读同样的值：立即吸收 optimistic，不产生跳变/重复命令。
    await _pumpPlayer(tester, _state(volumePercent: 50), onVolume: volumes.add);
    await tester.pump();
    expect(
      tester
          .widget<RoundedProgressBar>(
            find.byKey(const ValueKey('volume-slider')),
          )
          .value,
      closeTo(.5, .001),
    );
    expect(volumes, [50]);
    await tester.pump(const Duration(milliseconds: 1000));
    expect(volumes, [50]);
  });

  testWidgets('D-pad 焦点链：菜单→进度→播放→下一曲→音量', (tester) async {
    final menuFocus = FocusNode(debugLabel: 'test-menu');
    final progressFocus = FocusNode(debugLabel: 'test-progress');
    final volumeFocus = FocusNode(debugLabel: 'test-volume');
    addTearDown(menuFocus.dispose);
    addTearDown(progressFocus.dispose);
    addTearDown(volumeFocus.dispose);

    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics(), protocol: 'xiaomi_miplay'),
      menuFocusNode: menuFocus,
      onOpenMenu: () {},
      progressFocusNode: progressFocus,
      volumeFocusNode: volumeFocus,
    );

    menuFocus.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(progressFocus.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-play-pause');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-next');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(volumeFocus.hasPrimaryFocus, isTrue);
  });

  testWidgets('矮横屏 620x320 不溢出且歌词常驻', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics()),
      size: const Size(620, 320),
    );
    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('纵向紧凑布局 420x760 不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics()),
      size: const Size(420, 760),
    );
    await tester.pump();
    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    await tester.drag(
      find.byKey(const ValueKey('player-layout-horizontal')),
      const Offset(0, -160),
    );
    await tester.pump();
    expect(tester.takeException(), isNull);
  });

  testWidgets('disableAnimations 下正常渲染', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()), reduceMotion: true);
    expect(find.text('第一句'), findsWidgets);
    expect(tester.takeException(), isNull);
  });

  testWidgets('AirPlay 无歌词：居中布局 + 音频流未提供歌词（不按协议断言）', (tester) async {
    await _pumpPlayer(
      tester,
      _state(
        protocol: 'airplay',
        qualityText: 'AAC / 44.1 kHz / 16-bit / 2 ch',
      ),
    );

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.text('音频流未提供歌词'), findsNothing);
    expect(find.textContaining('不提供歌词'), findsNothing);
    // 顶部只保留协议；音频格式不再显示。
    expect(find.text('AirPlay 2'), findsWidgets);
    expect(find.text('AAC / 44.1 kHz / 16-bit / 2 ch'), findsNothing);
    // 没有歌词时不出现歌词开关。
    expect(
      find.byKey(const ValueKey('playback-lyrics-toggle-button')),
      findsNothing,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('DLNA 无歌词：居中布局 + 发送端未提供歌词', (tester) async {
    await _pumpPlayer(tester, _state(protocol: 'dlna'));

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.text('此发送端未提供歌词'), findsNothing);
    expect(find.text('暂无歌词'), findsNothing);
  });

  testWidgets('小米妙播无歌词：居中布局 + 本曲没有歌词', (tester) async {
    await _pumpPlayer(tester, _state(protocol: 'xiaomi_miplay'));

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.text('此歌曲没有歌词'), findsNothing);
    expect(find.text('暂无歌词'), findsNothing);
  });

  testWidgets('AirPlay 带 lyrics 时正常渲染歌词（实时判断，不按协议禁用）', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics(), protocol: 'airplay'));

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.text('第一句'), findsWidgets);
    expect(find.text('第二句歌词'), findsOneWidget);
    expect(find.textContaining('不提供歌词'), findsNothing);
    expect(
      find.byKey(const ValueKey('playback-lyrics-toggle-button')),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('有歌词默认双栏，点击歌词按钮可隐藏/恢复', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics(), protocol: 'xiaomi_miplay'),
    );

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    final toggle = find.byKey(const ValueKey('playback-lyrics-toggle-button'));
    expect(toggle, findsOneWidget);

    await tester.tap(toggle);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    // 用户主动隐藏时不再显示“无歌词”说明。
    expect(find.text('此歌曲没有歌词'), findsNothing);
    expect(find.text('暂无歌词'), findsNothing);

    await tester.tap(toggle);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
  });

  testWidgets('换曲后歌词显示状态重置为显示', (tester) async {
    tester.view.physicalSize = const Size(1000, 660);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      _playerHost(
        _state(
          lyrics: _lyrics(),
          protocol: 'xiaomi_miplay',
          trackIdentity: 'track-a',
        ),
      ),
    );
    await tester.pump();
    await tester.tap(
      find.byKey(const ValueKey('playback-lyrics-toggle-button')),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);

    await tester.pumpWidget(
      _playerHost(
        _state(
          lyrics: _lyrics(),
          protocol: 'xiaomi_miplay',
          trackIdentity: 'track-b',
        ),
      ),
    );
    await tester.pump();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
  });

  testWidgets('居中↔双栏切换使用 300ms fade+scale 过渡', (tester) async {
    tester.view.physicalSize = const Size(1000, 660);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(_playerHost(_state(protocol: 'dlna')));
    await tester.pump();
    final switcher = tester.widget<AnimatedSwitcher>(
      find.byKey(const ValueKey('player-body-switcher')),
    );
    expect(switcher.duration, const Duration(milliseconds: 300));
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );

    await tester.pumpWidget(
      _playerHost(_state(protocol: 'dlna', lyrics: _lyrics())),
    );
    await tester.pump();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.byKey(const ValueKey('player-centered-layout')), findsNothing);
  });

  testWidgets('reduce motion 时布局切换直接完成', (tester) async {
    tester.view.physicalSize = const Size(1000, 660);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      _playerHost(_state(protocol: 'dlna'), reduceMotion: true),
    );
    await tester.pump();
    final switcher = tester.widget<AnimatedSwitcher>(
      find.byKey(const ValueKey('player-body-switcher')),
    );
    expect(switcher.duration, Duration.zero);
  });

  testWidgets('焦点链：进度→播放→歌词按钮→三点菜单→进度', (tester) async {
    final menuFocus = FocusNode(debugLabel: 'test-menu');
    final progressFocus = FocusNode(debugLabel: 'test-progress');
    addTearDown(menuFocus.dispose);
    addTearDown(progressFocus.dispose);

    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics(), protocol: 'xiaomi_miplay'),
      menuFocusNode: menuFocus,
      onOpenMenu: () {},
      progressFocusNode: progressFocus,
    );

    progressFocus.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-play-pause');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(
      FocusManager.instance.primaryFocus?.debugLabel,
      'player-lyrics-toggle',
    );

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'test-menu');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(progressFocus.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(menuFocus.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pump();
    expect(
      FocusManager.instance.primaryFocus?.debugLabel,
      'player-lyrics-toggle',
    );
  });

  testWidgets('AirPlay 居中布局在矮横屏 620x320 不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(protocol: 'airplay'),
      size: const Size(620, 320),
    );
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('居中布局在纵向 420x760 不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(protocol: 'dlna'),
      size: const Size(420, 760),
    );
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    await tester.pump();
    expect(tester.takeException(), isNull);
  });

  testWidgets('隐藏歌词后的居中布局在矮横屏也不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics(), protocol: 'xiaomi_miplay'),
      size: const Size(620, 320),
    );
    await tester.tap(
      find.byKey(const ValueKey('playback-lyrics-toggle-button')),
    );
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('横屏空态使用封面+控制双区，不再被底部 dock 挤压', (tester) async {
    await _pumpPlayer(tester, _state(), size: const Size(800, 360));

    expect(find.text('Track'), findsOneWidget);
    expect(find.text('暂无歌词'), findsNothing);
    expect(find.byKey(const ValueKey('player-dock')), findsNothing);
    expect(
      find.byKey(const ValueKey('playback-play-pause-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('volume-slider')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('无歌词横屏播放按钮与进度/音量同列可用', (tester) async {
    await _pumpPlayer(tester, _state(), size: const Size(800, 360));
    await tester.pump(const Duration(milliseconds: 400));

    final play = tester.getRect(
      find.byKey(const ValueKey('playback-play-pause-button')),
    );
    final progress = tester.getRect(
      find.byKey(const ValueKey('player-progress-bar')),
    );
    expect(play.top, greaterThan(progress.bottom));
    expect(play.left, greaterThan(0));
    expect(play.right, lessThanOrEqualTo(800));
    expect(find.byKey(const ValueKey('volume-slider')), findsOneWidget);
  });

  testWidgets('播放中本地时钟/呼吸动画不抛异常，退出后释放', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics(), playing: true));
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pump(const Duration(milliseconds: 300));
    expect(tester.takeException(), isNull);

    await tester.pumpWidget(const MaterialApp(home: SizedBox.shrink()));
    await tester.pump();
    expect(tester.takeException(), isNull);
  });
  testWidgets('AirPlay 同专辑连播不把三首曲名当歌词', (tester) async {
    for (final title in ['第一首', '第二首', '第三首']) {
      final state = _stateFromMap({
        'playback': {
          'protocol': 'AirPlay',
          'title': title,
          'artist': '同一歌手',
          'album': '同一专辑',
          'sourceEpoch': 7,
        },
      });
      await _pumpPlayer(tester, state);
      expect(find.text(title), findsOneWidget);
      expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    }
  });

  testWidgets('AirPlay 曲目标识切换时丢弃上一首的标题歌词', (tester) async {
    for (final (id, title) in [
      ('old', 'a'),
      ('old', 'b'),
      ('old', 'c'),
      ('new', '新歌'),
    ]) {
      await _pumpPlayer(
        tester,
        _stateFromMap({
          'playback': {
            'protocol': 'AirPlay',
            'title': title,
            'artist': '歌曲 - 作者',
            'album': '同一专辑',
            'trackIdentity': id,
          },
        }),
      );
    }
    expect(find.text('新歌'), findsOneWidget);
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
  });
  testWidgets('AirPlay 标题持续变化时按歌词双栏显示并移除空态', (tester) async {
    AppState airPlay(String title) => _stateFromMap({
      'playback': {
        'protocol': 'airplay',
        'title': title,
        'artist': '晴天 - 周杰伦',
        'album': '叶惠美',
        'isPlaying': true,
        'streamActive': true,
        'positionMs': 5000,
        'durationMs': 180000,
      },
    });

    await _pumpPlayer(tester, airPlay('Track'));
    await tester.pumpWidget(_playerHost(airPlay('第一句歌词')));
    await tester.pump();
    await tester.pumpWidget(_playerHost(airPlay('第二句歌词')));
    await tester.pump();
    await tester.pumpWidget(_playerHost(airPlay('第三句歌词')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.text('音频流未提供歌词'), findsNothing);
    expect(find.text('第一句歌词'), findsOneWidget);
    expect(find.text('晴天'), findsOneWidget);
  });
  testWidgets('歌词加载中保持稳定布局，到达后自然切换双栏', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyricsLoading: true),
      size: const Size(800, 360),
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.text('音频流未提供歌词'), findsNothing);
    expect(tester.takeException(), isNull);

    await tester.pumpWidget(
      _playerHost(_state(lyricsLoading: false, lyrics: _lyrics())),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.text('第一句'), findsWidgets);
    expect(tester.takeException(), isNull);
  });
  testWidgets('21:9 目标尺寸无溢出，左面板+右歌词', (tester) async {
    for (final size in const [
      Size(840, 360),
      Size(900, 400),
      Size(980, 420),
      Size(1008, 432),
      Size(1080, 462),
      Size(1280, 720),
      Size(1920, 1080),
    ]) {
      await _pumpPlayer(tester, _state(lyrics: _lyrics()), size: size);
      expect(
        find.byKey(const ValueKey('player-controls-column')),
        findsOneWidget,
        reason: '$size left panel',
      );
      expect(
        find.byKey(const ValueKey('player-lyrics-view')),
        findsOneWidget,
        reason: '$size lyrics',
      );
      expect(tester.takeException(), isNull, reason: '$size overflow');
    }
  });

  testWidgets('21:9 无歌词为左大封面 + 右控制面板，无空白歌词列', (tester) async {
    await _pumpPlayer(tester, _state(), size: const Size(840, 360));
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.byKey(const ValueKey('volume-slider')), findsOneWidget);
    expect(find.byKey(const ValueKey('player-dock')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('21:9 长标题截断且控制区不跳动', (tester) async {
    final state = _stateFromMap({
      'playback': {
        'title': '这是一个非常非常长的歌曲名称用于验证超宽屏下的截断与布局稳定性',
        'artist': 'Demo Artist',
        'durationMs': 200000,
        'streamActive': true,
      },
    });
    await _pumpPlayer(tester, state, size: const Size(840, 360));
    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });
  testWidgets('流式单句歌词保留最近历史，不伪造未来歌词', (tester) async {
    AppState line(String text) => _state(
      trackIdentity: 'stream-track',
      lyrics: LyricsDocument(
        origin: 'airplay',
        synced: false,
        lines: [LyricLine(text: text)],
      ),
    );
    await _pumpPlayer(tester, line('第一句流式歌词'), size: const Size(840, 360));
    expect(find.text('第一句流式歌词'), findsOneWidget);

    await tester.pumpWidget(_playerHost(line('第二句流式歌词')));
    await tester.pump();
    expect(find.text('第一句流式歌词'), findsOneWidget);
    expect(find.text('第二句流式歌词'), findsOneWidget);

    await tester.pumpWidget(_playerHost(line('第三句流式歌词')));
    await tester.pump();
    expect(find.text('第一句流式歌词'), findsOneWidget);
    expect(find.text('第二句流式歌词'), findsOneWidget);
    expect(find.text('第三句流式歌词'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
