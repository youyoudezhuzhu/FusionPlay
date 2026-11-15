import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';

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
}) => AppState(
  initialized: true,
  playback: PlaybackSnapshot(
    title: 'Track',
    artist: 'Artist',
    durationMs: 180000,
    positionMs: 200,
    streamActive: true,
    isPlaying: playing,
    lyrics: lyrics,
    lyricsLoading: lyricsLoading,
  ),
);

Widget _player(AppState state, {bool reduceMotion = false}) => MaterialApp(
  theme: ThemeData(splashFactory: NoSplash.splashFactory),
  home: Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context).copyWith(disableAnimations: reduceMotion),
      child: Scaffold(
        body: PlayerView(
          state: state,
          onPlayback: (_) {},
          onSeek: (_) {},
          onVolume: (_) {},
        ),
      ),
    ),
  ),
);

Future<void> _pumpPlayer(
  WidgetTester tester,
  AppState state, {
  bool reduceMotion = false,
  Size size = const Size(1000, 660),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(_player(state, reduceMotion: reduceMotion));
  await tester.pump();
}

void main() {
  testWidgets('无歌词且非加载时不显示歌词按钮', (tester) async {
    await _pumpPlayer(tester, _state());

    expect(find.byKey(const ValueKey('playback-lyrics-button')), findsNothing);
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
  });

  testWidgets('有歌词时按钮可用，点击进入/退出歌词模式', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()));

    final button = find.byKey(const ValueKey('playback-lyrics-button'));
    expect(button, findsOneWidget);
    final artworkBefore = tester
        .widget<ArtworkCard>(find.byType(ArtworkCard))
        .size;

    await tester.tap(button);
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    // 第一行带逐字 timing，所以渲染为逐词高亮；第二行是普通整行文本。
    expect(find.text('第一句'), findsWidgets);
    expect(find.text('第二句歌词'), findsOneWidget);
    final artworkDuring = tester
        .widget<ArtworkCard>(find.byType(ArtworkCard))
        .size;
    expect(artworkDuring, lessThan(artworkBefore));

    await tester.tap(button);
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(
      tester.widget<ArtworkCard>(find.byType(ArtworkCard)).size,
      closeTo(artworkBefore, .01),
    );
  });

  testWidgets('歌词按钮可通过 D-pad 从下一曲按钮进入并激活', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()));

    final nodes = FocusManager.instance.rootScope.descendants;
    nodes
        .singleWhere((node) => node.debugLabel == 'player-next')
        .requestFocus();
    await tester.pump();

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-lyrics');

    await tester.sendKeyEvent(LogicalKeyboardKey.select);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
  });

  testWidgets('歌词 loading 时按钮可用并显示 shimmer', (tester) async {
    await _pumpPlayer(tester, _state(lyricsLoading: true));

    final button = find.byKey(const ValueKey('playback-lyrics-button'));
    expect(button, findsOneWidget);
    await tester.tap(button);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.byKey(const ValueKey('lyrics-loading')), findsOneWidget);
  });

  testWidgets('歌词消失时自动退出歌词模式', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()));
    await tester.tap(find.byKey(const ValueKey('playback-lyrics-button')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);

    await tester.pumpWidget(_player(_state()));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsNothing);
    expect(find.byKey(const ValueKey('playback-lyrics-button')), findsNothing);
  });

  testWidgets('disableAnimations 下歌词模式切换仍然可用', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics()), reduceMotion: true);

    await tester.tap(find.byKey(const ValueKey('playback-lyrics-button')));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(find.text('第一句'), findsWidgets);
  });

  testWidgets('播放中歌词模式的本地时钟 ticker 不抛异常', (tester) async {
    await _pumpPlayer(tester, _state(lyrics: _lyrics(), playing: true));

    await tester.tap(find.byKey(const ValueKey('playback-lyrics-button')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(tester.takeException(), isNull);

    // 退出播放页，确认 ticker 被正确释放。
    await tester.pumpWidget(const MaterialApp(home: SizedBox.shrink()));
    await tester.pump();
    expect(tester.takeException(), isNull);
  });

  testWidgets('矮横屏（660 高度以下）打开歌词不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics()),
      size: const Size(620, 320),
    );

    await tester.tap(find.byKey(const ValueKey('playback-lyrics-button')));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('player-layout-horizontal')),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('紧凑（纵向）布局下歌词模式不溢出', (tester) async {
    await _pumpPlayer(
      tester,
      _state(lyrics: _lyrics()),
      size: const Size(420, 760),
    );

    await tester.tap(find.byKey(const ValueKey('playback-lyrics-button')));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('player-layout-compact')), findsOneWidget);
    expect(find.byKey(const ValueKey('player-lyrics-view')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
