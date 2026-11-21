import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';

void main() {
  setUpAll(() async {
    await (FontLoader(
      'MaterialIcons',
    )..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'))).load();
    await (FontLoader(
      'MiSans',
    )..addFont(rootBundle.load('assets/fonts/MiSansUI.ttf'))).load();
  });
  for (final item in [
    ('airplay', 'AirPlay 2'),
    ('xiaomi_miplay', '小米妙播'),
    ('dlna', 'DLNA'),
  ]) {
    testWidgets('native playback payload to rendered ${item.$2} home', (
      tester,
    ) async {
      final controller = NativeAppController();
      addTearDown(controller.dispose);
      final cover = File(
        '../fusionplay-ui-audit-codex/assets/cover.png',
      ).absolute;
      final source = item.$1 == 'xiaomi_miplay'
          ? 'data:image/png;base64,${base64Encode(cover.readAsBytesSync())}'
          : cover.uri.toString();
      AppState state(String art, String protocol) => AppState.fromMap({
        'initialized': true,
        'playback': {
          'title': 'Starlight',
          'artist': 'Muse',
          'album': 'Drones',
          'protocol': protocol,
          'durationMs': 180000,
          'positionMs': 22000,
          'volumePercent': 68,
          'streamActive': true,
          'coverArt': art,
          'lyrics': {
            'synced': true,
            'lines': [
              {'startMs': 0, 'text': 'The night is calling'},
              {'startMs': 20000, 'text': 'With you by my side'},
            ],
          },
        },
      });
      tester.view.physicalSize = const Size(980, 420);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final key = GlobalKey();
      await tester.pumpWidget(const MaterialApp(home: SizedBox()));
      final context = tester.element(find.byType(SizedBox).last);
      await tester.runAsync(
        () => precacheImage(
          artworkProvider(source)!,
          context,
          onError: (e, st) {
            throw e;
          },
        ),
      );
      Widget home(String art, {String? protocol}) => MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: fusionTheme(),
        home: RepaintBoundary(
          key: key,
          child: FusionPlayHome(
            controller: controller,
            state: state(art, protocol ?? item.$1),
            isAndroid: true,
          ),
        ),
      );
      await tester.pumpWidget(home(source));
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 400)),
      );
      await tester.pump(const Duration(seconds: 1));
      // Let the prepared cover finish its background crossfade.
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.text('Starlight'), findsOneWidget);
      expect(find.text(item.$2), findsWidgets);
      expect(
        tester.widget<ArtworkCard>(find.byType(ArtworkCard)).source,
        source,
      );
      expect(find.text('With you by my side'), findsOneWidget);
      final output = Platform.environment['FUSIONPLAY_METADATA_PREVIEW_DIR'];
      if (output != null) {
        Directory(output).createSync(recursive: true);
        final boundary =
            key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
        await tester.runAsync(() async {
          final image = await boundary.toImage();
          final data = await image.toByteData(format: ui.ImageByteFormat.png);
          File(
            '$output/receiver-preview-${item.$1}.png',
          ).writeAsBytesSync(data!.buffer.asUint8List());
          image.dispose();
        });
      }
      await tester.pumpWidget(home('data:image/png;base64,AAAA'));
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 100)),
      );
      await tester.pump(const Duration(seconds: 1));
      expect(
        tester.widget<ArtworkCard>(find.byType(ArtworkCard)).source,
        source,
      );
      if (item.$1 == 'airplay') {
        const invalidCover = 'data:image/png;base64,@@@';
        await tester.pumpWidget(home(invalidCover, protocol: 'dlna'));
        await tester.pump(const Duration(seconds: 1));
        expect(
          tester.widget<ArtworkCard>(find.byType(ArtworkCard)).source,
          isNull,
        );
        expect(
          tester.widget<PlayerBackdrop>(find.byType(PlayerBackdrop)).source,
          isNull,
        );
        await tester.pumpWidget(home(source, protocol: 'dlna'));
        await tester.pump(const Duration(milliseconds: 350));
        expect(
          tester.widget<ArtworkCard>(find.byType(ArtworkCard)).source,
          source,
        );
      }
      expect(tester.takeException(), isNull);
    });
  }
}
