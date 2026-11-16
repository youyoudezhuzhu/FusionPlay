import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';

void main() {
  test('AirPlay 本地时钟在两次 now-playing 事件之间推进进度', () {
    expect(
      airPlayInterpolatedPositionMs(
        anchorMs: 10000,
        elapsed: const Duration(milliseconds: 1200),
      ),
      11200,
    );
  });

  test('AirPlay 本地时钟不会超过总时长，也不会倒退', () {
    expect(
      airPlayInterpolatedPositionMs(
        anchorMs: 179500,
        elapsed: const Duration(seconds: 3),
        durationMs: 180000,
      ),
      180000,
    );
    expect(
      airPlayInterpolatedPositionMs(
        anchorMs: 500,
        elapsed: const Duration(milliseconds: -20),
        durationMs: 180000,
      ),
      500,
    );
  });
}
