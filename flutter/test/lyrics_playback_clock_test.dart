import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/lyrics_playback_clock.dart';

void main() {
  late DateTime now;

  LyricsPlaybackClock clock() => LyricsPlaybackClock(now: () => now);

  setUp(() {
    now = DateTime(2026, 11, 15, 9, 0, 0);
  });

  test('播放时按墙钟外推位置', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(positionMs: 1000, isPlaying: true, durationMs: 10000);
    expect(playbackClock.positionMs, 1000);

    now = now.add(const Duration(milliseconds: 250));
    expect(playbackClock.positionMs, 1250);

    now = now.add(const Duration(milliseconds: 750));
    expect(playbackClock.positionMs, 2000);
  });

  test('小回退样本被单调处理，不产生抖动', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(positionMs: 1000, isPlaying: true);
    now = now.add(const Duration(milliseconds: 250));
    expect(playbackClock.positionMs, 1250);

    // 10Hz 采样通常比外推位置略旧，不能把高亮往回拽。
    playbackClock.update(positionMs: 1100, isPlaying: true);
    expect(playbackClock.positionMs, 1250);

    now = now.add(const Duration(milliseconds: 100));
    expect(playbackClock.positionMs, 1350);
  });

  test('大幅回退样本视为 seek 并重锚', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(positionMs: 5000, isPlaying: true);
    now = now.add(const Duration(milliseconds: 100));
    expect(playbackClock.positionMs, 5100);

    playbackClock.update(positionMs: 1000, isPlaying: true);
    expect(playbackClock.positionMs, 1000);
    now = now.add(const Duration(milliseconds: 100));
    expect(playbackClock.positionMs, 1100);
  });

  test('暂停时冻结位置，恢复播放后继续外推', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(positionMs: 2000, isPlaying: true);
    now = now.add(const Duration(milliseconds: 300));
    playbackClock.update(positionMs: 2300, isPlaying: false);

    now = now.add(const Duration(seconds: 5));
    expect(playbackClock.positionMs, 2300);
    expect(playbackClock.isPlaying, isFalse);

    playbackClock.update(positionMs: 2300, isPlaying: true);
    now = now.add(const Duration(milliseconds: 500));
    expect(playbackClock.positionMs, 2800);
  });

  test('换曲（trackKey 变化）直接重锚，不做单调处理', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(
      positionMs: 5000,
      isPlaying: true,
      trackKey: 'track-a',
    );
    now = now.add(const Duration(milliseconds: 200));
    expect(playbackClock.positionMs, 5200);

    playbackClock.update(positionMs: 0, isPlaying: true, trackKey: 'track-b');
    expect(playbackClock.positionMs, 0);
  });

  test('位置被时长夹取', () {
    final playbackClock = clock();
    addTearDown(playbackClock.dispose);

    playbackClock.update(positionMs: 9900, isPlaying: true, durationMs: 10000);
    now = now.add(const Duration(seconds: 2));
    expect(playbackClock.positionMs, 10000);
  });

  test('dispose 后 update 不抛异常', () {
    final playbackClock = clock();
    playbackClock.dispose();
    expect(
      () => playbackClock.update(positionMs: 100, isPlaying: true),
      returnsNormally,
    );
  });
}
