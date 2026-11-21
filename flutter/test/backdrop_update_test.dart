import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/glass_backdrop.dart';

void main() {
  testWidgets('background repaint updates an offset texture without revision', (
    tester,
  ) async {
    final color = ValueNotifier<Color>(Colors.red);
    final captures = <GlassBackdropTexture>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: SizedBox(
            width: 80,
            height: 40,
            child: GlassBackdropScope(
              captureInterval: Duration.zero,
              onTextureChanged: (value) {
                if (value != null) captures.add(value);
              },
              background: ValueListenableBuilder<Color>(
                valueListenable: color,
                builder: (_, value, _) => ColoredBox(color: value),
              ),
              child: const SizedBox.expand(),
            ),
          ),
        ),
      ),
    );
    Future<void> waitForCapture(int count) async {
      await tester.runAsync(() async {
        for (var i = 0; i < 100 && captures.length < count; i++) {
          await Future<void>.delayed(const Duration(milliseconds: 10));
        }
      });
      await tester.pump();
      expect(captures.length, count);
    }

    await waitForCapture(1);
    expect(captures.single.origin, const Offset(360, 280));
    color.value = Colors.blue;
    await tester.pump();
    await waitForCapture(2);
    final bytes = await tester.runAsync(
      () => captures.last.image.toByteData(format: ui.ImageByteFormat.rawRgba),
    );
    expect(bytes!.getUint8(2), Colors.blue.toARGB32() & 255);
    await tester.pump(const Duration(seconds: 1));
    expect(captures.length, 2);
  });
}
