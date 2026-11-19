import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/main.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/remote_focus.dart';

AppState _state({bool isTelevision = false, bool advancedEffects = false}) =>
    AppState(
      initialized: true,
      receiverName: '客厅 FusionPlay',
      isTelevision: isTelevision,
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
  required AppState state,
  Size size = const Size(1600, 900),
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
        state: state,
        isAndroid: false,
        forceGlassFallback: true,
      ),
    ),
  );
  await tester.pump();
}

Future<void> _openSettings(WidgetTester tester) async {
  await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
  await tester.pumpAndSettle();
  await tester.tap(find.text('设置'));
  await tester.pumpAndSettle();
}

AnimatedOpacity _menuButtonOpacity(WidgetTester tester) =>
    tester.widget<AnimatedOpacity>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('playback-menu-button')),
            matching: find.byType(AnimatedOpacity),
          )
          .first,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late NativeAppController controller;

  setUp(() => controller = NativeAppController());
  tearDown(() => controller.dispose());

  group('isTelevision 状态解析', () {
    test('root isTelevision 解析，缺省 false', () {
      expect(
        AppState.fromMap(const {'isTelevision': true}).isTelevision,
        isTrue,
      );
      expect(
        AppState.fromMap(const {'isTelevision': false}).isTelevision,
        isFalse,
      );
      expect(AppState.fromMap(const {}).isTelevision, isFalse);
    });

    test('isTelevision 纳入相等性/hash/copyWith', () {
      final tv = AppState.fromMap(const {'isTelevision': true});
      final tvAgain = AppState.fromMap(const {'isTelevision': true});
      final phone = AppState.fromMap(const {});
      expect(tv, tvAgain);
      expect(tv == phone, isFalse);
      expect(tv.hashCode, tvAgain.hashCode);
      expect(tv.copyWith().isTelevision, isTrue);
    });
  });

  testWidgets('TV 播放页有 overscan 安全边距，非 TV 为 0', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    final tvPadding = tester.widget<Padding>(
      find.byKey(const ValueKey('player-overscan')),
    );
    expect(
      tvPadding.padding,
      const EdgeInsets.symmetric(horizontal: 28, vertical: 14),
    );

    await tester.pumpWidget(
      MaterialApp(
        theme: fusionTheme(),
        home: FusionPlayHome(
          controller: controller,
          state: _state(),
          isAndroid: false,
          forceGlassFallback: true,
        ),
      ),
    );
    await tester.pump();
    final phonePadding = tester.widget<Padding>(
      find.byKey(const ValueKey('player-overscan')),
    );
    expect(phonePadding.padding, EdgeInsets.zero);
  });

  testWidgets('TV 焦点样式：ring 4px + focus scale 1.02；非 TV 默认', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    var style = tester.widget<FusionFocusStyle>(find.byType(FusionFocusStyle));
    expect(style.borderWidth, 4);
    expect(style.focusScale, 1.02);

    await tester.pumpWidget(
      MaterialApp(
        theme: fusionTheme(),
        home: FusionPlayHome(
          controller: controller,
          state: _state(),
          isAndroid: false,
          forceGlassFallback: true,
        ),
      ),
    );
    await tester.pump();
    style = tester.widget<FusionFocusStyle>(find.byType(FusionFocusStyle));
    expect(style.borderWidth, 3);
    expect(style.focusScale, 1);
  });

  testWidgets('TV 打开设置焦点在 MiPlay 行；非 TV 也不自动聚焦名称输入框', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    await _openSettings(tester);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');

    await tester.pumpWidget(
      MaterialApp(
        theme: fusionTheme(),
        home: FusionPlayHome(
          controller: controller,
          state: _state(),
          isAndroid: false,
          forceGlassFallback: true,
        ),
      ),
    );
    await tester.pump();
    await _openSettings(tester);
    // 非 TV 同样不自动聚焦名称输入框（避免自动弹键盘）。
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');
    expect(find.byType(TextField), findsNothing);
  });

  testWidgets('TV 设置 MiPlay 上键聚焦编辑按钮，点击编辑后才聚焦名称输入框', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    await _openSettings(tester);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(
      FocusManager.instance.primaryFocus?.debugLabel,
      'settings-name-edit',
    );
    expect(find.byType(TextField), findsNothing);

    await tester.tap(find.byKey(const ValueKey('edit-name')));
    await tester.pumpAndSettle();
    final field = tester.widget<TextField>(find.byType(TextField));
    expect(field.focusNode?.hasPrimaryFocus, isTrue);
    expect(field.controller?.selection.isCollapsed, isTrue);

    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(find.byType(TextField), findsNothing);

    final miPlayFocus = FocusManager.instance.rootScope.descendants
        .where((node) => node.debugLabel == 'settings-miplay')
        .toList();
    expect(miPlayFocus, isNotEmpty);
    miPlayFocus.first.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    final focused = FocusManager.instance.primaryFocus;
    expect(focused, isNotNull);
    expect(focused?.debugLabel, isNot('settings-name'));
    expect(focused?.debugLabel, isNot('settings-name-edit'));
  });

  testWidgets('进入设置不弹键盘/不选文本；点击编辑后才请求输入法', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    await _openSettings(tester);
    expect(find.byType(TextField), findsNothing);
    expect(tester.testTextInput.isVisible, isFalse);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-miplay');

    await tester.tap(find.byKey(const ValueKey('edit-name')));
    await tester.pumpAndSettle();
    expect(tester.testTextInput.isVisible, isTrue);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'settings-name');

    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(tester.testTextInput.isVisible, isFalse);
  });

  testWidgets('TV 设置面板宽度上限 720（仍 ≤ 半屏），非 TV 为 560', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    await _openSettings(tester);
    var rect = tester.getRect(find.byKey(const ValueKey('morph-container')));
    expect(rect.width, 720);
    expect(rect.width, lessThanOrEqualTo(1600 / 2));

    await tester.pumpWidget(
      MaterialApp(
        theme: fusionTheme(),
        home: FusionPlayHome(
          controller: controller,
          state: _state(),
          isAndroid: false,
          forceGlassFallback: true,
        ),
      ),
    );
    await tester.pump();
    await _openSettings(tester);
    rect = tester.getRect(find.byKey(const ValueKey('morph-container')));
    expect(rect.width, 560);
  });

  testWidgets('菜单打开时三点按钮淡出/缩小/不可点/不可聚焦，关闭后恢复', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(advancedEffects: true),
    );
    expect(_menuButtonOpacity(tester).opacity, 1);

    await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
    await tester.pumpAndSettle();
    expect(_menuButtonOpacity(tester).opacity, 0);
    final ignorePointer = tester.widget<IgnorePointer>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('playback-menu-button')),
            matching: find.byType(IgnorePointer),
          )
          .first,
    );
    expect(ignorePointer.ignoring, isTrue);
    final excludeFocus = tester.widget<ExcludeFocus>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('playback-menu-button')),
            matching: find.byType(ExcludeFocus),
          )
          .first,
    );
    expect(excludeFocus.excluding, isTrue);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'menu-item-0');

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(_menuButtonOpacity(tester).opacity, 1);

    final ignorePointerAfter = tester.widget<IgnorePointer>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('playback-menu-button')),
            matching: find.byType(IgnorePointer),
          )
          .first,
    );
    expect(ignorePointerAfter.ignoring, isFalse);
  });

  testWidgets('设置打开期间三点按钮保持隐藏', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(advancedEffects: true),
    );
    await _openSettings(tester);
    expect(_menuButtonOpacity(tester).opacity, 0);
  });

  testWidgets('reduce motion 时三点按钮直接隐藏', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(advancedEffects: false),
    );
    await tester.tap(find.byKey(const ValueKey('playback-menu-button')));
    await tester.pumpAndSettle();
    expect(_menuButtonOpacity(tester).opacity, 0);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(_menuButtonOpacity(tester).opacity, 1);
  });

  testWidgets('TV 播放页 D-pad 焦点链仍通（菜单→进度→播放）', (tester) async {
    await _pumpHome(
      tester,
      controller: controller,
      state: _state(isTelevision: true),
    );
    final nodes = FocusManager.instance.rootScope.descendants;
    final menu = nodes.singleWhere((node) => node.debugLabel == 'player-menu');
    final progress = nodes.singleWhere(
      (node) => node.debugLabel == 'player-progress',
    );
    menu.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(progress.hasPrimaryFocus, isTrue);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'player-play-pause');
  });
}
