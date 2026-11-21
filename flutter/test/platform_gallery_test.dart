// Run with FUSIONPLAY_PLATFORM_PREVIEW=/tmp/gallery to write screenshots.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/backend/windows/windows_app_controller.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';

const available = UpdateCheckResult(
  status: UpdateCheckStatus.available,
  versionName: '（预览）',
  releaseKey: 'preview',
  notes: '安装包已就绪，可选择下载。',
);

class _WindowsPreview extends WindowsAppController {
  @override
  Future<UpdateCheckResult> checkForUpdate() async => available;
}

class _AndroidPreview extends NativeAppController {
  @override
  Future<UpdateCheckResult> checkForUpdate() async => available;
}

void main() {
  setUpAll(() async {
    for (final (family, asset) in [
      ('MiSans', 'assets/fonts/MiSansUI.ttf'),
      ('MaterialIcons', 'fonts/MaterialIcons-Regular.otf'),
    ]) {
      final loader = FontLoader(family)..addFont(rootBundle.load(asset));
      await loader.load();
    }
  });
  TestWidgetsFlutterBinding.ensureInitialized();
  for (final android in [true, false]) {
    final host = android ? 'android' : 'windows';
    testWidgets('$host player, settings and platform update action', (
      tester,
    ) async {
      final controller = android ? _AndroidPreview() : _WindowsPreview();
      addTearDown(controller.dispose);
      final size = android ? const Size(410, 840) : const Size(1100, 680);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = size;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final key = GlobalKey();
      final photo = File(
        '../fusionplay-ui-audit-codex/cover-study/alpenglow.jpg',
      ).absolute.path;
      await tester.pumpWidget(const MaterialApp(home: SizedBox()));
      await tester.runAsync(
        () => precacheImage(
          artworkProvider(photo)!,
          tester.element(find.byType(SizedBox).last),
        ),
      );
      await tester.pumpWidget(
        RepaintBoundary(
          key: key,
          child: MaterialApp(
            theme: fusionTheme().copyWith(
              platform: android
                  ? TargetPlatform.android
                  : TargetPlatform.windows,
            ),
            debugShowCheckedModeBanner: false,
            home: FusionPlayHome(
              controller: controller,
              isAndroid: android,
              state: AppState(
                initialized: true,
                receiverName: '客厅',
                controls: PlaybackControls.all,
                settings: const AppSettings(
                  advancedEffectsEnabled: true,
                  receiverName: '客厅',
                ),
                playback: PlaybackSnapshot(
                  protocol: 'dlna',
                  title: 'Starlight',
                  artist: 'Muse · Drones',
                  coverArt: photo,
                  streamActive: true,
                  isPlaying: true,
                  durationMs: 210000,
                  positionMs: 84000,
                  volumePercent: 68,
                ),
              ),
            ),
          ),
        ),
      );
      Future<void> capture(String screen) async {
        // Allow real image and shader loading, then finish the menu morph.
        for (int i = 0; i < 10; i++) {
          await tester.pump(const Duration(milliseconds: 350));
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 70)),
          );
        }
        expect(tester.takeException(), isNull);
        final dir = Platform.environment['FUSIONPLAY_PLATFORM_PREVIEW'];
        if (dir == null) return;
        final boundary =
            key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
        await tester.runAsync(() async {
          final img = await boundary.toImage();
          try {
            final bytes = await img.toByteData(format: ui.ImageByteFormat.png);
            await Directory(dir).create(recursive: true);
            await File(
              '$dir/$host-$screen.png',
            ).writeAsBytes(bytes!.buffer.asUint8List());
          } finally {
            img.dispose();
          }
        });
      }

      await capture('player');
      if (android) {
        final art = tester.getRect(find.byType(ArtworkCard));
        final title = tester.getRect(find.text('Starlight'));
        expect(art.width, greaterThan(size.width * .65));
        expect(title.top - art.bottom, inInclusiveRange(0, 48));
      }
      await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
      await capture('menu');
      await tester.tap(find.text('设置'));
      await capture('settings');
      await tester.tap(find.bySemanticsLabel('关闭设置'));
      await capture('menu');
      await tester.tap(find.text('检查更新'));
      await capture('update');
      expect(find.text(android ? '下载更新' : '选择保存目录'), findsOneWidget);
      Navigator.pop(tester.element(find.text(android ? '下载更新' : '选择保存目录')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
      await capture('menu');
      await tester.tap(find.text('关于'));
      await capture('about');
      expect(find.byKey(const ValueKey('about-dialog')), findsOneWidget);
    });
  }
}
