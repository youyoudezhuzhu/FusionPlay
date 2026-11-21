import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/glass_backdrop.dart';
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart';

class _GridBackdrop extends StatelessWidget {
  const _GridBackdrop();

  @override
  Widget build(BuildContext context) =>
      CustomPaint(painter: _GridPainter(), child: const SizedBox.expand());
}

class _GridPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final background = Paint()..color = const Color(0xFF0C1420);
    canvas.drawRect(Offset.zero & size, background);
    final minor = Paint()
      ..color = const Color(0xFF4A607A)
      ..strokeWidth = 1;
    final major = Paint()
      ..color = const Color(0xFFA9D8FF)
      ..strokeWidth = 2;
    for (var x = 0.0; x < size.width; x += 24) {
      canvas.drawLine(
        Offset(x, 0),
        Offset(x, size.height),
        x % 96 == 0 ? major : minor,
      );
    }
    for (var y = 0.0; y < size.height; y += 24) {
      canvas.drawLine(
        Offset(0, y),
        Offset(size.width, y),
        y % 96 == 0 ? major : minor,
      );
    }
    canvas.drawCircle(
      Offset(size.width * .38, size.height * .42),
      64,
      Paint()..color = const Color(0xFFE05EFF),
    );
    canvas.drawRect(
      Rect.fromLTWH(size.width * .62, size.height * .18, 110, 110),
      Paint()..color = const Color(0xFF28F0B0),
    );
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

Widget _host({
  double? refraction,
  bool interactive = false,
  required GlobalKey boundaryKey,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    home: RepaintBoundary(
      key: boundaryKey,
      child: GlassBackdropScope(
        revision: 'optics',
        background: const _GridBackdrop(),
        child: Center(
          child: SizedBox(
            width: 314,
            height: 230,
            child: LiquidGlass(
              level: GlassLevel.control,
              borderRadius: 72,
              refraction: refraction,
              interactive: interactive,
              specular: interactive ? 0 : null,
              child: const SizedBox.expand(),
            ),
          ),
        ),
      ),
    ),
  );
}

Future<Uint8List> _capturePng(WidgetTester tester, GlobalKey key) async {
  await tester.pump();
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 350)),
  );
  await tester.pump(const Duration(milliseconds: 150));
  final boundary =
      key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage(pixelRatio: 2));
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  image!.dispose();
  return data!.buffer.asUint8List();
}

Future<Uint8List> _captureRgba(WidgetTester tester, GlobalKey key) async {
  await tester.pump();
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 350)),
  );
  await tester.pump(const Duration(milliseconds: 150));
  final boundary =
      key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage(pixelRatio: 2));
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.rawRgba),
  );
  image!.dispose();
  return data!.buffer.asUint8List();
}

double _meanRegionDiff(Uint8List a, Uint8List b, int width) {
  var total = 0.0;
  var count = 0;
  // The glass is centered at 640x420 logical and captured at 2x: sample the
  // 14 logical pixels inside the straight left edge of the 314-wide surface.
  for (var y = 300; y < 560; y += 3) {
    for (var x = 326; x < 354; x++) {
      final index = (y * width + x) * 4;
      if (index + 2 >= a.length || index + 2 >= b.length) continue;
      total +=
          (a[index] - b[index]).abs() +
          (a[index + 1] - b[index + 1]).abs() +
          (a[index + 2] - b[index + 2]).abs();
      count += 3;
    }
  }
  return count == 0 ? 0 : total / count;
}

void main() {
  setUpAll(
    () => Directory('/tmp/fusionplay-preview').createSync(recursive: true),
  );
  testWidgets('glass panel transmits layers outside the artwork capture', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(640, 420);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final boundary = GlobalKey();
    final lowerLayer = ValueNotifier<Color>(Colors.red);
    addTearDown(lowerLayer.dispose);

    await tester.pumpWidget(
      MaterialApp(
        home: RepaintBoundary(
          key: boundary,
          child: GlassBackdropScope(
            background: const ColoredBox(color: Colors.black),
            child: Stack(
              fit: StackFit.expand,
              children: [
                ValueListenableBuilder<Color>(
                  valueListenable: lowerLayer,
                  builder: (_, color, _) => ColoredBox(color: color),
                ),
                const Center(
                  child: SizedBox(
                    width: 240,
                    height: 120,
                    child: LiquidGlass(
                      level: GlassLevel.panel,
                      blurSigma: 9,
                      child: SizedBox.expand(),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );

    final red = await _captureRgba(tester, boundary);
    lowerLayer.value = Colors.blue;
    final blue = await _captureRgba(tester, boundary);
    final center = (420 * 1280 + 640) * 4;
    expect(red[center], greaterThan(blue[center] + 35));
    expect(blue[center + 2], greaterThan(red[center + 2] + 35));
    expect(tester.takeException(), isNull);
  });

  testWidgets('pressing clear glass bends the scene and release restores it', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(640, 420);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final key = GlobalKey();
    await tester.pumpWidget(_host(boundaryKey: key, interactive: true));
    final idle = await _captureRgba(tester, key);
    final finger = await tester.startGesture(const Offset(327, 215));
    final pressed = await _captureRgba(tester, key);
    await finger.up();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    final released = await _captureRgba(tester, key);
    // At 2x the shader's 56 logical pixel contact radius spans 112 pixels.
    int darkened = 0;
    int outside = 0;
    for (int y = 0; y < 840; y++) {
      for (int x = 0; x < 1280; x++) {
        final index = (y * 1280 + x) * 4;
        if (idle[index] - pressed[index] <= 40) continue;
        final dx = x - 654, dy = y - 430;
        if (dx * dx + dy * dy < 114 * 114) {
          darkened++;
        } else {
          outside++;
        }
      }
    }
    expect(darkened, greaterThan(100));
    expect(outside, 0);
    expect(listEquals(released, idle), isTrue);
  });
  testWidgets('real liquid glass refracts a high-contrast backdrop', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(640, 420);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final refractedKey = GlobalKey();
    await tester.pumpWidget(_host(boundaryKey: refractedKey));
    final refractedPng = await _capturePng(tester, refractedKey);
    final refractedPixels = await _captureRgba(tester, refractedKey);
    File(
      '/tmp/fusionplay-preview/glass-optics.png',
    ).writeAsBytesSync(refractedPng);

    final flatKey = GlobalKey();
    await tester.pumpWidget(_host(refraction: 0, boundaryKey: flatKey));
    final flatPng = await _capturePng(tester, flatKey);
    final flatPixels = await _captureRgba(tester, flatKey);
    File(
      '/tmp/fusionplay-preview/glass-optics-flat.png',
    ).writeAsBytesSync(flatPng);

    expect(refractedPng.length, greaterThan(1000));
    expect(flatPng.length, greaterThan(1000));
    final diff = _meanRegionDiff(refractedPixels, flatPixels, 1280);
    expect(
      diff,
      greaterThan(4.0),
      reason: 'shader must displace the sampled backdrop, not just tint it',
    );
  });
}
