import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';

class _ContrastController extends NativeAppController {
  @override
  Future<UpdateCheckResult> checkForUpdate() async => const UpdateCheckResult(
    status: UpdateCheckStatus.available,
    versionName: '测试',
    notes: '更新说明',
  );
}

double contrast(double a, double b) =>
    (math.max(a, b) + .05) / (math.min(a, b) + .05);

Future<String?> cover(String scene) async {
  if (scene == 'missing') return null;
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  canvas.drawColor(
    scene == 'black' ? Colors.black : Colors.white,
    BlendMode.src,
  );
  if (scene == 'split') {
    canvas.drawRect(
      const Rect.fromLTWH(0, 0, 32, 64),
      Paint()..color = Colors.black,
    );
  }
  final picture = recorder.endRecording();
  final image = await picture.toImage(64, 64);
  final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  picture.dispose();
  return 'data:image/png;base64,${base64Encode(bytes!.buffer.asUint8List())}';
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async {
    for (final (family, file) in [
      ('MiSans', 'assets/fonts/MiSansUI.ttf'),
      ('MaterialIcons', 'fonts/MaterialIcons-Regular.otf'),
    ]) {
      await (FontLoader(family)..addFont(rootBundle.load(file))).load();
    }
  });
  for (final scene in ['white', 'black', 'split', 'missing']) {
    for (final effects in [true, false]) {
      testWidgets(
        '$scene cover, effects=$effects keeps player captions readable',
        (tester) async {
          const size = Size(980, 420);
          tester.view.devicePixelRatio = 1;
          tester.view.physicalSize = size;
          addTearDown(tester.view.resetPhysicalSize);
          addTearDown(tester.view.resetDevicePixelRatio);
          final photo = await tester.runAsync(() => cover(scene));
          final controller = _ContrastController();
          addTearDown(controller.dispose);
          await tester.pumpWidget(const MaterialApp(home: SizedBox()));
          if (photo != null) {
            await tester.runAsync(
              () => precacheImage(
                artworkProvider(photo)!,
                tester.element(find.byType(SizedBox).last),
              ),
            );
          }
          final key = GlobalKey();
          Widget app({LyricsDocument? lyrics, bool idle = false}) =>
              RepaintBoundary(
                key: key,
                child: MaterialApp(
                  debugShowCheckedModeBanner: false,
                  theme: fusionTheme(),
                  home: FusionPlayHome(
                    controller: controller,
                    isAndroid: true,
                    state: AppState(
                      initialized: true,
                      controls: PlaybackControls.all,
                      settings: AppSettings(advancedEffectsEnabled: effects),
                      playback: idle
                          ? const PlaybackSnapshot()
                          : PlaybackSnapshot(
                              lyrics: lyrics,
                              title: 'Starlight',
                              artist: 'Muse',
                              coverArt: photo,
                              durationMs: 210000,
                              positionMs: 84000,
                              volumePercent: 68,
                              protocol: 'dlna',
                              streamActive: true,
                            ),
                    ),
                  ),
                ),
              );
          await tester.pumpWidget(app());
          Future<void> settle() async {
            for (int i = 0; i < 12; i++) {
              await tester.pump(const Duration(milliseconds: 200));
              await tester.runAsync(
                () => Future<void>.delayed(const Duration(milliseconds: 28)),
              );
            }
          }

          final dir = Platform.environment['FUSIONPLAY_CONTRAST_PREVIEW'];
          late Uint8List pixels;
          Future<void> capture(String screen) async {
            await settle();
            final boundary =
                key.currentContext!.findRenderObject()!
                    as RenderRepaintBoundary;
            pixels = (await tester.runAsync(() async {
              final image = await boundary.toImage();
              try {
                if (dir != null) {
                  Directory(dir).createSync(recursive: true);
                  final png = await image.toByteData(
                    format: ui.ImageByteFormat.png,
                  );
                  File(
                    '$dir/$scene-$effects$screen.png',
                  ).writeAsBytesSync(png!.buffer.asUint8List());
                }
                return (await image.toByteData(
                  format: ui.ImageByteFormat.rawRgba,
                ))!.buffer.asUint8List();
              } finally {
                image.dispose();
              }
            }))!;
          }

          await capture('');
          double luminance(int x, int y) {
            final i = (y * size.width.toInt() + x) * 4;
            return Color.fromARGB(
              255,
              pixels[i],
              pixels[i + 1],
              pixels[i + 2],
            ).computeLuminance();
          }

          final art = tester.getRect(find.byType(ArtworkCard)).center;
          if (scene == 'white') {
            expect(luminance(art.dx.round(), art.dy.round()), greaterThan(.97));
          }
          if (scene == 'black') {
            expect(luminance(art.dx.round(), art.dy.round()), lessThan(.03));
          }
          final ratios = <String, double>{};
          void probe(String label, {bool darkInk = false}) {
            final finder = find.text(label).last;
            final paragraph = tester.renderObject<RenderParagraph>(finder);
            final box = paragraph
                .getBoxesForSelection(
                  TextSelection(baseOffset: 0, extentOffset: label.length),
                )
                .first
                .toRect();
            final rect = box.shift(paragraph.localToGlobal(Offset.zero));
            final ink = <double>[], under = <double>[];
            for (int x = rect.left.floor(); x < rect.right.ceil(); x++) {
              under.add(luminance(x, rect.top.floor() - 1));
              for (int y = rect.top.floor(); y < rect.bottom.ceil(); y++) {
                ink.add(luminance(x, y));
              }
            }
            ink.sort();
            under.sort();
            ratios[label] = contrast(
              ink[((darkInk ? .01 : .99) * (ink.length - 1)).round()],
              under[under.length ~/ 2],
            );
          }

          for (final label in [
            'Starlight',
            'Muse',
            '1:24',
            '-2:06',
            '68',
            'DLNA',
          ]) {
            probe(label);
          }
          for (final key in ['player-progress-bar', 'volume-slider']) {
            final rect = tester.getRect(find.byKey(ValueKey(key)));
            final x = (rect.left + .8 * rect.width).round(),
                y = rect.center.dy.round();
            ratios[key] = contrast(luminance(x, y), luminance(x, y - 7));
          }
          final graphics = <String>{'player-progress-bar', 'volume-slider'};
          void symbol(String label, Rect bounds) {
            final values = <double>[];
            final area = Rect.fromCenter(
              center: bounds.center,
              width: bounds.width * .42,
              height: bounds.height * .42,
            );
            for (var x = area.left.ceil(); x < area.right.floor(); x++) {
              for (var y = area.top.ceil(); y < area.bottom.floor(); y++) {
                values.add(luminance(x, y));
              }
            }
            values.sort();
            ratios[label] = contrast(
              values[(values.length * .02).floor()],
              values[(values.length * .98).floor()],
            );
            graphics.add(label);
          }

          for (final id in [
            'playback-previous-button',
            'playback-play-pause-button',
            'playback-next-button',
          ]) {
            symbol(id, tester.getRect(find.byKey(ValueKey(id))));
          }
          if (scene == 'missing') {
            symbol(
              'missing-art-note',
              tester.getRect(find.byType(ArtworkCard)),
            );
            await tester.pumpWidget(app(idle: true));
            await capture('-idle');
            symbol('idle-art-note', tester.getRect(find.byType(ArtworkCard)));
          }
          if (scene == 'white' || scene == 'black') {
            final menu = find.byKey(const ValueKey('playback-menu-button'));
            await tester.tap(menu);
            await capture('-menu');
            probe('设置');
            probe('检查更新');
            await tester.tap(find.text('设置'));
            await capture('-settings');
            probe('自定义名称');
            await tester.tap(find.bySemanticsLabel('关闭设置'));
            await settle();
            await tester.tap(find.text('检查更新'));
            await capture('-update');
            probe('发现新版本 测试');
            probe('更新说明');
            probe('下载更新', darkInk: true);
            Navigator.pop(tester.element(find.text('下载更新')));
            await settle();
            await tester.tap(menu);
            await settle();
            await tester.tap(find.text('关于'));
            await capture('-about');
            probe('协议');
            probe('设备名');
            probe('好', darkInk: true);
            Navigator.pop(tester.element(find.text('好')));
            await settle();
            await tester.pumpWidget(
              app(
                lyrics: const LyricsDocument(
                  synced: true,
                  lines: [
                    LyricLine(startMs: 0, text: 'First verse'),
                    LyricLine(
                      startMs: 70000,
                      text: 'Current verse',
                      translation: '当前歌词',
                    ),
                    LyricLine(
                      startMs: 90000,
                      text: 'Following verse',
                      translation: '下一句歌词',
                    ),
                  ],
                ),
              ),
            );
            await capture('-lyrics');
            probe('Current verse');
            probe('当前歌词');
            probe('Following verse');
            probe('下一句歌词');
            for (final (line, translation) in [
              ('Stream one', '第一句译文'),
              ('Stream two', '第二句译文'),
            ]) {
              await tester.pumpWidget(
                app(
                  lyrics: LyricsDocument(
                    lines: [LyricLine(text: line, translation: translation)],
                  ),
                ),
              );
            }
            await capture('-stream');
            probe('Stream one');
            probe('第一句译文');
            probe('Stream two');
            probe('第二句译文');
          }
          if (dir != null) {
            File(
              '$dir/$scene-$effects.json',
            ).writeAsStringSync(jsonEncode(ratios));
          }
          for (final entry in ratios.entries) {
            final min = entry.key == 'Starlight' || graphics.contains(entry.key)
                ? 3.0
                : 4.5;
            expect(
              entry.value,
              greaterThanOrEqualTo(min),
              reason:
                  '$scene, effects=$effects, ${entry.key}: ${entry.value.toStringAsFixed(2)}',
            );
          }
        },
      );
    }
  }
}
