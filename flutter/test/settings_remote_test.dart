import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/settings_view.dart';

void main() {
  testWidgets('三点菜单打开设置浮层，Escape 逐层关闭', (tester) async {
    final controller = NativeAppController();
    addTearDown(controller.dispose);
    tester.view.physicalSize = const Size(1200, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        home: FusionPlayHome(
          controller: controller,
          state: const AppState(initialized: true),
          isAndroid: false,
        ),
      ),
    );
    await tester.pump();

    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
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

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
    // 设置先收回菜单，再关菜单。
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
  });

  testWidgets('MiPlay 上键聚焦编辑按钮，点击编辑后才聚焦名称输入框', (tester) async {
    final initial = FocusNode(debugLabel: 'settings-miplay-test');
    addTearDown(initial.dispose);
    tester.view.physicalSize = const Size(480, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(splashFactory: NoSplash.splashFactory),
        home: Scaffold(
          body: SettingsView(
            state: const AppState(initialized: true),
            initialFocusNode: initial,
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
          ),
        ),
      ),
    );
    // 展示态：没有输入框，进入设置不会自动弹键盘。
    expect(find.byType(TextField), findsNothing);

    initial.requestFocus();
    await tester.pump();
    expect(initial.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(
      FocusManager.instance.primaryFocus?.debugLabel,
      'settings-name-edit',
    );
    expect(find.byType(TextField), findsNothing);

    await tester.tap(find.byKey(const ValueKey('edit-name')));
    await tester.pumpAndSettle();
    final textField = tester.widget<TextField>(find.byType(TextField));
    expect(textField.focusNode?.hasPrimaryFocus, isTrue);
  });

  testWidgets('名称展示→编辑→取消恢复原名，不触发持久化', (tester) async {
    final initial = FocusNode(debugLabel: 'settings-cancel-test');
    addTearDown(initial.dispose);
    final names = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(splashFactory: NoSplash.splashFactory),
        home: Scaffold(
          body: SettingsView(
            state: const AppState(
              initialized: true,
              settings: AppSettings(receiverName: '客厅 FusionPlay'),
            ),
            initialFocusNode: initial,
            onClose: () {},
            onReceiverName: names.add,
            onStartup: (_) {},
            onAutoWake: (_) {},
            onAdvancedEffects: (_) {},
            onMiPlay: (_) {},
            onMiPlayIdentity: (_) {},
            onAirPlay: (_) {},
            onDlna: (_) {},
            onExportLogs: () async {},
          ),
        ),
      ),
    );
    expect(find.text('客厅 FusionPlay'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('edit-name')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '书房');
    await tester.pump();
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();

    expect(names, isEmpty);
    expect(find.byType(TextField), findsNothing);
    expect(find.text('客厅 FusionPlay'), findsOneWidget);
  });

  testWidgets('名称编辑→应用原子调用一次 onReceiverName 并回到展示态', (tester) async {
    final initial = FocusNode(debugLabel: 'settings-save-test');
    addTearDown(initial.dispose);
    final names = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(splashFactory: NoSplash.splashFactory),
        home: Scaffold(
          body: SettingsView(
            state: const AppState(
              initialized: true,
              settings: AppSettings(receiverName: '旧名字'),
            ),
            initialFocusNode: initial,
            onReceiverName: names.add,
            onClose: () {},
            onStartup: (_) {},
            onAutoWake: (_) {},
            onAdvancedEffects: (_) {},
            onMiPlay: (_) {},
            onMiPlayIdentity: (_) {},
            onAirPlay: (_) {},
            onDlna: (_) {},
            onExportLogs: () async {},
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const ValueKey('edit-name')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '新名字');
    await tester.pump();
    await tester.tap(find.text('应用'));
    await tester.pumpAndSettle();

    expect(names, ['新名字']);
    expect(find.byType(TextField), findsNothing);
    expect(find.text('新名字'), findsOneWidget);
    expect(
      FocusManager.instance.primaryFocus?.debugLabel,
      isNot('settings-name'),
    );
  });

  testWidgets('日志导出入口与关于应用位于同一卡片并可触发', (tester) async {
    var exportCount = 0;
    final initial = FocusNode(debugLabel: 'settings-export-test');
    addTearDown(initial.dispose);
    tester.view.physicalSize = const Size(480, 1100);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(splashFactory: NoSplash.splashFactory),
        home: Scaffold(
          body: SettingsView(
            state: const AppState(initialized: true),
            initialFocusNode: initial,
            onClose: () {},
            onReceiverName: (_) {},
            onStartup: (_) {},
            onAutoWake: (_) {},
            onAdvancedEffects: (_) {},
            onMiPlay: (_) {},
            onMiPlayIdentity: (_) {},
            onAirPlay: (_) {},
            onDlna: (_) {},
            onExportLogs: () async => exportCount++,
          ),
        ),
      ),
    );

    expect(find.text('关于应用'), findsOneWidget);
    expect(find.text('导出日志'), findsOneWidget);
    await tester.ensureVisible(find.byKey(const ValueKey('export-logs')));
    await tester.tap(find.byKey(const ValueKey('export-logs')));
    await tester.pump();

    expect(exportCount, 1);
  });

  testWidgets('select does not open settings while player control has focus', (
    tester,
  ) async {
    final controller = NativeAppController();
    addTearDown(controller.dispose);
    tester.view.physicalSize = const Size(1200, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        home: FusionPlayHome(
          controller: controller,
          state: const AppState(
            initialized: true,
            playback: PlaybackSnapshot(
              title: 'Track',
              protocol: 'AirPlay',
              durationMs: 180000,
              streamActive: true,
            ),
          ),
          isAndroid: false,
        ),
      ),
    );
    await tester.pump();

    final playPause = FocusManager.instance.rootScope.descendants.singleWhere(
      (node) => node.debugLabel == 'player-play-pause',
    );
    playPause.requestFocus();
    await tester.pump();
    expect(playPause.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.select);
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
  });

  testWidgets('player focus follows settings progress and play pause space', (
    tester,
  ) async {
    final controller = NativeAppController();
    addTearDown(controller.dispose);
    tester.view.physicalSize = const Size(1200, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        home: FusionPlayHome(
          controller: controller,
          state: const AppState(
            initialized: true,
            playback: PlaybackSnapshot(
              title: 'Track',
              protocol: 'AirPlay',
              durationMs: 180000,
              streamActive: true,
            ),
            remoteControl: RemoteControlState(
              available: true,
              commands: {'play_pause', 'seek'},
            ),
          ),
          isAndroid: false,
        ),
      ),
    );
    await tester.pump();

    final nodes = FocusManager.instance.rootScope.descendants;
    final progress = nodes.singleWhere(
      (node) => node.debugLabel == 'player-progress',
    );
    final playPause = nodes.singleWhere(
      (node) => node.debugLabel == 'player-play-pause',
    );
    final settings = nodes.singleWhere(
      (node) => node.debugLabel == 'player-menu',
    );

    progress.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(playPause.hasPrimaryFocus, isTrue);

    progress.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(settings.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(progress.hasPrimaryFocus, isTrue);

    final settingsRect = tester.getRect(
      find.byKey(const ValueKey('playback-menu-button')),
    );
    // The menu trigger now lives in the bottom secondary bar, not the top bar.
    expect(settingsRect.top, greaterThan(0));
    expect(settingsRect.bottom, lessThanOrEqualTo(600));
    expect(settingsRect.right, lessThanOrEqualTo(1200));
  });
}
