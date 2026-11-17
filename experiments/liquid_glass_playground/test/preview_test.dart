import 'dart:io';
import 'dart:ui' as ui;
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:liquid_glass_playground/main.dart';

Future<void> _capture(
  WidgetTester tester,
  GlassScheme scheme,
  PlaygroundBackground background,
  String name, {
  bool lyrics = true,
  bool dynamic = false,
  int waitMs = 500,
}) async {
  tester.view.physicalSize = const Size(1280, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final key = GlobalKey();
  await tester.pumpWidget(
    RepaintBoundary(
      key: key,
      child: GlassPlaygroundApp(
        initialScheme: scheme,
        initialBackground: background,
        initialLyrics: lyrics,
        initialDynamic: dynamic,
      ),
    ),
  );
  await tester.pump();
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 500)),
  );
  await tester.pump(Duration(milliseconds: waitMs));
  final boundary =
      key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage(pixelRatio: 2));
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory('/tmp/fusionplay-glass-playground').createSync(recursive: true);
  File(
    '/tmp/fusionplay-glass-playground/$name.png',
  ).writeAsBytesSync(data!.buffer.asUint8List());
}

void main() {
  setUpAll(() async {
    Future<ByteData> bytes(String path) async =>
        ByteData.sublistView(File(path).readAsBytesSync());
    final loader = FontLoader('MiSans')
      ..addFont(bytes('../../flutter/assets/fonts/MiSansUI.ttf'));
    await loader.load();
    final icons = FontLoader('MaterialIcons')
      ..addFont(
        bytes(
          '/vol1/@appdata/deepseek.harness/home/.cache/flutter-sdk/flutter/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf',
        ),
      );
    await icons.load();
  });

  testWidgets('render glass scheme comparison previews', (tester) async {
    final previousOnError = FlutterError.onError;
    FlutterError.onError = (details) {
      final text = details.exceptionAsString();
      if (text.contains('deactivated widget')) {
        debugPrint('GLASS_PLAYGROUND_WARN: ${details.exceptionAsString()}');
        return;
      }
      previousOnError?.call(details);
    };
    addTearDown(() => FlutterError.onError = previousOnError);
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.grid,
      'custom-grid',
    );
    await _capture(
      tester,
      GlassScheme.widgets,
      PlaygroundBackground.grid,
      'widgets-grid',
    );
    await _capture(
      tester,
      GlassScheme.renderer,
      PlaygroundBackground.grid,
      'renderer-grid',
    );
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.cover,
      'custom-cover',
    );
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.light,
      'custom-light',
    );
    await _capture(
      tester,
      GlassScheme.widgets,
      PlaygroundBackground.cover,
      'widgets-cover-2',
    );
    await _capture(
      tester,
      GlassScheme.renderer,
      PlaygroundBackground.cover,
      'renderer-cover',
    );
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.grid,
      'custom-no-lyrics',
      lyrics: false,
    );
    await _capture(
      tester,
      GlassScheme.widgets,
      PlaygroundBackground.light,
      'widgets-light',
    );
    await _capture(
      tester,
      GlassScheme.widgets,
      PlaygroundBackground.cover,
      'widgets-cover',
    );
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.grid,
      'custom-dynamic-a',
      dynamic: true,
      waitMs: 320,
    );
    await _capture(
      tester,
      GlassScheme.custom,
      PlaygroundBackground.grid,
      'custom-dynamic-b',
      dynamic: true,
      waitMs: 1200,
    );
  });
}
