import 'dart:async';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/glass_appearance.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';
import 'package:fusionplay_android_flutter/ui/update_dialogs.dart';

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

void main() {
  testWidgets('frosted panel transmits changes to the live lower layer', (
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
          child: FrostedScene(
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
                    child: FrostedGlass(
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

  testWidgets('checking for updates frosts and transmits its lower route', (
    tester,
  ) async {
    final checking = Completer<void>();
    final lowerLayer = ValueNotifier<Color>(Colors.red);
    addTearDown(lowerLayer.dispose);
    final boundary = GlobalKey();
    tester.view.physicalSize = const Size(640, 420);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      RepaintBoundary(
        key: boundary,
        child: MaterialApp(
          home: FrostedScene(
            background: const _GridBackdrop(),
            child: ValueListenableBuilder<Color>(
              valueListenable: lowerLayer,
              builder: (_, color, _) => Scaffold(
                backgroundColor: color,
                body: Builder(
                  builder: (context) => TextButton(
                    onPressed: () => showBusyDialog<void>(
                      context: context,
                      message: '正在检查更新…',
                      task: () => checking.future,
                    ),
                    child: const Text('打开更新'),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await _captureRgba(tester, boundary);
    await tester.tap(find.text('打开更新'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    final red = await _captureRgba(tester, boundary);
    final dialog = find.byKey(const ValueKey('update-busy-dialog'));
    expect(
      find.descendant(of: dialog, matching: find.byType(BackdropFilter)),
      findsOneWidget,
    );

    lowerLayer.value = Colors.blue;
    final blue = await _captureRgba(tester, boundary);
    // Sample the empty central area just inside the top padding.
    final pixel = ((210 - 21) * 2 * 1280 + 340 * 2) * 4;
    expect(red[pixel], greaterThan(blue[pixel] + 30));
    expect(blue[pixel + 2], greaterThan(red[pixel + 2] + 30));

    checking.complete();
    await tester.pumpAndSettle();
    expect(dialog, findsNothing);
  });
}
