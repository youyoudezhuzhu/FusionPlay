import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/glass_appearance.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';

class _Stripes extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawColor(const Color(0xff192b45), BlendMode.src);
    final paint = Paint()..color = const Color(0xffbbdbef);
    for (double x = 0; x < size.width; x += 12) {
      canvas.drawRect(Rect.fromLTWH(x, 0, 5, size.height), paint);
    }
  }

  @override
  bool shouldRepaint(_Stripes oldDelegate) => false;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async {
    for (final (family, asset) in [
      ('MiSans', 'assets/fonts/MiSansUI.ttf'),
      ('MaterialIcons', 'fonts/MaterialIcons-Regular.otf'),
    ]) {
      await (FontLoader(family)..addFont(rootBundle.load(asset))).load();
    }
  });
  for (final stripes in [true, false]) {
    final scene = stripes ? 'stripes' : 'cover';
    testWidgets('solid volume thumb tracks a drag on $scene and commits once', (
      tester,
    ) async {
      final key = GlobalKey();
      final commits = <int>[];
      const width = 900;
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(900, 400);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final photo = File(
        '../fusionplay-ui-audit-codex/cover-study/alpenglow.jpg',
      ).absolute.path;
      if (!stripes) {
        await tester.pumpWidget(const MaterialApp(home: SizedBox()));
        await tester.runAsync(
          () => precacheImage(
            artworkProvider(photo)!,
            tester.element(find.byType(SizedBox).last),
          ),
        );
      }
      await tester.pumpWidget(
        MaterialApp(
          theme: fusionTheme(),
          debugShowCheckedModeBanner: false,
          home: Scaffold(
            body: RepaintBoundary(
              key: key,
              child: FrostedScene(
                background: stripes
                    ? CustomPaint(painter: _Stripes())
                    : PlayerBackdrop(source: photo, hasPlayback: true),
                child: PlayerView(
                  artworkSource: stripes ? null : photo,
                  state: const AppState(
                    initialized: true,
                    controls: PlaybackControls.all,
                    playback: PlaybackSnapshot(
                      protocol: 'dlna',
                      title: 'Starlight',
                      artist: 'Muse · Drones',
                      durationMs: 200000,
                      volumePercent: 25,
                    ),
                  ),
                  onVolume: commits.add,
                  onPlayback: (_) {},
                  onSeek: (_) {},
                ),
              ),
            ),
          ),
        ),
      );
      final slider = find.byKey(const ValueKey('volume-slider'));
      final capsule = find.byKey(const ValueKey('player-volume-capsule'));
      for (int i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 120)),
        );
      }
      expect(
        find.descendant(of: capsule, matching: find.byType(FrostedGlass)),
        findsOneWidget,
      );
      expect(
        find.descendant(of: slider, matching: find.byType(FrostedGlass)),
        findsNothing,
      );
      final bounds = tester.getRect(slider);
      final dir = Platform.environment['FUSIONPLAY_CAPSULE_PREVIEW'];
      Future<Uint8List> capture(int frame) async {
        await tester.pump(const Duration(milliseconds: 120));
        final boundary =
            key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
        return (await tester.runAsync(() async {
          final image = await boundary.toImage();
          try {
            if (dir != null) {
              Directory(dir).createSync(recursive: true);
              final data = await image.toByteData(
                format: ui.ImageByteFormat.png,
              );
              File(
                '$dir/$scene-$frame.png',
              ).writeAsBytesSync(data!.buffer.asUint8List());
              final r = tester.getRect(
                find.byKey(const ValueKey('player-volume-capsule')),
              );
              File('$dir/$scene-capsule.txt').writeAsStringSync(
                '${r.left.floor()} ${r.top.floor()} ${r.right.ceil()} ${r.bottom.ceil()}',
              );
            }
            return (await image.toByteData(
              format: ui.ImageByteFormat.rawRgba,
            ))!.buffer.asUint8List();
          } finally {
            image.dispose();
          }
        }))!;
      }

      Offset point(double t) =>
          Offset(bounds.left + 10 + (bounds.width - 20) * t, bounds.center.dy);
      final gesture = await tester.startGesture(point(.25));
      for (final value in [.36, .72]) {
        await gesture.moveTo(point(value));
        await tester.pump();
        final pixels = await capture((value * 100).round());
        final center = point(value);
        // Probe both sides of the track centreline inside the solid thumb.
        for (final offset in [const Offset(-3, -4), const Offset(3, 4)]) {
          final sample = center + offset;
          final index = (sample.dy.round() * width + sample.dx.round()) * 4;
          expect(pixels.sublist(index, index + 4), [255, 255, 255, 255]);
        }
      }
      expect(commits, isEmpty);
      await gesture.up();
      await tester.pump(const Duration(milliseconds: 200));
      expect(commits, [72]);
      await tester.pumpWidget(const SizedBox());
      await tester.pump();
    });
  }
}
