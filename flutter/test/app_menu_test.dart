import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/native_app_controller.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart';
import 'package:fusionplay_android_flutter/ui/settings_view.dart';

const _runtimeChannel = MethodChannel('com.fusionplay.android/runtime');
const _updateChannel = MethodChannel(AndroidUpdateService.channelName);

final List<String> _runtimeMethods = <String>[];

void _mockRuntimeChannel() {
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_runtimeChannel, (call) async {
        _runtimeMethods.add(call.method);
        return null;
      });
}

void _clearChannels() {
  _runtimeMethods.clear();
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_runtimeChannel, null);
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_updateChannel, null);
}

AppState _state() => const AppState(
  initialized: true,
  receiverName: '客厅 FusionPlay',
  playback: PlaybackSnapshot(
    title: 'Track',
    artist: 'Artist',
    durationMs: 180000,
    streamActive: true,
    protocol: 'airplay',
    qualityText: 'AAC 44.1 kHz',
  ),
  remoteControl: RemoteControlState(
    available: true,
    commands: {'play_pause', 'previous_track', 'next_track'},
  ),
  controls: PlaybackControls.all,
);

Future<void> _pumpHome(
  WidgetTester tester, {
  required NativeAppController controller,
  bool isAndroid = false,
  Size size = const Size(1200, 600),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      theme: fusionTheme(),
      home: FusionPlayHome(
        controller: controller,
        state: _state(),
        isAndroid: isAndroid,
        forceGlassFallback: true,
      ),
    ),
  );
  await tester.pump();
}

Future<void> _openMenu(WidgetTester tester) async {
  await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late NativeAppController controller;

  setUp(() {
    controller = NativeAppController();
    _mockRuntimeChannel();
  });

  tearDown(() {
    controller.dispose();
    _clearChannels();
  });

  testWidgets('三点按钮打开锚定菜单，非 Android 不显示检查更新', (tester) async {
    await _pumpHome(tester, controller: controller);
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);

    await _openMenu(tester);

    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);
    expect(find.text('设置'), findsOneWidget);
    expect(find.text('导出日志'), findsOneWidget);
    expect(find.text('关于'), findsOneWidget);
    expect(find.text('检查更新'), findsNothing);
    expect(find.byType(LiquidGlass), findsWidgets);
    // 焦点进入菜单第一项。
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');
    expect(tester.takeException(), isNull);
  });

  testWidgets('Android 菜单包含检查更新', (tester) async {
    await _pumpHome(tester, controller: controller, isAndroid: true);
    await _openMenu(tester);

    expect(find.text('检查更新'), findsOneWidget);
  });

  testWidgets('菜单键盘上下可导航', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-1');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');
  });

  testWidgets('Escape 关闭菜单并把焦点还给三点按钮', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('点击菜单外部关闭菜单', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tapAt(const Offset(600, 320));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
  });

  testWidgets('菜单项「设置」打开居中玻璃浮层，首个焦点在设置内容', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('settings-sheet')), findsOneWidget);
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(find.text('自定义名称'), findsOneWidget);
    expect(find.byKey(const ValueKey('export-logs')), findsOneWidget);
    // 焦点进入设置内容（MiPlay 行），而不是关闭按钮。
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');

    // Escape 先关设置浮层。
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('设置浮层可滚动且关闭按钮可点击', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);
    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    final scrollable = find.descendant(
      of: find.byKey(const ValueKey('settings-sheet')),
      matching: find.byType(Scrollable),
    );
    expect(scrollable, findsWidgets);
    await tester.drag(scrollable.first, const Offset(0, -160));
    await tester.pump();

    await tester.tap(find.bySemanticsLabel('关闭设置'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
  });

  testWidgets('菜单项「关于」展示版本/协议/设备名', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('关于'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(find.text('FusionPlay'), findsWidgets);
    expect(find.text('版本'), findsOneWidget);
    expect(find.text(fusionPlayVersion), findsOneWidget);
    expect(find.text('协议'), findsOneWidget);
    expect(find.text('AirPlay'), findsWidgets);
    expect(find.text('设备名'), findsOneWidget);
    expect(find.text('客厅 FusionPlay'), findsOneWidget);

    await tester.tap(find.text('好'));
    await tester.pumpAndSettle();
    expect(find.text('版本'), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('菜单项「导出日志」触发控制器回调', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('导出日志'));
    await tester.pump();

    expect(_runtimeMethods, contains('exportLogs'));
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
  });

  testWidgets('Android 菜单项「检查更新」复用更新流程', (tester) async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_updateChannel, (call) async {
          expect(call.method, 'check');
          return <String, Object?>{'status': 'up_to_date'};
        });

    await _pumpHome(tester, controller: controller, isAndroid: true);
    await _openMenu(tester);
    await tester.tap(find.text('检查更新'));
    await tester.pumpAndSettle();

    expect(find.text('已是最新版本'), findsOneWidget);
    await tester.tap(find.text('好'));
    await tester.pumpAndSettle();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('menu/contextMenu 快捷键打开菜单，返回键先关设置再关菜单', (tester) async {
    await _pumpHome(tester, controller: controller);

    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await tester.pumpAndSettle();
    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsOneWidget);

    // 返回键（escape）优先关闭设置浮层，而不是直接退出页面。
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
  });
}
