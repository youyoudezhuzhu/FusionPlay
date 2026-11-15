import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';

Map<String, Object?> _stateMap({
  String? protocol,
  Map<String, Object?>? remoteControl,
  Map<String, Object?>? controls,
  Map<String, Object?> playback = const <String, Object?>{},
}) => <String, Object?>{
  'initialized': true,
  'playback': <String, Object?>{
    'title': 'Track',
    'durationMs': 180000,
    'streamActive': true,
    'protocol': ?protocol,
    ...playback,
    'controls': ?controls,
  },
  'remoteControl': ?remoteControl,
  'controls': ?controls,
};

void main() {
  group('PlaybackControls 解析', () {
    test('根级 controls 优先解析', () {
      final state = AppState.fromMap(
        _stateMap(
          protocol: 'airplay',
          controls: const {
            'playPause': true,
            'previous': false,
            'next': false,
            'seek': true,
          },
        ),
      );
      expect(state.controls.playPause, isTrue);
      expect(state.controls.previous, isFalse);
      expect(state.controls.next, isFalse);
      expect(state.controls.seek, isTrue);
    });

    test('兼容 playback 内旧 controls 字段', () {
      final state = AppState.fromMap({
        'playback': {
          'title': 'Track',
          'durationMs': 1000,
          'streamActive': true,
          'controls': {
            'playPause': true,
            'previous': true,
            'next': true,
            'seek': false,
          },
        },
      });
      expect(state.controls.previous, isTrue);
      expect(state.controls.seek, isFalse);
    });

    test('缺省 fallback：小米妙播视为全 true', () {
      final state = AppState.fromMap(_stateMap(protocol: 'xiaomi_miplay'));
      expect(state.controls, PlaybackControls.all);

      final chinese = AppState.fromMap(_stateMap(protocol: '小米妙播'));
      expect(chinese.controls, PlaybackControls.all);
    });

    test('缺省 fallback：AirPlay/DLNA 只看 remoteControl.commands', () {
      final state = AppState.fromMap(
        _stateMap(
          protocol: 'airplay',
          remoteControl: const {'available': true, 'commands': <Object?>[]},
        ),
      );
      expect(state.controls.playPause, isTrue);
      expect(state.controls.previous, isFalse);
      expect(state.controls.next, isFalse);
      expect(state.controls.seek, isTrue);

      final withCommands = AppState.fromMap(
        _stateMap(
          protocol: 'airplay',
          remoteControl: const {
            'available': true,
            'commands': <Object?>['play_pause', 'previous_track'],
          },
        ),
      );
      expect(withCommands.controls.previous, isTrue);
      expect(withCommands.controls.next, isFalse);

      final dlna = AppState.fromMap(
        _stateMap(
          protocol: 'dlna',
          remoteControl: const {'available': false, 'commands': <Object?>[]},
        ),
      );
      expect(dlna.controls.previous, isFalse);
      expect(dlna.controls.next, isFalse);
    });

    test('无协议旧状态（Windows/占位）保持宽容 fallback', () {
      final state = AppState.fromMap(_stateMap());
      expect(state.controls.playPause, isTrue);
      expect(state.controls.previous, isTrue);
      expect(state.controls.next, isTrue);
      expect(state.controls.seek, isTrue);
    });

    test('没有任何播放内容时不显示 playback 控制', () {
      final state = AppState.fromMap(const {'playback': <String, Object?>{}});
      expect(state.controls.playPause, isFalse);
      expect(state.controls.previous, isFalse);
      expect(state.controls.next, isFalse);
      expect(state.controls.seek, isFalse);
    });

    test('PlaybackControls 相等性与 copyWith', () {
      expect(
        const PlaybackControls(previous: false),
        const PlaybackControls(previous: false),
      );
      expect(
        const PlaybackControls().copyWith(next: false),
        const PlaybackControls(next: false),
      );
      expect(
        const PlaybackControls().hashCode,
        const PlaybackControls().hashCode,
      );
    });
  });
}
