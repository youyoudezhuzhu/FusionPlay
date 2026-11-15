import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart';
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
    // dock / 玻璃组件走 fallback 渲染。
    expect(find.byType(LiquidGlass), findsWidgets);
    expect(
      find.byKey(const ValueKey('playback-play-pause-button')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('volume-slider')), findsOneWidget);
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
    expect(find.text('暂无歌词'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('AirPlay 无 remote 命令时隐藏上一曲/下一曲，播放保留', (tester) async {
    final state = _stateFromMap({
      'playback': {'protocol': 'airplay'},
      'remoteControl': {'available': false, 'commands': <Object?>[]},
    });
    await _pumpPlayer(tester, state);

    expect(
      find.byKey(const ValueKey('playback-previous-button')),
      findsNothing,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsNothing);
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

  testWidgets('显式 controls 可隐藏 skip 并禁用 seek', (tester) async {
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
      findsNothing,
    );
    expect(find.byKey(const ValueKey('playback-next-button')), findsNothing);
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

  testWidgets('拖动中立即跟手：onChanged 只更新本地滑杆值', (tester) async {
    final volumes = <int>[];
    await _pumpPlayer(tester, _state(volumePercent: 65), onVolume: volumes.add);

    final slider = find.byKey(const ValueKey('volume-slider'));
    final widget = tester.widget<RoundedProgressBar>(slider);
    widget.onChanged(.2);
    await tester.pump();
    final value = tester
        .widget<RoundedProgressBar>(find.byKey(const ValueKey('volume-slider')))
        .value;
    expect(value, closeTo(.2, .001));
    // 130ms 防抖后才回写。
    expect(volumes, isEmpty);
    await tester.pump(const Duration(milliseconds: 150));
    expect(volumes.last, 20);
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
    expect(find.byKey(const ValueKey('player-layout-compact')), findsOneWidget);
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    await tester.drag(
      find.byKey(const ValueKey('player-layout-compact')),
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

  testWidgets('AirPlay 无歌词：居中布局 + AirPlay 2 不提供歌词', (tester) async {
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
    expect(find.text('AirPlay 2 不提供歌词'), findsOneWidget);
    expect(find.text('暂无歌词'), findsNothing);
    // 顶部保留音频来源（协议）胶囊，但不再显示音质胶囊。
    expect(find.text('AirPlay 2'), findsWidgets);
    expect(find.text('AAC / 44.1 kHz / 16-bit / 2 ch'), findsNothing);
    // AirPlay 永远不出现歌词开关。
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
    expect(find.text('此发送端未提供歌词'), findsOneWidget);
    expect(find.text('暂无歌词'), findsNothing);
  });

  testWidgets('小米妙播无歌词：居中布局 + 本曲没有歌词', (tester) async {
    await _pumpPlayer(tester, _state(protocol: 'xiaomi_miplay'));

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.text('此歌曲没有歌词'), findsOneWidget);
    expect(find.text('暂无歌词'), findsNothing);
  });

  testWidgets('AirPlay 即使带 lyrics 也走居中布局（容错不崩）', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics(), protocol: 'airplay'));

    expect(
      find.byKey(const ValueKey('player-centered-layout')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.text('第一句'), findsNothing);
    expect(find.text('AirPlay 2 不提供歌词'), findsOneWidget);
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

  testWidgets('手机横屏空态完整显示在 dock 上方（不被裁切/重叠）', (tester) async {
    await _pumpPlayer(
      tester,
      const AppState(initialized: true),
      size: const Size(800, 360),
    );

    expect(find.text('暂无音乐'), findsOneWidget);
    expect(find.text('暂无歌词'), findsOneWidget);
    final hintBottom = tester.getBottomRight(find.text('暂无歌词')).dy;
    final dockTop = tester
        .getTopLeft(find.byKey(const ValueKey('player-dock')))
        .dy;
    expect(hintBottom, lessThanOrEqualTo(dockTop), reason: '空态提示不能被底部 dock 覆盖');
    expect(tester.takeException(), isNull);
  });

  testWidgets('dock 中播放按钮在进度条与屏幕底部之间垂直居中', (tester) async {
    await _pumpPlayer(tester, _state(), size: const Size(800, 360));
    // Let the control entrance (5px translate) settle before measuring.
    await tester.pump(const Duration(milliseconds: 400));

    final play = tester.getRect(
      find.byKey(const ValueKey('playback-play-pause-button')),
    );
    final progress = tester.getRect(
      find.byKey(const ValueKey('player-progress-bar')),
    );
    final gapAbove = play.top - progress.bottom;
    final gapBelow = 360 - play.bottom;
    expect(gapAbove, closeTo(gapBelow, 0.5), reason: '播放按钮上下留白应一致，而不是偏向底部');
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
}
