import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/backend/app_controller.dart';
import 'package:fusionplay_android_flutter/backend/native_app_controller.dart';
import 'package:fusionplay_android_flutter/backend/windows_app_controller.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';

/// In-memory [SidecarTransport] used to drive the controller without a real
/// `FusionPlayCore.exe`.
class FakeSidecarTransport implements SidecarTransport {
  final StreamController<String> _output = StreamController<String>();
  final StreamController<String> _error = StreamController<String>();
  final Completer<int> _exited = Completer<int>();

  final List<String> sentLines = <String>[];
  bool started = false;
  bool killed = false;
  bool closeStdoutOnExit = false;

  @override
  Stream<String> get outputLines => _output.stream;

  @override
  Stream<String> get errorLines => _error.stream;

  @override
  Future<int> get exited => _exited.future;

  @override
  Future<void> start() async {
    started = true;
  }

  @override
  void sendLine(String line) {
    sentLines.add(line);
  }

  @override
  Future<void> kill() async {
    killed = true;
    finishExit(-9);
  }

  void emitEvent(Map<String, Object?> event) {
    _output.add(jsonEncode(event));
  }

  void emitRawLine(String line) {
    _output.add(line);
  }

  void emitStderrLine(String line) {
    _error.add(line);
  }

  void finishExit(int code) {
    if (!_exited.isCompleted) _exited.complete(code);
    if (closeStdoutOnExit) {
      unawaited(_output.close());
      unawaited(_error.close());
    }
  }

  Map<String, Object?> get lastCommand =>
      (jsonDecode(sentLines.last) as Map).cast<String, Object?>();
}

Map<String, Object?> _appStateMap({
  bool initialized = true,
  bool coreRunning = true,
  bool receiverReady = true,
  String receiverName = '客厅电脑',
  String? lastError,
  String? title = '晴天',
  bool isPlaying = true,
}) => <String, Object?>{
  'initialized': initialized,
  'busy': false,
  'coreRunning': coreRunning,
  'receiverReady': receiverReady,
  'receiverPort': 7000,
  'receiverDeviceId': '02:11:22:33:44:55',
  'connectedClient': '192.168.1.20',
  'activeMediaSource': 'airplay',
  'selectedCoreMediaSource': 'airplay',
  'receiverName': receiverName,
  'settings': <String, Object?>{
    'schemaVersion': 8,
    'receiverName': receiverName,
    'startupEnabled': true,
    'autoWakeEnabled': true,
    'advancedEffectsEnabled': false,
    'miPlayEnabled': true,
    'miPlayDeviceIdentity': 'television',
    'airPlayEnabled': true,
    'dlnaEnabled': true,
  },
  'playback': <String, Object?>{
    'title': title,
    'artist': '周杰伦',
    'album': '叶惠美',
    'coverArt': r'C:\covers\1.jpg',
    'mediaUrl': 'http://192.168.1.2/song.mp3',
    'mediaKind': 'audio',
    'protocol': 'airplay',
    'qualityText': 'AAC 44.1 kHz',
    'durationMs': 269000,
    'positionMs': 42000,
    'volumePercent': 65,
    'isPlaying': isPlaying,
    'streamActive': isPlaying,
    'sourceEpoch': 7,
    'trackIdentity': 'airplay:1',
    'trackChangeDirection': 'next',
  },
  'remoteControl': <String, Object?>{
    'available': true,
    'commands': <String>['play', 'pause'],
    'transport': 'airplay',
    'experimental': false,
  },
  'lastError': lastError,
};

Map<String, Object?> _readyEvent() => <String, Object?>{
  'type': 'ready',
  'app': 'FusionPlay',
  'role': 'core',
  'version': '1.3.0',
  'dataDir': r'C:\Users\me\AppData\Local\FusionPlay',
  'logFile': r'C:\Users\me\AppData\Local\FusionPlay\logs\fusionplay.log',
  'nullAudio': true,
};

Future<Map<String, Object?>> _roundTrip(
  FakeSidecarTransport fake,
  Future<void> Function() action,
) async {
  final future = action();
  await pumpEventQueue();
  final command = fake.lastCommand;
  expect(command['requestId'], isA<String>());
  fake.emitEvent(<String, Object?>{
    'type': 'response',
    'requestId': command['requestId'],
    'command': command['command'],
    'ok': true,
    'message': null,
  });
  await future;
  return command;
}

/// Creates a controller wired to a fake transport, with cleanup that never
/// depends on a real executable.
Future<WindowsAppController> _createController(
  FakeSidecarTransport fake, {
  Duration shutdownTimeout = const Duration(seconds: 2),
  Duration killTimeout = const Duration(seconds: 1),
}) async {
  final controller = WindowsAppController(
    transport: fake,
    shutdownTimeout: shutdownTimeout,
    killTimeout: killTimeout,
  );
  addTearDown(() async {
    fake.finishExit(0);
    await controller.shutdown();
    controller.dispose();
  });
  await controller.initialize();
  return controller;
}

void main() {
  group('NDJSON 命令序列化', () {
    test('方法名与 Android MethodChannel 参数键一致并携带 requestId', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      var command = await _roundTrip(
        fake,
        () => controller.setReceiverName('客厅电脑'),
      );
      expect(command['command'], 'setReceiverName');
      expect(command['value'], '客厅电脑');

      command = await _roundTrip(
        fake,
        () => controller.setStartupEnabled(false),
      );
      expect(command['command'], 'setStartupEnabled');
      expect(command['value'], false);

      command = await _roundTrip(
        fake,
        () => controller.setAutoWakeEnabled(true),
      );
      expect(command['command'], 'setAutoWakeEnabled');
      expect(command['value'], true);

      command = await _roundTrip(
        fake,
        () => controller.setAdvancedEffectsEnabled(true),
      );
      expect(command['command'], 'setAdvancedEffectsEnabled');
      expect(command['value'], true);

      command = await _roundTrip(
        fake,
        () => controller.setProtocolEnabled('airplay', true),
      );
      expect(command['command'], 'setProtocolEnabled');
      expect(command['protocol'], 'airplay');
      expect(command['value'], true);

      command = await _roundTrip(
        fake,
        () => controller.setMiPlayDeviceIdentity(MiPlayDeviceIdentity.tablet),
      );
      expect(command['command'], 'setMiPlayDeviceIdentity');
      expect(command['value'], 'tablet');

      command = await _roundTrip(fake, () => controller.seek(42500));
      expect(command['command'], 'seek');
      expect(command['positionMs'], 42500);

      command = await _roundTrip(fake, () => controller.volume(42));
      expect(command['command'], 'volume');
      expect(command['percent'], 42);

      command = await _roundTrip(fake, controller.exportLogs);
      expect(command['command'], 'exportLogs');
      expect(command.containsKey('value'), isFalse);

      command = await _roundTrip(fake, controller.clearError);
      expect(command['command'], 'clearError');
    });

    test('playback 使用 value 承载 PlaybackCommand.name 线上值', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      var command = await _roundTrip(
        fake,
        () => controller.playback(PlaybackCommand.playPause),
      );
      expect(command['command'], 'playback');
      expect(command['value'], 'playPause');
      expect(command.containsKey('value'), isTrue);

      command = await _roundTrip(
        fake,
        () => controller.playback(PlaybackCommand.previous),
      );
      expect(command['value'], 'previous');

      command = await _roundTrip(
        fake,
        () => controller.playback(PlaybackCommand.next),
      );
      expect(command['value'], 'next');
    });

    test('response.ok=false 时写入 lastError 且 Future 正常结束', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      final future = controller.setReceiverName('客厅');
      await pumpEventQueue();
      final command = fake.lastCommand;
      fake.emitEvent(<String, Object?>{
        'type': 'response',
        'requestId': command['requestId'],
        'command': command['command'],
        'ok': false,
        'message': '设置保存失败',
      });
      await future;

      expect(controller.state.lastError, '设置保存失败');
    });

    test('核心未运行时命令直接写入 lastError', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);
      fake.finishExit(0);
      await pumpEventQueue();
      expect(controller.coreRunning, isFalse);

      await controller.setStartupEnabled(true);
      expect(controller.state.lastError, contains('未运行'));
      expect(fake.sentLines, isEmpty);
    });
  });

  group('事件处理', () {
    test('ready 事件记录诊断信息并视为核心运行中', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      expect(controller.coreRunning, isTrue);
      fake.emitEvent(_readyEvent());
      await pumpEventQueue();

      final ready = controller.ready;
      expect(ready, isNotNull);
      expect(ready!.version, '1.3.0');
      expect(ready.dataDir, contains('FusionPlay'));
      expect(ready.logFile, endsWith('fusionplay.log'));
      expect(ready.nullAudio, isTrue);
    });

    test('initialize 不阻塞 UI，3 秒内无事件则提示接收器未响应', () async {
      final fake = FakeSidecarTransport();
      final controller = WindowsAppController(
        transport: fake,
        initializeTimeout: const Duration(milliseconds: 20),
      );
      addTearDown(() async {
        fake.finishExit(0);
        await controller.shutdown();
        controller.dispose();
      });

      await controller.initialize();
      expect(controller.coreRunning, isTrue);
      await Future<void>.delayed(const Duration(milliseconds: 60));

      expect(controller.state.lastError, '接收器未响应');
      expect(controller.coreRunning, isTrue);
    });

    test('initialize 超时前收到事件则不再提示未响应', () async {
      final fake = FakeSidecarTransport();
      final controller = WindowsAppController(
        transport: fake,
        initializeTimeout: const Duration(milliseconds: 40),
      );
      addTearDown(() async {
        fake.finishExit(0);
        await controller.shutdown();
        controller.dispose();
      });

      await controller.initialize();
      fake.emitEvent(_readyEvent());
      await Future<void>.delayed(const Duration(milliseconds: 80));

      expect(controller.state.lastError, isNull);
    });

    test('app_state 映射到 AppState 且重复状态不通知', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      var notifications = 0;
      controller.addListener(() => notifications++);

      fake.emitEvent(<String, Object?>{
        'type': 'app_state',
        'state': _appStateMap(),
      });
      await pumpEventQueue();

      expect(notifications, 1);
      final state = controller.state;
      expect(state.initialized, isTrue);
      expect(state.coreRunning, isTrue);
      expect(state.receiverReady, isTrue);
      expect(state.receiverPort, 7000);
      expect(state.connectedClient, '192.168.1.20');
      expect(state.activeMediaSource, 'airplay');
      expect(state.receiverName, '客厅电脑');
      expect(state.settings.schemaVersion, 8);
      expect(
        state.settings.miPlayDeviceIdentity,
        MiPlayDeviceIdentity.television,
      );
      expect(state.playback.title, '晴天');
      expect(state.playback.artist, '周杰伦');
      expect(state.playback.durationMs, 269000);
      expect(state.playback.positionMs, 42000);
      expect(state.playback.isPlaying, isTrue);
      expect(state.playback.trackChangeDirection, TrackChangeDirection.next);
      expect(state.remoteControl.available, isTrue);
      expect(state.remoteControl.commands, contains('play'));

      // 完全相同的状态必须去重。
      fake.emitEvent(<String, Object?>{
        'type': 'app_state',
        'state': _appStateMap(),
      });
      await pumpEventQueue();
      expect(notifications, 1);

      fake.emitEvent(<String, Object?>{
        'type': 'app_state',
        'state': _appStateMap(receiverName: '书房', isPlaying: false),
      });
      await pumpEventQueue();
      expect(notifications, 2);
      expect(controller.state.receiverName, '书房');
      expect(controller.state.playback.isPlaying, isFalse);
    });

    test('fatal 事件写入 lastError、停止核心并结束挂起命令', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      var notifications = 0;
      controller.addListener(() => notifications++);
      final pending = controller.setReceiverName('客厅');
      await pumpEventQueue();

      fake.emitEvent(<String, Object?>{
        'type': 'fatal',
        'message': 'FusionPlay 已在运行',
        'exitCode': 2,
      });
      await pending;
      fake.finishExit(2);
      await pumpEventQueue();

      expect(controller.coreRunning, isFalse);
      expect(controller.state.coreRunning, isFalse);
      expect(controller.state.lastError, 'FusionPlay 已在运行');
      expect(notifications, greaterThanOrEqualTo(2));
    });

    test('exiting 与进程退出都会把 coreRunning 置为 false', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      fake.emitEvent(<String, Object?>{
        'type': 'exiting',
        'reason': 'shutdown',
      });
      await pumpEventQueue();
      expect(controller.coreRunning, isFalse);
      expect(controller.state.coreRunning, isFalse);

      fake.finishExit(0);
      await pumpEventQueue();
      expect(controller.coreRunning, isFalse);
      expect(controller.state.lastError, isNull);
    });

    test('进程异常退出给出中文提示且不抛出', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      fake.finishExit(3);
      await pumpEventQueue();

      expect(controller.coreRunning, isFalse);
      expect(controller.state.coreRunning, isFalse);
      expect(controller.state.lastError, contains('已退出（退出码 3）'));
    });

    test('log 事件保留最近 300 条且不触发通知', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      var notifications = 0;
      controller.addListener(() => notifications++);
      for (var index = 0; index < 305; index++) {
        fake.emitEvent(<String, Object?>{
          'type': 'log',
          'timestamp': '2026-11-10T17:00:00.123+08:00',
          'level': 'info',
          'source': 'core',
          'message': 'line $index',
        });
      }
      await pumpEventQueue();

      expect(notifications, 0);
      expect(controller.recentLogs, hasLength(300));
      expect(controller.recentLogs.first.message, 'line 5');
      expect(controller.recentLogs.last.message, 'line 304');
      expect(controller.recentLogs.last.timestamp, isNotNull);
    });

    test('stderr 与非 JSON stdout 收集为诊断日志', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      fake.emitStderrLine('tracing: core started');
      fake.emitRawLine('not json at all');
      await pumpEventQueue();

      expect(controller.recentDiagnostics, hasLength(2));
      expect(
        controller.recentDiagnostics.first.message,
        'tracing: core started',
      );
      expect(controller.recentDiagnostics.first.fromStderr, isTrue);
      expect(controller.recentDiagnostics.last.source, 'stdout');
    });
  });

  group('生命周期', () {
    test('shutdown 发送 shutdown 命令并等待进程退出（不 kill）', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(fake);

      final shutdown = controller.shutdown();
      await pumpEventQueue();
      expect(fake.lastCommand, <String, Object?>{'command': 'shutdown'});

      fake.finishExit(0);
      await shutdown;

      expect(controller.coreRunning, isFalse);
      expect(fake.killed, isFalse);
    });

    test('shutdown 超时后 kill 侧车', () async {
      final fake = FakeSidecarTransport();
      final controller = await _createController(
        fake,
        shutdownTimeout: const Duration(milliseconds: 20),
        killTimeout: const Duration(milliseconds: 20),
      );

      await controller.shutdown();

      expect(fake.lastCommand['command'], 'shutdown');
      expect(fake.killed, isTrue);
      expect(controller.coreRunning, isFalse);
    });

    test('dispose 会触发 shutdown 命令', () async {
      final fake = FakeSidecarTransport();
      final controller = WindowsAppController(
        transport: fake,
        shutdownTimeout: const Duration(milliseconds: 20),
        killTimeout: const Duration(milliseconds: 20),
      );
      await controller.initialize();

      controller.dispose();
      await pumpEventQueue();

      expect(fake.sentLines, isNotEmpty);
      expect(fake.lastCommand['command'], 'shutdown');
      await controller.shutdown();
    });

    test('侧车 exe 缺失时给出中文错误且不崩溃', () async {
      final missingPath =
          '${Directory.systemTemp.path}${Platform.pathSeparator}'
          'definitely-missing-FusionPlayCore.exe';
      final controller = WindowsAppController(executablePath: missingPath);
      addTearDown(controller.dispose);

      await controller.initialize();

      expect(controller.coreRunning, isFalse);
      expect(
        controller.state.lastError,
        contains('未找到 FusionPlayCore.exe（请与 FusionPlay.exe 放在同一目录）'),
      );
    });

    test('默认侧车路径与 UI 可执行文件同目录', () {
      final path = WindowsAppController.defaultExecutablePath();
      expect(path, endsWith('${Platform.pathSeparator}FusionPlayCore.exe'));
      expect(path, startsWith(File(Platform.resolvedExecutable).parent.path));
    });
  });

  group('抽象接口', () {
    test(
      'NativeAppController 与 WindowsAppController 都实现 FusionPlayController',
      () {
        expect(NativeAppController(), isA<FusionPlayController>());
        final fake = FakeSidecarTransport();
        final controller = WindowsAppController(transport: fake);
        addTearDown(() {
          fake.finishExit(0);
          controller.dispose();
        });
        expect(controller, isA<FusionPlayController>());
      },
    );
  });
}
