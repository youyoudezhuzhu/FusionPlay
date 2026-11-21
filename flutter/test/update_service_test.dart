import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/android/native_app_controller.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/backend/windows/windows_app_controller.dart';

const _updateChannel = MethodChannel(AndroidUpdateService.channelName);

void _mockUpdateChannel(Future<Object?> Function(MethodCall call) handler) {
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_updateChannel, handler);
}

void _clearUpdateChannel() {
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_updateChannel, null);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  tearDown(_clearUpdateChannel);

  group('UpdateCheckResult.fromMap', () {
    test('解析 available 的全部字段', () {
      final result = UpdateCheckResult.fromMap(const {
        'status': 'available',
        'downloaded': true,
        'releaseKey': 'release-a',
        'versionName': '1.4.0',
        'versionCode': 10400,
        'notes': '修复若干问题',
        'publishedAt': '2026-11-01T10:00:00+08:00',
        'apkSize': 12345678,
        'message': null,
      });

      expect(result.status, UpdateCheckStatus.available);
      expect(result.releaseKey, 'release-a');
      expect(result.downloaded, isTrue);
      expect(result.versionName, '1.4.0');
      expect(result.versionCode, 10400);
      expect(result.notes, '修复若干问题');
      expect(result.publishedAt, '2026-11-01T10:00:00+08:00');
      expect(result.apkSize, 12345678);
      expect(result.message, isNull);
    });

    test('up_to_date / unavailable 与未知状态', () {
      expect(
        UpdateCheckResult.fromMap(const {'status': 'up_to_date'}).status,
        UpdateCheckStatus.upToDate,
      );
      final unavailable = UpdateCheckResult.fromMap(const {
        'status': 'unavailable',
        'message': '服务器暂不可达',
      });
      expect(unavailable.status, UpdateCheckStatus.unavailable);
      expect(unavailable.message, '服务器暂不可达');
      expect(
        UpdateCheckResult.fromMap(const {'status': 'error'}).status,
        UpdateCheckStatus.error,
      );
      // 未知状态按错误处理，避免把异常载荷当成可安装更新。
      expect(
        UpdateCheckResult.fromMap(const {'status': 'weird'}).status,
        UpdateCheckStatus.error,
      );
    });
  });

  group('UpdateInstallResult.fromMap', () {
    test('解析 started / permission_required / error', () {
      expect(
        UpdateInstallResult.fromMap(const {'status': 'started'}).status,
        UpdateInstallStatus.started,
      );
      final permission = UpdateInstallResult.fromMap(const {
        'status': 'permission_required',
        'message': '请允许安装未知应用',
      });
      expect(permission.status, UpdateInstallStatus.permissionRequired);
      expect(permission.message, '请允许安装未知应用');
      final error = UpdateInstallResult.fromMap(const {
        'status': 'error',
        'message': '下载失败',
      });
      expect(error.status, UpdateInstallStatus.error);
      expect(error.message, '下载失败');
    });
  });

  group('AndroidUpdateService（MethodChannel 契约）', () {
    test('check 调用 check 方法并解析 available', () async {
      final calls = <String>[];
      _mockUpdateChannel((call) async {
        calls.add(call.method);
        return <String, Object?>{
          'status': 'available',
          'versionName': '1.4.0',
          'versionCode': 10400,
          'notes': '新版本说明',
          'publishedAt': '2026-11-01T10:00:00+08:00',
          'apkSize': 12345678,
          'message': null,
        };
      });

      final result = await AndroidUpdateService().check();

      expect(calls, ['check']);
      expect(result.status, UpdateCheckStatus.available);
      expect(result.versionName, '1.4.0');
      expect(result.versionCode, 10400);
      expect(result.notes, '新版本说明');
      expect(result.apkSize, 12345678);
    });

    test('check 解析 up_to_date 与 error', () async {
      _mockUpdateChannel(
        (call) async => <String, Object?>{'status': 'up_to_date'},
      );
      expect(
        (await AndroidUpdateService().check()).status,
        UpdateCheckStatus.upToDate,
      );

      _mockUpdateChannel(
        (call) async => <String, Object?>{
          'status': 'error',
          'message': '网络连接失败',
        },
      );
      final failed = await AndroidUpdateService().check();
      expect(failed.status, UpdateCheckStatus.error);
      expect(failed.message, '网络连接失败');
    });

    test('downloadAndInstall 调用 downloadAndInstall 方法并解析 started', () async {
      final calls = <String>[];
      _mockUpdateChannel((call) async {
        calls.add(call.method);
        return <String, Object?>{'status': 'started', 'message': null};
      });

      final result = await AndroidUpdateService().downloadAndInstall();

      expect(calls, ['downloadAndInstall']);
      expect(result.status, UpdateInstallStatus.started);
    });

    test('downloadAndInstall 解析 permission_required 与 error', () async {
      _mockUpdateChannel(
        (call) async => <String, Object?>{
          'status': 'permission_required',
          'message': '请允许安装未知应用',
        },
      );
      final permission = await AndroidUpdateService().downloadAndInstall();
      expect(permission.status, UpdateInstallStatus.permissionRequired);
      expect(permission.message, '请允许安装未知应用');

      _mockUpdateChannel(
        (call) async => <String, Object?>{'status': 'error', 'message': '下载失败'},
      );
      final failed = await AndroidUpdateService().downloadAndInstall();
      expect(failed.status, UpdateInstallStatus.error);
      expect(failed.message, '下载失败');
    });

    test('通道缺失（旧安装包）优雅降级为 error', () async {
      _clearUpdateChannel();

      final check = await AndroidUpdateService().check();
      expect(check.status, UpdateCheckStatus.error);
      expect(check.message, '当前安装包不支持在线更新');

      final install = await AndroidUpdateService().downloadAndInstall();
      expect(install.status, UpdateInstallStatus.error);
      expect(install.message, '当前安装包不支持在线更新');
    });
  });

  test('下载通道仅接收当前安装包的进度，安装独立调用', () async {
    final calls = <String>[];
    _mockUpdateChannel((call) async {
      calls.add(call.method);
      expect(call.arguments, {'releaseKey': 'current'});
      if (call.method == 'install') return {'status': 'permission_required'};
      for (final key in ['previous', 'current']) {
        await TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .handlePlatformMessage(
              AndroidUpdateService.channelName,
              const StandardMethodCodec().encodeMethodCall(
                MethodCall('progress', {
                  'releaseKey': key,
                  'bytes': 42,
                  'total': 100,
                }),
              ),
              (_) {},
            );
      }
      return {'status': 'ready'};
    });
    final progress = <(int, int)>[];
    final service = AndroidUpdateService();
    await service.download(
      'current',
      (bytes, total) => progress.add((bytes, total)),
    );
    expect(progress, [(42, 100)]);
    expect(calls, ['download']);
    expect(
      (await service.install('current')).status,
      UpdateInstallStatus.permissionRequired,
    );
    expect(calls, ['download', 'install']);
  });

  group('Controller 的更新入口', () {
    test('NativeAppController 通过 AndroidUpdateService 走更新通道', () async {
      final methods = <String>[];
      _mockUpdateChannel((call) async {
        methods.add(call.method);
        return switch (call.method) {
          'check' => <String, Object?>{'status': 'available', 'versionCode': 1},
          _ => <String, Object?>{'status': 'started'},
        };
      });
      final controller = NativeAppController();
      addTearDown(controller.dispose);

      final check = await controller.checkForUpdate();
      final install = await controller.downloadAndInstallUpdate();

      expect(methods, ['check', 'downloadAndInstall']);
      expect(check.status, UpdateCheckStatus.available);
      expect(install.status, UpdateInstallStatus.started);
    });

    test('Windows 提供目录保存能力并报告侧车缺失', () async {
      final controller = WindowsAppController(
        executablePath: 'missing-for-update-test/FusionPlayCore.exe',
      );
      addTearDown(controller.dispose);

      final check = await controller.checkForUpdate();
      expect(controller.saveUpdate, isA<SaveUpdate>());
      expect(check.status, UpdateCheckStatus.error);
      expect(check.message, contains('FusionPlayCore.exe'));

      final install = await controller.downloadAndInstallUpdate();
      expect(install.status, UpdateInstallStatus.error);
      expect(install.message, '当前平台不支持在线更新');
    });
  });
}
