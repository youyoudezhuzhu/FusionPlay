import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart';
import 'package:fusionplay_android_flutter/ui/settings_view.dart';

Future<void> _pumpSettings(
  WidgetTester tester, {
  required bool isAndroid,
  Future<UpdateCheckResult> Function()? onCheckUpdate,
  Future<UpdateInstallResult> Function()? onInstallUpdate,
  Size size = const Size(480, 1100),
}) async {
  final initialFocus = FocusNode(debugLabel: 'settings-update-test');
  addTearDown(initialFocus.dispose);
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);

  await tester.pumpWidget(
    MaterialApp(
      theme: ThemeData(splashFactory: NoSplash.splashFactory),
      home: Scaffold(
        body: SettingsView(
          state: const AppState(initialized: true),
          initialFocusNode: initialFocus,
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
          isAndroid: isAndroid,
          onCheckUpdate: onCheckUpdate,
          onInstallUpdate: onInstallUpdate,
        ),
      ),
    ),
  );
  await tester.pump();
}

Future<void> _tapCheckUpdate(WidgetTester tester) async {
  final finder = find.byKey(const ValueKey('check-update'));
  await tester.ensureVisible(finder);
  await tester.tap(finder);
  await tester.pump();
}

const _available = UpdateCheckResult(
  status: UpdateCheckStatus.available,
  versionName: '1.4.0',
  versionCode: 10400,
  notes: '1. 修复 DLNA 播放偶发卡顿\n2. 优化 MiPlay 连接速度\n3. 更新 Chinese 文案',
  publishedAt: '2026-11-01T10:00:00+08:00',
  apkSize: 12345678,
);

void main() {
  testWidgets('Windows（非 Android）不显示检查更新行', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: false,
      onCheckUpdate: () async => _available,
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    expect(find.text('检查更新'), findsNothing);
    expect(find.text('导出日志'), findsOneWidget);
  });

  testWidgets('Android 显示检查更新行', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => _available,
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    expect(find.text('检查更新'), findsOneWidget);
  });

  testWidgets('available：检查→展示版本与长 notes→下载→安装器已打开', (tester) async {
    final checkCompleter = Completer<UpdateCheckResult>();
    final installCompleter = Completer<UpdateInstallResult>();
    var installCalls = 0;
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () => checkCompleter.future,
      onInstallUpdate: () {
        installCalls++;
        return installCompleter.future;
      },
    );

    await _tapCheckUpdate(tester);
    expect(find.text('正在检查更新…'), findsOneWidget);

    // 长 notes 必须在对话框内滚动，不能溢出。
    checkCompleter.complete(
      UpdateCheckResult(
        status: UpdateCheckStatus.available,
        versionName: '1.4.0',
        notes: List<String>.generate(
          40,
          (index) => '第 ${index + 1} 条更新说明：修复了一些已知问题。',
        ).join('\n'),
        publishedAt: '2026-11-01T10:00:00+08:00',
        apkSize: 12345678,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('正在检查更新…'), findsNothing);
    expect(find.textContaining('发现新版本 1.4.0'), findsOneWidget);
    expect(find.textContaining('安装包大小'), findsOneWidget);
    expect(find.textContaining('第 1 条更新说明'), findsOneWidget);
    expect(find.text('下载并安装'), findsOneWidget);
    expect(find.text('取消'), findsOneWidget);

    await tester.tap(find.text('下载并安装'));
    await tester.pump();
    await tester.pump();
    expect(find.text('正在下载更新…'), findsOneWidget);
    expect(installCalls, 1);

    installCompleter.complete(
      const UpdateInstallResult(status: UpdateInstallStatus.started),
    );
    await tester.pumpAndSettle();
    expect(find.text('安装器已打开'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('横屏窄高度下长 notes 在对话框内滚动且不溢出', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      size: const Size(640, 360),
      onCheckUpdate: () async => UpdateCheckResult(
        status: UpdateCheckStatus.available,
        versionName: '1.4.0',
        notes: List<String>.generate(
          60,
          (index) => '第 ${index + 1} 条更新说明：修复了一些已知问题。',
        ).join('\n'),
        publishedAt: '2026-11-01T10:00:00+08:00',
        apkSize: 12345678,
      ),
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();

    expect(find.textContaining('发现新版本 1.4.0'), findsOneWidget);
    expect(find.text('下载并安装'), findsOneWidget);
    expect(find.text('取消'), findsOneWidget);
    // 更新说明在受限高度的 ScrollView 中，可滚动查看末尾条目。
    expect(find.textContaining('第 60 条更新说明'), findsOneWidget);
    await tester.drag(
      find.byType(SingleChildScrollView).last,
      const Offset(0, -200),
    );
    await tester.pump();
    expect(find.text('下载并安装'), findsOneWidget);
  });

  testWidgets('available 对话框点取消不会触发下载', (tester) async {
    var installCalls = 0;
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => _available,
      onInstallUpdate: () async {
        installCalls++;
        return const UpdateInstallResult(status: UpdateInstallStatus.started);
      },
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();
    expect(find.textContaining('发现新版本'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byType(AlertDialog), findsNothing);

    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();

    expect(find.textContaining('发现新版本'), findsNothing);
    expect(installCalls, 0);
  });

  testWidgets('up_to_date：提示已是最新版本', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async =>
          const UpdateCheckResult(status: UpdateCheckStatus.upToDate),
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();

    expect(find.text('已是最新版本'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('error：展示侧车返回的 message', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => const UpdateCheckResult(
        status: UpdateCheckStatus.error,
        message: '网络连接失败',
      ),
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();

    expect(find.text('检查更新失败'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.text('网络连接失败'), findsOneWidget);
  });

  testWidgets('unavailable：展示提示文案', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => const UpdateCheckResult(
        status: UpdateCheckStatus.unavailable,
        message: '服务器暂不可达',
      ),
      onInstallUpdate: () async =>
          const UpdateInstallResult(status: UpdateInstallStatus.started),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();

    expect(find.text('当前无法检查更新'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.text('服务器暂不可达'), findsOneWidget);
  });

  testWidgets('permission_required：提示去系统设置允许安装未知应用', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => _available,
      onInstallUpdate: () async => const UpdateInstallResult(
        status: UpdateInstallStatus.permissionRequired,
        message: '请允许安装未知应用',
      ),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('下载并安装'));
    await tester.pump();
    await tester.pumpAndSettle();

    expect(find.text('需要安装权限'), findsOneWidget);
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byType(AlertDialog), findsNothing);
    expect(find.textContaining('允许'), findsWidgets);
  });

  testWidgets('downloadAndInstall error：展示 message', (tester) async {
    await _pumpSettings(
      tester,
      isAndroid: true,
      onCheckUpdate: () async => _available,
      onInstallUpdate: () async => const UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: '下载校验失败',
      ),
    );

    await _tapCheckUpdate(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('下载并安装'));
    await tester.pump();
    await tester.pumpAndSettle();

    expect(find.text('下载更新失败'), findsOneWidget);
    expect(find.text('下载校验失败'), findsOneWidget);
  });
}
