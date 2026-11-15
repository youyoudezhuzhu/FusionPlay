import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';

void main() {
  group('歌词可用性（按渠道）', () {
    test('AirPlay 永远不支持歌词', () {
      for (final protocol in ['airplay', 'AirPlay', 'airplay2', 'AirPlay 2']) {
        expect(
          lyricsAvailabilityForProtocol(protocol),
          LyricsAvailability.unsupported,
          reason: protocol,
        );
      }
    });

    test('DLNA / 小米妙播 / 未知来源保持 supported', () {
      for (final protocol in [
        'dlna',
        'DLNA',
        'xiaomi_miplay',
        'miplay',
        '小米妙播',
        'custom-protocol',
        null,
        '',
      ]) {
        expect(
          lyricsAvailabilityForProtocol(protocol),
          LyricsAvailability.supported,
          reason: '$protocol',
        );
      }
    });

    test('渠道切换: AirPlay→DLNA 的可用性随之变化', () {
      expect(
        lyricsAvailabilityForProtocol('airplay'),
        LyricsAvailability.unsupported,
      );
      expect(
        lyricsAvailabilityForProtocol('dlna'),
        LyricsAvailability.supported,
      );
    });
  });

  group('渠道分类与协议标签', () {
    test('playbackChannelForProtocol', () {
      expect(playbackChannelForProtocol('airplay'), PlaybackChannel.airPlay);
      expect(playbackChannelForProtocol('dlna'), PlaybackChannel.dlna);
      expect(
        playbackChannelForProtocol('xiaomi_miplay'),
        PlaybackChannel.miPlay,
      );
      expect(playbackChannelForProtocol('小米妙播'), PlaybackChannel.miPlay);
      expect(playbackChannelForProtocol('weird'), PlaybackChannel.other);
      expect(playbackChannelForProtocol(null), PlaybackChannel.other);
    });

    test('AirPlay 标签统一为 AirPlay 2', () {
      expect(playbackProtocolLabel('airplay'), 'AirPlay 2');
      expect(playbackProtocolLabel('AirPlay'), 'AirPlay 2');
      expect(playbackProtocolLabel('dlna'), 'DLNA');
      expect(playbackProtocolLabel('xiaomi_miplay'), '小米妙播');
      expect(playbackProtocolLabel('custom'), 'custom');
      expect(playbackProtocolLabel(null), isNull);
      expect(playbackProtocolLabel('  '), isNull);
    });
  });

  group('渠道空态文案', () {
    test('DLNA 提示发送端未提供并提供 .lrc 建议', () {
      final copy = lyricsEmptyCopyForChannel(PlaybackChannel.dlna);
      expect(copy.title, '此发送端未提供歌词');
      expect(copy.description, contains('.lrc'));
      expect(copy.description, contains('投屏 App'));
    });

    test('小米妙播提示本曲未附带', () {
      final copy = lyricsEmptyCopyForChannel(PlaybackChannel.miPlay);
      expect(copy.title, '此歌曲没有歌词');
      expect(copy.description, contains('手机端'));
      expect(copy.description, contains('当前歌曲未附带'));
    });

    test('未知来源保持通用文案', () {
      final copy = lyricsEmptyCopyForChannel(PlaybackChannel.other);
      expect(copy.title, '暂无歌词');
      expect(copy.description, '歌词由发送端提供');
    });
  });
}
