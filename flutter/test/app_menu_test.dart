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

AppState _state({bool advancedEffects = false}) => AppState(
  initialized: true,
  receiverName: '客厅 FusionPlay',
  settings: AppSettings(advancedEffectsEnabled: advancedEffects),
  playback: const PlaybackSnapshot(
    title: 'Track',
    artist: 'Artist',
    durationMs: 180000,
    streamActive: true,
    protocol: 'airplay',
    qualityText: 'AAC 44.1 kHz',
  ),
  remoteControl: const RemoteControlState(
    available: true,
    commands: {'play_pause', 'previous_track', 'next_track'},
  ),
  controls: PlaybackControls.all,
);

Future<void> _pumpHome(
  WidgetTester tester, {
  required NativeAppController controller,
  bool isAndroid = false,
  bool advancedEffects = false,
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
        state: _state(advancedEffects: advancedEffects),
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

  testWidgets('菜单项「设置」morph 成右侧玻璃面板，焦点进入设置内容', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('settings-sheet')), findsOneWidget);
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(find.text('自定义名称'), findsOneWidget);
    expect(find.byKey(const ValueKey('export-logs')), findsOneWidget);
    // 焦点进入设置内容（MiPlay 行），不会聚焦名称输入框/弹键盘。
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');
    expect(find.byType(TextField), findsNothing);
    expect(find.byKey(const ValueKey('edit-name')), findsOneWidget);

    // 第一次 Escape：设置收回菜单（不是直接回播放页），焦点回菜单首项。
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');

    // 第二次 Escape：菜单收回三点按钮。
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('设置标题使用官方 PinnedHeaderSliver，内容从标题下滚过', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('settings-pinned-header')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('settings-scroll')), findsOneWidget);

    // 顶栏是透明玻璃表面：pinned sliver 只占位，没有深色实色渐变遮挡。
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('settings-pinned-header')),
        matching: find.byType(DecoratedBox),
      ),
      findsNothing,
    );
    final header = tester.widget<DecoratedBox>(
      find.byKey(const ValueKey('settings-glass-header')),
    );
    final decoration = header.decoration as BoxDecoration;
    final gradient = decoration.gradient as LinearGradient;
    expect(
      gradient.colors.every((color) => color.a < 1),
      isTrue,
      reason: 'header must not paint an opaque/dark band',
    );

    await tester.drag(
      find.byKey(const ValueKey('settings-scroll')),
      const Offset(0, -140),
    );
    await tester.pumpAndSettle();

    // 标题仍钉在顶部，内容没有在标题区域留下硬分割/裁切。
    expect(
      find.byKey(const ValueKey('settings-pinned-header')),
      findsOneWidget,
    );
    expect(find.text('设置'), findsOneWidget);
    expect(tester.takeException(), isNull);
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
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
  });

  testWidgets('菜单项「关于」展示版本/协议/设备名', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);

    await tester.tap(find.text('关于'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byKey(const ValueKey('about-dialog')), findsOneWidget);
    expect(find.byType(AlertDialog), findsNothing);
    expect(find.text('FusionPlay'), findsWidgets);
    expect(find.text('版本'), findsOneWidget);
    expect(find.text(fusionPlayVersion), findsOneWidget);
    expect(find.text('协议'), findsOneWidget);
    expect(find.text('AirPlay 2'), findsWidgets);
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
    await tester.pumpAndSettle();

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

  testWidgets('菜单从三点按钮 rect morph 展开（圆角容器形变）', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      advancedEffects: true,
      size: const Size(1000, 600),
    );
    final buttonRect = tester.getRect(
      find.byKey(const ValueKey('playback-menu-button')),
    );

    await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));

    final midRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );
    // 动画中：容器比最终菜单小，且靠近按钮（不是凭空 fade 出现）。
    expect(midRect.width, lessThan(240));
    expect(midRect.width, greaterThan(30));
    expect((midRect.center - buttonRect.center).distance, lessThan(160));

    await tester.pumpAndSettle();
    final menuRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );
    expect(menuRect.width, 240);
    // The expanded menu keeps the trigger's top-right corner and top edge;
    // only its left/bottom edges grow.
    expect(menuRect.right, closeTo(buttonRect.right, .01));
    expect(menuRect.top, closeTo(buttonRect.top, .01));
    expect(menuRect.height, closeTo(3 * 46 + 14, .01));
    final container = tester.widget<LiquidGlass>(
      find.byKey(const ValueKey('morph-container')),
    );
    expect(container.borderRadius, closeTo(buttonRect.height / 2, .01));
  });

  testWidgets('设置面板 morph 到右半边：宽 ≤ 屏宽/2，右/上留 16', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      advancedEffects: true,
      size: const Size(1200, 600),
    );
    await _openMenu(tester);
    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    final settingsRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );
    expect(settingsRect.width, lessThanOrEqualTo(1200 / 2));
    expect(settingsRect.width, 560);
    expect(1200 - settingsRect.right, closeTo(16, .01));
    expect(settingsRect.top, closeTo(16, .01));
    expect(settingsRect.bottom, closeTo(600 - 16, .01));
    expect(find.byKey(const ValueKey('morph-overlay')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('设置面板关闭：先收回菜单再收回按钮', (tester) async {
    await _pumpHome(tester, controller: controller);
    await _openMenu(tester);
    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('settings-sheet')), findsNothing);
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsNothing);
    expect(find.byKey(const ValueKey('morph-overlay')), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-menu');
  });

  testWidgets('reduce motion 时菜单直接到位（无过渡动画）', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      size: const Size(1000, 600),
    );
    final buttonRect = tester.getRect(
      find.byKey(const ValueKey('playback-menu-button')),
    );

    await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
    await tester.pump();
    // reduce motion 下没有过渡：下一帧即到目标位置。
    await tester.pump();
    final firstFrameRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );

    expect(firstFrameRect.width, 240);
    expect(firstFrameRect.top, closeTo(buttonRect.top, .01));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-menu')), findsOneWidget);
  });

  testWidgets('800x360：菜单与设置面板在屏内且不溢出', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      advancedEffects: true,
      size: const Size(800, 360),
    );

    await _openMenu(tester);
    final menuRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );
    expect(menuRect.width, 240);
    expect(menuRect.left, greaterThanOrEqualTo(0));
    expect(menuRect.right, lessThanOrEqualTo(800));
    expect(menuRect.top, greaterThanOrEqualTo(0));
    expect(menuRect.bottom, lessThanOrEqualTo(360));
    expect(tester.takeException(), isNull);

    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();

    final settingsRect = tester.getRect(
      find.byKey(const ValueKey('morph-container')),
    );
    expect(settingsRect.width, 400);
    expect(settingsRect.width, lessThanOrEqualTo(800 / 2));
    expect(settingsRect.right, closeTo(800 - 16, .01));
    expect(settingsRect.top, closeTo(16, .01));
    expect(settingsRect.bottom, closeTo(360 - 16, .01));
    expect(find.text('自定义名称'), findsOneWidget);
    expect(tester.takeException(), isNull);
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
