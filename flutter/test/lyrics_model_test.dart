import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';

LyricsDocument _syncedDocument({
  bool withWords = true,
  List<int?> starts = const [0, 1000, 2000, 3000],
}) {
  final lines = <LyricLine>[
    for (var index = 0; index < starts.length; index++)
      LyricLine(
        startMs: starts[index],
        text: '第 $index 行',
        words: withWords
            ? <LyricsWord>[
                LyricsWord(
                  startMs: (starts[index] ?? 0) + 10,
                  endMs: (starts[index] ?? 0) + 400,
                  text: '第',
                ),
                LyricsWord(
                  startMs: (starts[index] ?? 0) + 400,
                  text: '$index 行',
                ),
              ]
            : const <LyricsWord>[],
      ),
  ];
  return LyricsDocument(origin: 'embedded', synced: true, lines: lines);
}

void main() {
  group('AppState 歌词解析', () {
    test('解析同步歌词、origin、逐字 timing 与 loading', () {
      final state = AppState.fromMap(const {
        'playback': {
          'title': 'Track',
          'durationMs': 200000,
          'lyricsLoading': true,
          'lyrics': {
            'origin': 'metadata_uri',
            'synced': true,
            'lines': [
              {
                'startMs': 0,
                'text': '第一行',
                'words': [
                  {'startMs': 0, 'endMs': 400, 'text': '第一'},
                  {'startMs': 400, 'text': '行'},
                ],
              },
              {'startMs': 1500, 'text': '第二行', 'words': <Object?>[]},
            ],
          },
        },
      });

      final playback = state.playback;
      expect(playback.lyricsLoading, isTrue);
      final lyrics = playback.lyrics!;
      expect(lyrics.origin, 'metadata_uri');
      expect(lyrics.synced, isTrue);
      expect(lyrics.isSynced, isTrue);
      expect(lyrics.lines, hasLength(2));
      expect(lyrics.lines.first.words, hasLength(2));
      expect(lyrics.lines.first.words.first.startMs, 0);
      expect(lyrics.lines.first.words.first.endMs, 400);
      expect(lyrics.lines.first.words.last.endMs, isNull);
      expect(lyrics.lines.last.hasWords, isFalse);
      expect(lyrics.hasWordTiming, isTrue);
    });

    test('缺失 lyrics / 空 lines / 缺字段均容错', () {
      expect(
        AppState.fromMap(const {
          'playback': <String, Object?>{},
        }).playback.lyrics,
        isNull,
      );
      expect(
        AppState.fromMap(const {
          'playback': {'lyrics': null},
        }).playback.lyrics,
        isNull,
      );
      expect(
        AppState.fromMap(const {
          'playback': {
            'lyrics': {'lines': <Object?>[]},
          },
        }).playback.lyrics,
        isNull,
      );

      final lyrics = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'lines': [
              {'text': '只有文本'},
            ],
          },
        },
      }).playback.lyrics!;
      expect(lyrics.synced, isFalse);
      expect(lyrics.isSynced, isFalse);
      expect(lyrics.lines.single.startMs, isNull);
      expect(lyrics.lines.single.words, isEmpty);
      expect(lyrics.origin, isNull);
    });

    test('所有 startMs 为 null 时不做同步高亮', () {
      final lyrics = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': null, 'text': '静态一'},
              {'text': '静态二'},
            ],
          },
        },
      }).playback.lyrics!;

      expect(lyrics.synced, isTrue);
      expect(lyrics.isSynced, isFalse);
      expect(lyrics.activeLineIndex(5000), -1);
    });

    test('逐字歌词全是空 words 时按整行处理', () {
      final lyrics = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': 100, 'text': '整行', 'words': <Object?>[]},
            ],
          },
        },
      }).playback.lyrics!;
      expect(lyrics.isSynced, isTrue);
      expect(lyrics.hasWordTiming, isFalse);
      expect(lyrics.activeLineIndex(150), 0);
    });

    test('PlaybackSnapshot 相等性包含歌词，用于 10Hz 状态去重', () {
      final withLyrics = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': 0, 'text': '一'},
            ],
          },
        },
      });
      final sameAgain = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': 0, 'text': '一'},
            ],
          },
        },
      });
      final withoutLyrics = AppState.fromMap(const {
        'playback': <String, Object?>{},
      });

      expect(withLyrics, sameAgain);
      expect(withLyrics == withoutLyrics, isFalse);
      expect(withLyrics.playback.hashCode, sameAgain.playback.hashCode);
    });

    test('歌词 lines 乱序时按 startMs 排序', () {
      final lyrics = AppState.fromMap(const {
        'playback': {
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': 3000, 'text': 'C'},
              {'startMs': 1000, 'text': 'A'},
              {'startMs': 2000, 'text': 'B'},
            ],
          },
        },
      }).playback.lyrics!;

      expect(lyrics.lines.map((line) => line.text), ['A', 'B', 'C']);
    });
  });

  group('LyricsDocument.activeLineIndex', () {
    final document = _syncedDocument(starts: const [0, 1000, 2000, 3000]);

    test('边界与二分定位', () {
      expect(document.activeLineIndex(-500), 0);
      expect(document.activeLineIndex(0), 0);
      expect(document.activeLineIndex(999), 0);
      expect(document.activeLineIndex(1000), 1);
      expect(document.activeLineIndex(2500), 2);
      expect(document.activeLineIndex(3000), 3);
      expect(document.activeLineIndex(999999), 3);
    });

    test('非同步文档返回 -1', () {
      final unsynced = LyricsDocument(synced: false, lines: document.lines);
      expect(unsynced.activeLineIndex(1500), -1);
    });

    test('null startMs 行不参与高亮查询', () {
      final withUntimed = LyricsDocument(
        synced: true,
        lines: [
          const LyricLine(startMs: 0, text: 'A'),
          const LyricLine(text: '无时间'),
          const LyricLine(startMs: 2000, text: 'B'),
        ],
      );
      expect(withUntimed.activeLineIndex(1000), 0);
      expect(withUntimed.activeLineIndex(2500), 2);
    });
  });

  group('LyricsDocument.wordProgressAt', () {
    test('使用 endMs 计算进度并夹取 0..1', () {
      final document = _syncedDocument(starts: const [0]);
      final line = document.lines.single;
      expect(line.words.first.startMs, 10);
      expect(line.words.first.endMs, 400);

      expect(document.wordProgressAt(0, 0, 0), 0);
      expect(document.wordProgressAt(0, 0, 205), closeTo(.5, .001));
      expect(document.wordProgressAt(0, 0, 999), 1);
    });

    test('缺 endMs 时用下一个词起点', () {
      final document = LyricsDocument(
        synced: true,
        lines: const [
          LyricLine(
            startMs: 0,
            text: 'ab',
            words: [
              LyricsWord(startMs: 0, text: 'a'),
              LyricsWord(startMs: 500, text: 'b'),
            ],
          ),
        ],
      );
      expect(document.wordProgressAt(0, 0, 250), closeTo(.5, .001));
      expect(document.wordProgressAt(0, 0, 500), 1);
      // 最后一个词没有下一词/下一行，默认给 4s 兜底。
      expect(document.wordProgressAt(0, 1, 750), closeTo(.0625, .001));
    });

    test('行末词缺 endMs 时用下一行起点', () {
      final document = LyricsDocument(
        synced: true,
        lines: const [
          LyricLine(
            startMs: 0,
            text: 'A',
            words: [LyricsWord(startMs: 0, text: 'A')],
          ),
          LyricLine(startMs: 2000, text: 'B'),
        ],
      );
      expect(document.wordProgressAt(0, 0, 1000), closeTo(.5, .001));
      expect(document.lineEndMs(0), 2000);
    });

    test('最后一行最后词用时长兜底', () {
      final document = LyricsDocument(
        synced: true,
        lines: const [
          LyricLine(
            startMs: 1000,
            text: 'A',
            words: [LyricsWord(startMs: 1000, text: 'A')],
          ),
        ],
      );
      expect(
        document.wordProgressAt(0, 0, 3000, durationMs: 5000),
        closeTo(.5, .001),
      );
      expect(document.lineEndMs(0, durationMs: 5000), 5000);
      // 无时长时给 4s 兜底，避免出现除零/无限进度。
      expect(document.wordProgressAt(0, 0, 3000), closeTo(.5, .001));
    });

    test('越界与零长度词', () {
      final document = LyricsDocument(
        synced: true,
        lines: const [
          LyricLine(
            startMs: 0,
            text: 'x',
            words: [LyricsWord(startMs: 1000, endMs: 1000, text: 'x')],
          ),
        ],
      );
      expect(document.wordProgressAt(-1, 0, 1000), 0);
      expect(document.wordProgressAt(0, -1, 1000), 0);
      expect(document.wordProgressAt(0, 9, 1000), 0);
      expect(document.wordProgressAt(0, 0, 999), 0);
      expect(document.wordProgressAt(0, 0, 1000), 1);
    });
  });
}
