import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/fnos/lyrics.dart';
import 'package:fusionplay_android_flutter/models/lyrics.dart';

void main() {
  test(
    'fnOS passes repeated LRC timestamps and plain lines to the shared player',
    () {
      final doc = parseLyrics(
        receiverLyrics('[ar:Muse]\n[00:03.12][00:08.5]Moon\n[00:09]Stars'),
      );
      expect(doc!.synced, isTrue);
      expect(doc.lines.map((line) => line.startMs), [3120, 8500, 9000]);
      expect(doc.lines.map((line) => line.text), ['Moon', 'Moon', 'Stars']);
      expect(parseLyrics(receiverLyrics('一行歌词'))!.synced, isFalse);
      expect(receiverLyrics(''), isNull);
    },
  );
}
