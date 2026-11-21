import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/app_menu.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/glass_appearance.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';
import 'package:fusionplay_android_flutter/ui/settings_view.dart';

final String _coverData =
    'data:image/png;base64,${base64Encode(File('../fusionplay-ui-audit-codex/assets/cover.png').readAsBytesSync())}';

Widget _backdrop() => Stack(
  fit: StackFit.expand,
  children: [
    ArtworkBackground(source: _coverData, hasPlayback: true),
    const ColoredBox(color: Color(0x4D000000)),
  ],
);

Widget _host(Widget child, GlobalKey boundaryKey) => MaterialApp(
  debugShowCheckedModeBanner: false,
  theme: fusionTheme(),
  home: RepaintBoundary(
    key: boundaryKey,
    child: FrostedScene(
      background: _backdrop(),
      child: Scaffold(backgroundColor: Colors.transparent, body: child),
    ),
  ),
);

void main() {
  setUpAll(
    () => Directory('/tmp/fusionplay-preview').createSync(recursive: true),
  );
  setUpAll(() async {
    final loader = FontLoader('MiSans')
      ..addFont(rootBundle.load('assets/fonts/MiSansUI.ttf'));
    await loader.load();
    final icons = FontLoader('MaterialIcons')
      ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
    await icons.load();
  });

  testWidgets('render settings/menu/dialog gallery', (tester) async {
    tester.view.physicalSize = const Size(1280, 582);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final settingsKey = GlobalKey();
    await tester.pumpWidget(
      _host(
        Align(
          alignment: Alignment.centerRight,
          child: SizedBox(
            width: 500,
            height: 520,
            child: FrostedGlass(
              level: GlassLevel.panel,
              borderRadius: 24,
              tint: const Color(0x8814191F),

              child: SettingsView(
                state: const AppState(initialized: true),
                showHeader: false,
                initialFocusNode: FocusNode(),
                onClose: () {},
                onReceiverName: (_) {},
                onStartup: (_) {},
                onAutoWake: (_) {},
                onAdvancedEffects: (_) {},
                onMiPlay: (_) {},
                onMiPlayIdentity: (_) {},
                onAirPlay: (_) {},
                onDlna: (_) {},
                onExportLogs: () async {},
                isAndroid: true,
              ),
            ),
          ),
        ),
        settingsKey,
      ),
    );
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 250)),
    );
    await tester.pump(const Duration(milliseconds: 120));
    final boundary =
        settingsKey.currentContext!.findRenderObject()!
            as RenderRepaintBoundary;
    final image = await tester.runAsync(() => boundary.toImage(pixelRatio: 2));
    final data = await tester.runAsync(
      () => image!.toByteData(format: ui.ImageByteFormat.png),
    );
    File(
      '/tmp/fusionplay-preview/settings-21x9.png',
    ).writeAsBytesSync(data!.buffer.asUint8List());

    final menuKey = GlobalKey();
    await tester.pumpWidget(
      _host(
        Align(
          alignment: Alignment.topRight,
          child: Padding(
            padding: const EdgeInsets.all(20),
            child: SizedBox(
              width: 250,
              child: FrostedGlass(
                level: GlassLevel.panel,
                borderRadius: 22,
                tint: const Color(0x7014171C),

                child: FusionAppMenu(
                  items: [
                    FusionMenuItem(
                      icon: const Icon(Icons.settings_rounded),
                      label: '设置',
                      onSelected: () {},
                    ),
                    FusionMenuItem(
                      icon: const Icon(Icons.system_update_rounded),
                      label: '检查更新',
                      onSelected: () {},
                    ),
                    FusionMenuItem(
                      icon: const Icon(Icons.description_rounded),
                      label: '导出日志',
                      onSelected: () {},
                    ),
                    FusionMenuItem(
                      icon: const Icon(Icons.info_rounded),
                      label: '关于',
                      onSelected: () {},
                    ),
                  ],
                  focusNodes: List.generate(4, (_) => FocusNode()),
                ),
              ),
            ),
          ),
        ),
        menuKey,
      ),
    );
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 250)),
    );
    await tester.pump(const Duration(milliseconds: 120));
    final menuBoundary =
        menuKey.currentContext!.findRenderObject()! as RenderRepaintBoundary;
    final menuImage = await tester.runAsync(
      () => menuBoundary.toImage(pixelRatio: 2),
    );
    final menuData = await tester.runAsync(
      () => menuImage!.toByteData(format: ui.ImageByteFormat.png),
    );
    File(
      '/tmp/fusionplay-preview/menu-21x9.png',
    ).writeAsBytesSync(menuData!.buffer.asUint8List());

    final dialogKey = GlobalKey();
    await tester.pumpWidget(
      _host(
        Center(
          child: SizedBox(
            width: 420,
            child: GlassDialog(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: const [
                  Text(
                    '关于 FusionPlay',
                    style: TextStyle(fontSize: 18, color: Colors.white),
                  ),
                  SizedBox(height: 12),
                  Text(
                    '版本 1.7.0 · AirPlay 2\n协议：AirPlay 2\n设备名：FusionPlay',
                    style: TextStyle(fontSize: 14, color: Color(0xB8FFFFFF)),
                  ),
                ],
              ),
            ),
          ),
        ),
        dialogKey,
      ),
    );
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 250)),
    );
    await tester.pump(const Duration(milliseconds: 120));
    final dialogBoundary =
        dialogKey.currentContext!.findRenderObject()! as RenderRepaintBoundary;
    final dialogImage = await tester.runAsync(
      () => dialogBoundary.toImage(pixelRatio: 2),
    );
    final dialogData = await tester.runAsync(
      () => dialogImage!.toByteData(format: ui.ImageByteFormat.png),
    );
    File(
      '/tmp/fusionplay-preview/dialog-21x9.png',
    ).writeAsBytesSync(dialogData!.buffer.asUint8List());
  });
}
