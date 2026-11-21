import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';

void main() {
  LyricsDocument? fromWire(String protocol, Object? lyrics) =>
      PlaybackSnapshot.fromMap({'protocol': protocol, 'lyrics': lyrics}).lyrics;

  test('DLNA passes repeated timestamps and plain lines to the player', () {
    final doc = fromWire(
      'DLNA',
      '[ar:Muse]\n[00:03.12][00:08.5]Moon\n[00:09]Stars',
    );
    expect(doc!.isSynced, isTrue);
    expect(doc.lines.map((line) => line.startMs), [3120, 8500, 9000]);
    expect(doc.lines.map((line) => line.text), ['Moon', 'Moon', 'Stars']);
    expect(fromWire('DLNA', '一行歌词')!.isSynced, isFalse);
    expect(fromWire('DLNA', ''), isNull);
    expect(fromWire('DLNA', null), isNull);
  });

  test('MiPlay timed words retain their timestamps across host transports', () {
    const line = '[00:01.00]{Hello }[00:01.50]{world}';
    final doc = fromWire('miplay', line)!;
    expect(doc.lines.single.text, 'Hello world');
    expect(doc.lines.single.words.map((w) => w.startMs), [1000, 1500]);
    expect(fromWire('小米妙播', line), doc);
  });

  test(
    'AirPlay current lines and native structured documents retain their origins',
    () {
      final line = fromWire('AirPlay', '[Chorus]')!;
      expect(line.origin, 'airplay_title');
      expect(line.lines.single.text, '[Chorus]');
      final native = fromWire('AirPlay', {
        'origin': 'metadata_uri',
        'synced': true,
        'lines': [
          {'startMs': 500, 'text': 'A', 'translation': '译'},
        ],
      })!;
      expect(native.origin, 'metadata_uri');
      expect(native.lines.single.translation, '译');
      expect(native.isSynced, isTrue);
    },
  );
}
