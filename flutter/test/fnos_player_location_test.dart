import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/fnos/player_location.dart';

void main() {
  final base = Uri.parse('https://nas/app/fusionplay-fnos/player/index.html');
  for (final id in ['default', 'studio', '-studio']) {
    test(
      '$id bookmark keeps status, controls, cover and settings on that speaker',
      () {
        final page = base.replace(queryParameters: {'instance': id});
        final location = SpeakerPlayerLocation(page);
        final prefix = 'https://nas/app/fusionplay-fnos/api/instances/$id/';
        expect(location.api('status').toString(), '${prefix}status');
        expect(location.api('player').toString(), '${prefix}player');
        expect(
          location.artwork('artwork/cover.png').toString(),
          '${prefix}artwork/cover.png',
        );
        expect(
          location.artwork('https://sender/art.png').toString(),
          'https://sender/art.png',
        );
        expect(location.settings.queryParameters['instance'], id);
        expect(location.settings.fragment, '/settings');
        expect(
          location.speakerPicker.path,
          '/app/fusionplay-fnos/speakers.html',
        );
        expect(location.speakerPicker.fragment, 'rooms');
        expect(location.createInstance.fragment, 'new');
        expect(location.createInstance.queryParameters['instance'], id);
        expect(SpeakerPlayerLocation(location.select('kitchen')).id, 'kitchen');
        expect(location.id, id);
      },
    );
  }

  test('unscoped launch pins the advertised default once', () {
    final root = SpeakerPlayerLocation(base);
    expect(() => root.api('player'), throwsFormatException);
    final studio = root.resolveDefault('studio', ['studio', 'kitchen']);
    expect(studio.id, 'studio');
    expect(studio.resolveDefault('kitchen', ['kitchen']).id, 'studio');
    expect(root.resolveDefault('studio', []).pinned, isFalse);
  });

  test('malformed bookmarks never reach the default player', () {
    for (final id in ['', '..', '../default', 'a/b', 'x' * 41]) {
      final location = SpeakerPlayerLocation(
        base.replace(queryParameters: {'instance': id}),
      );
      expect(location.resolveDefault('default', ['default']).id, id);
      expect(() => location.api('player'), throwsFormatException);
      expect(location.catalog.path, '/app/fusionplay-fnos/api/instances');
    }
  });
}
