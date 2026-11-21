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
import 'package:fusionplay_android_flutter/ui/glass_appearance.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';

class _PreviewController extends NativeAppController {
  final effects = ValueNotifier(true);

  @override
  Future<void> setAdvancedEffectsEnabled(bool value) async =>
      effects.value = value;

  @override
  void dispose() {
    effects.dispose();
    super.dispose();
  }
}

void main() {
  testWidgets('advanced effects switch updates the open settings surface', (
    tester,
  ) async {
    final output = Platform.environment['FUSIONPLAY_EFFECTS_PREVIEW'];
    final key = GlobalKey();
    final controller = _PreviewController();
    addTearDown(controller.dispose);
    final cover = File(
      '../fusionplay-ui-audit-codex/cover-study/alpenglow.jpg',
    ).absolute.path;
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1000, 600);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final font = FontLoader('MiSans')
      ..addFont(rootBundle.load('assets/fonts/MiSansUI.ttf'));
    await font.load();
    final icons = FontLoader('MaterialIcons')
      ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
    await icons.load();
    await tester.pumpWidget(const MaterialApp(home: SizedBox()));
    await tester.runAsync(
      () => precacheImage(
        artworkProvider(cover)!,
        tester.element(find.byType(SizedBox).last),
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: fusionTheme(),
        home: RepaintBoundary(
          key: key,
          child: ValueListenableBuilder<bool>(
            valueListenable: controller.effects,
            builder: (_, enabled, _) => FusionPlayHome(
              isAndroid: true,
              controller: controller,
              state: AppState(
                initialized: true,
                receiverName: 'FusionPlay',
                settings: AppSettings(advancedEffectsEnabled: enabled),
                controls: PlaybackControls.all,
                playback: PlaybackSnapshot(
                  streamActive: true,
                  protocol: 'dlna',
                  title: 'Starlight',
                  artist: 'Muse · Drones',
                  coverArt: cover,
                  positionMs: 84000,
                  durationMs: 210000,
                ),
              ),
            ),
          ),
        ),
      ),
    );
    Future<void> settle() async {
      for (var i = 0; i < 3; i++) {
        await tester.pump(const Duration(milliseconds: 400));
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 80)),
        );
      }
      await tester.pump();
    }

    Future<void> capture(String name) async {
      await settle();
      if (output == null) return;
      final boundary =
          key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
      await tester.runAsync(() async {
        final image = await boundary.toImage();
        try {
          final data = await image.toByteData(format: ui.ImageByteFormat.png);
          Directory(output).createSync(recursive: true);
          await File(
            '$output/$name.png',
          ).writeAsBytes(data!.buffer.asUint8List());
        } finally {
          image.dispose();
        }
      });
    }

    await capture('player-on');
    await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
    await capture('menu-on');
    await tester.tap(find.text('设置'));
    await capture('settings-on');
    final sheet = find.byKey(const ValueKey('settings-sheet'));
    final panel = find.byKey(const ValueKey('morph-container'));
    final sheetElement = tester.element(sheet);
    expect(GlassAppearance.enabledOf(sheetElement), isTrue);
    final toggle = find.text('高级效果');
    await tester.ensureVisible(toggle);
    await tester.pumpAndSettle();
    final scroll = Scrollable.of(tester.element(toggle)).position;
    final offset = scroll.pixels;
    await tester.tap(toggle);
    await settle();
    expect(controller.effects.value, isFalse);
    expect(scroll.pixels, closeTo(offset, 1));
    scroll.jumpTo(0);
    await capture('settings-off');
    expect(tester.element(sheet), same(sheetElement));
    expect(GlassAppearance.enabledOf(sheetElement), isFalse);
    expect(
      find.byWidgetPredicate((w) => w is BackdropFilter && w.enabled),
      findsNothing,
    );
    expect(
      find.descendant(
        of: panel,
        matching: find.byWidgetPredicate(
          (w) =>
              w is ColoredBox &&
              w.color == GlassSpec.panel.solidColor &&
              w.color.a == 1,
        ),
      ),
      findsOneWidget,
    );
    await tester.tap(find.byIcon(Icons.close_rounded).first);
    await capture('menu-off');
    expect(sheet, findsNothing);
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);
    await tester.tapAt(const Offset(20, 300));
    await capture('player-off');
    expect(panel, findsNothing);
    expect(tester.takeException(), isNull);
  });
}
