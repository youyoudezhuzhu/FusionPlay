import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/glass_backdrop.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';

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
    testWidgets('volume lens tracks a drag on $scene and commits once', (
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
              child: GlassBackdropScope(
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
      final lens = find.byKey(const ValueKey('volume-droplet'));
      for (int i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 120)),
        );
      }
      expect(
        find.descendant(of: lens, matching: find.byType(BackdropFilter)),
        findsNothing,
        reason: 'Wait for the droplet shader before measuring its image',
      );
      final bounds = tester.getRect(slider);
      final dir = Platform.environment['FUSIONPLAY_LENS_PREVIEW'];
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
          Offset(bounds.left + 14 + (bounds.width - 28) * t, bounds.center.dy);
      final gesture = await tester.startGesture(tester.getCenter(lens));
      await gesture.moveTo(point(.36));
      await tester.pump();
      final first = await capture(0);
      final firstCenter = tester.getCenter(lens);
      final steps = dir == null ? 10 : 24;
      for (int i = 1; i <= steps; i++) {
        await gesture.moveTo(point(.36 + .36 * i / steps));
        await tester.pump();
        if (dir != null && i < steps) await capture(i);
      }
      final last = await capture(steps);
      final lastCenter = tester.getCenter(lens);
      expect(lastCenter.dx - firstCenter.dx, greaterThan(60));
      expect((lastCenter - point(.72)).distance, lessThan(1));
      // Count bright-to-dark reversals through the dome, above and below the rail.
      // Adding white tint or a moving highlight cannot turn a bright stripe dark.
      int displaced(Uint8List throughLens, Uint8List clear, Offset center) {
        var count = 0;
        for (var y = -10; y <= 10; y++) {
          if (y.abs() < 5) continue;
          for (var x = -13; x <= 13; x++) {
            if (x * x / (14 * 14) + y * y / (11 * 11) >= 1) continue;
            final idx =
                ((center.dy.round() + y) * width + center.dx.round() + x) * 4;
            if (clear[idx] - throughLens[idx] > 45) count++;
          }
        }
        return count;
      }

      if (stripes) {
        expect(displaced(first, last, firstCenter), greaterThan(5));
        expect(displaced(last, first, lastCenter), greaterThan(5));
      }
      expect(commits, isEmpty);
      await gesture.up();
      await tester.pump(const Duration(milliseconds: 200));
      expect(commits, [72]);
      if (dir != null) await capture(steps + 1);
      await tester.pumpWidget(const SizedBox());
      await tester.pump();
    });
  }
}
