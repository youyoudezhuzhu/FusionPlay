import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/lyrics.dart';
import 'package:fusionplay_android_flutter/ui/lyrics_playback_clock.dart';
import 'package:fusionplay_android_flutter/ui/lyrics_view.dart';

LyricsDocument _syncedLines(List<String> texts, {bool withWords = false}) =>
    LyricsDocument(
      origin: 'embedded',
      synced: true,
      lines: [
        for (var index = 0; index < texts.length; index++)
          LyricLine(
            startMs: index * 1000,
            text: texts[index],
            words: withWords
                ? <LyricsWord>[
                    LyricsWord(
                      startMs: index * 1000,
                      endMs: index * 1000 + 400,
                      text: '${texts[index]}-词1',
                    ),
                    LyricsWord(
                      startMs: index * 1000 + 400,
                      endMs: index * 1000 + 900,
                      text: '${texts[index]}-词2',
                    ),
                  ]
                : const <LyricsWord>[],
          ),
      ],
    );

Widget _wrap(
  Widget child, {
  bool reduceMotion = false,
  Size size = const Size(360, 220),
}) => MaterialApp(
  home: Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context).copyWith(disableAnimations: reduceMotion),
      child: Scaffold(
        backgroundColor: Colors.black,
        body: Center(
          child: SizedBox(width: size.width, height: size.height, child: child),
        ),
      ),
    ),
  ),
);

LyricsPlaybackClock _clockAt(DateTime Function() now, int positionMs) {
  final clock = LyricsPlaybackClock(now: now);
  clock.update(positionMs: positionMs, isPlaying: false);
  return clock;
}

void main() {
  testWidgets('loading 显示 shimmer 骨架屏', (tester) async {
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: null,
          loading: true,
          isPlaying: false,
          positionMs: 0,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 120));

    expect(find.byKey(const ValueKey('lyrics-loading')), findsOneWidget);
    expect(find.byKey(const ValueKey('lyrics-empty')), findsNothing);
    expect(find.text('暂无歌词'), findsNothing);
  });

  testWidgets('空歌词显示图标与提示', (tester) async {
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: null,
          loading: false,
          isPlaying: false,
          positionMs: 0,
        ),
      ),
    );
    await tester.pump();

    expect(find.byKey(const ValueKey('lyrics-empty')), findsOneWidget);
    expect(find.text('暂无歌词'), findsOneWidget);
    expect(find.text('歌词由发送端提供'), findsOneWidget);
  });

  testWidgets('load 中但已有歌词时优先展示歌词', (tester) async {
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: _syncedLines(const ['第一句']),
          loading: true,
          isPlaying: false,
          positionMs: 0,
        ),
      ),
    );
    await tester.pump();

    expect(find.byKey(const ValueKey('lyrics-loading')), findsNothing);
    expect(find.text('第一句'), findsOneWidget);
  });

  testWidgets('非同步歌词渲染为可滚动静态列表', (tester) async {
    final lyrics = LyricsDocument(
      synced: false,
      lines: [
        for (var index = 0; index < 20; index++)
          LyricLine(text: '静态第 $index 行'),
      ],
    );
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: lyrics,
          loading: false,
          isPlaying: false,
          positionMs: 0,
        ),
      ),
    );
    await tester.pump();

    expect(find.text('静态第 0 行'), findsOneWidget);
    expect(find.text('静态第 19 行'), findsOneWidget);
    expect(find.byKey(const ValueKey('active-lyric-line')), findsNothing);

    final position = tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position;
    expect(position.pixels, 0);
    await tester.drag(
      find.byType(SingleChildScrollView),
      const Offset(0, -120),
    );
    await tester.pump();
    expect(position.pixels, greaterThan(0));
  });

  testWidgets('同步歌词按时间高亮当前行并随位置切换', (tester) async {
    final clock = _clockAt(DateTime.now, 1500);
    addTearDown(clock.dispose);
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: _syncedLines(const ['第一段', '第二段', '第三段']),
          loading: false,
          isPlaying: false,
          positionMs: 1500,
          clock: clock,
        ),
      ),
    );
    await tester.pump();

    final marker = find.byKey(const ValueKey('active-lyric-line'));
    expect(marker, findsOneWidget);
    final activeStack = find
        .ancestor(of: marker, matching: find.byType(Stack))
        .first;
    expect(
      find.descendant(of: activeStack, matching: find.text('第二段')),
      findsOneWidget,
    );

    final activeScale = tester.widget<AnimatedScale>(
      find.ancestor(of: marker, matching: find.byType(AnimatedScale)).first,
    );
    expect(activeScale.scale, 1.04);

    clock.update(positionMs: 2400, isPlaying: false);
    await tester.pump();
    expect(
      find.descendant(
        of: find.ancestor(of: marker, matching: find.byType(Stack)).first,
        matching: find.text('第三段'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('逐字歌词按 clock 进度填色，可跨词连续', (tester) async {
    final clock = _clockAt(DateTime.now, 200);
    addTearDown(clock.dispose);
    await tester.pumpWidget(
      _wrap(
        SizedBox(
          child: LyricsView(
            lyrics: _syncedLines(const ['第一段'], withWords: true),
            loading: false,
            isPlaying: false,
            positionMs: 200,
            durationMs: 10000,
            clock: clock,
          ),
        ),
      ),
    );
    await tester.pump();

    List<LyricsWordFill> fills() =>
        tester.widgetList<LyricsWordFill>(find.byType(LyricsWordFill)).toList();

    expect(fills(), hasLength(2));
    expect(fills()[0].progress, closeTo(.5, .001));
    expect(fills()[1].progress, 0);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('karaoke-word-0-0')),
        matching: find.byType(ClipRect),
      ),
      findsOneWidget,
    );

    clock.update(positionMs: 650, isPlaying: false);
    await tester.pump();
    expect(fills()[0].progress, 1);
    expect(fills()[1].progress, closeTo(.5, .001));

    clock.update(positionMs: 0, isPlaying: false);
    await tester.pump();
    expect(fills()[0].progress, 0);
    expect(fills()[1].progress, 0);
  });

  testWidgets('自动滚动到当前行，手动滚动后暂停并显示「回到当前」', (tester) async {
    final clock = _clockAt(DateTime.now, 0);
    addTearDown(clock.dispose);
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: _syncedLines(const [
            '行0',
            '行1',
            '行2',
            '行3',
            '行4',
            '行5',
            '行6',
            '行7',
            '行8',
            '行9',
            '行10',
            '行11',
            '行12',
            '行13',
            '行14',
            '行15',
          ]),
          loading: false,
          isPlaying: false,
          positionMs: 0,
          clock: clock,
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    final scrollable = tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position;
    final initialOffset = scrollable.pixels;
    expect(initialOffset, lessThan(60));

    clock.update(positionMs: 9000, isPlaying: false);
    await tester.pump();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    expect(scrollable.pixels, greaterThan(initialOffset + 50));

    // 手动拖动 → 暂停自动滚动，浮出「回到当前」。
    await tester.drag(find.byType(SingleChildScrollView), const Offset(0, 80));
    await tester.pump();
    AnimatedOpacity chip() => tester.widget<AnimatedOpacity>(
      find
          .ancestor(
            of: find.text('回到当前'),
            matching: find.byType(AnimatedOpacity),
          )
          .first,
    );
    expect(chip().opacity, 1);

    // 4 秒后自动恢复，chip 淡出并滚回当前行。
    await tester.pump(const Duration(seconds: 4));
    await tester.pump();
    expect(chip().opacity, 0);
    await tester.pump(const Duration(milliseconds: 500));
    expect(scrollable.pixels, greaterThan(0));
  });

  testWidgets('disableAnimations 时高亮不做缩放缓动', (tester) async {
    final clock = _clockAt(DateTime.now, 1000);
    addTearDown(clock.dispose);
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: _syncedLines(const ['第一段', '第二段', '第三段']),
          loading: false,
          isPlaying: false,
          positionMs: 1000,
          clock: clock,
        ),
        reduceMotion: true,
      ),
    );
    await tester.pump();

    final marker = find.byKey(const ValueKey('active-lyric-line'));
    expect(marker, findsOneWidget);
    final activeScale = tester.widget<AnimatedScale>(
      find.ancestor(of: marker, matching: find.byType(AnimatedScale)).first,
    );
    expect(activeScale.scale, 1);
    final activeTextStyle = tester.widget<AnimatedDefaultTextStyle>(
      find
          .ancestor(of: marker, matching: find.byType(AnimatedDefaultTextStyle))
          .first,
    );
    expect(activeTextStyle.duration, Duration.zero);
  });

  testWidgets('窄屏紧凑布局：长行与逐字歌词不溢出', (tester) async {
    final clock = _clockAt(DateTime.now, 0);
    addTearDown(clock.dispose);
    await tester.pumpWidget(
      _wrap(
        LyricsView(
          lyrics: _syncedLines(const [
            '这是一行非常非常长的歌词内容，用来验证在窄屏和紧凑布局下文字会自动换行而不是溢出屏幕。',
            '第二行',
          ], withWords: true),
          loading: false,
          isPlaying: false,
          positionMs: 0,
          compact: true,
          clock: clock,
        ),
        size: const Size(180, 150),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(tester.takeException(), isNull);
    expect(find.byType(LyricsWordFill), findsWidgets);
  });
}
