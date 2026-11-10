import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../models/app_state.dart';
import 'app_controller.dart';

/// Thrown when the sidecar executable is missing or cannot be spawned.
class SidecarLaunchException implements Exception {
  const SidecarLaunchException(this.message);

  final String message;

  @override
  String toString() => message;
}

/// Line-oriented transport to the `FusionPlayCore.exe` sidecar.
///
/// The default implementation is [ProcessSidecarTransport]; tests inject an
/// in-memory implementation so no real executable is required.
abstract class SidecarTransport {
  /// Spawns the sidecar. Throws [SidecarLaunchException] when it cannot start.
  Future<void> start();

  /// NDJSON lines written by the sidecar to stdout.
  ///
  /// Lines emitted before the first listener attaches must be buffered by the
  /// implementation (the controller subscribes right after [start]).
  Stream<String> get outputLines;

  /// Raw tracing lines written by the sidecar to stderr.
  Stream<String> get errorLines;

  /// Completes with the process exit code once the sidecar has exited.
  Future<int> get exited;

  /// Writes one JSON line (`\n` terminated) to the sidecar stdin.
  void sendLine(String line);

  /// Forcefully terminates the sidecar. Best effort; errors are swallowed.
  Future<void> kill();
}

/// Real transport backed by a child process (Windows desktop build).
class ProcessSidecarTransport implements SidecarTransport {
  ProcessSidecarTransport({
    required this.executablePath,
    this.arguments = const <String>[],
  });

  final String executablePath;
  final List<String> arguments;

  final StreamController<String> _output = StreamController<String>();
  final StreamController<String> _error = StreamController<String>();
  final Completer<int> _exited = Completer<int>();

  Process? _process;
  StreamSubscription<String>? _stdoutSubscription;
  StreamSubscription<String>? _stderrSubscription;
  bool _started = false;

  @override
  Stream<String> get outputLines => _output.stream;

  @override
  Stream<String> get errorLines => _error.stream;

  @override
  Future<int> get exited => _exited.future;

  @override
  Future<void> start() async {
    if (_started) return;
    _started = true;
    if (!File(executablePath).existsSync()) {
      throw SidecarLaunchException(
        '未找到 FusionPlayCore.exe（请与 FusionPlay.exe 放在同一目录）：$executablePath',
      );
    }
    try {
      _process = await Process.start(
        executablePath,
        arguments,
        mode: ProcessStartMode.normal,
      );
    } on ProcessException catch (error) {
      throw SidecarLaunchException('无法启动 FusionPlayCore.exe：${error.message}');
    }
    final process = _process!;
    _stdoutSubscription = process.stdout
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen(_output.add, onError: (Object _) {});
    _stderrSubscription = process.stderr
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen(_error.add, onError: (Object _) {});
    unawaited(
      process.exitCode.then((code) {
        if (!_exited.isCompleted) _exited.complete(code);
        unawaited(_stdoutSubscription?.cancel() ?? Future<void>.value());
        unawaited(_stderrSubscription?.cancel() ?? Future<void>.value());
        unawaited(_output.close());
        unawaited(_error.close());
      }),
    );
  }

  @override
  void sendLine(String line) {
    final process = _process;
    if (process == null) return;
    try {
      process.stdin.writeln(line);
    } catch (_) {
      // The sidecar closed its stdin; the exit handler reports the crash.
    }
  }

  @override
  Future<void> kill() async {
    final process = _process;
    if (process == null) return;
    try {
      process.kill();
    } catch (_) {
      // Already gone.
    }
  }
}

/// Payload of the sidecar `ready` event (§5.1).
class SidecarReadyInfo {
  const SidecarReadyInfo({
    required this.app,
    required this.role,
    required this.version,
    required this.dataDir,
    required this.logFile,
    required this.nullAudio,
  });

  final String app;
  final String role;
  final String version;
  final String dataDir;
  final String logFile;
  final bool nullAudio;

  @override
  String toString() =>
      'SidecarReadyInfo(version: $version, dataDir: $dataDir, '
      'logFile: $logFile, nullAudio: $nullAudio)';
}

/// One entry of the sidecar `log` event (§5.3) or a raw stderr line.
class SidecarLogEntry {
  const SidecarLogEntry({
    required this.message,
    this.timestamp,
    this.level = 'info',
    this.source = 'core',
    this.fromStderr = false,
  });

  final DateTime? timestamp;
  final String level;
  final String source;
  final String message;
  final bool fromStderr;

  @override
  String toString() =>
      '[${level.toUpperCase()}][$source]'
      '${timestamp == null ? '' : ' ${timestamp!.toIso8601String()}'} $message';
}

/// Windows backend: owns `FusionPlayCore.exe` and speaks NDJSON over its
/// stdin/stdout (see `docs/WINDOWS_ARCHITECTURE.md` §5/§6).
class WindowsAppController extends ChangeNotifier
    implements FusionPlayController {
  WindowsAppController({
    SidecarTransport? transport,
    String? executablePath,
    this.requestTimeout = const Duration(seconds: 10),
    this.shutdownTimeout = const Duration(seconds: 2),
    this.killTimeout = const Duration(seconds: 1),
    this.initializeTimeout = const Duration(seconds: 3),
  }) : _transport = transport,
       _executablePath = executablePath;

  /// Maximum number of retained sidecar log entries (stdout and stderr).
  static const int maxRetainedLogEntries = 300;

  /// Command name constant of the graceful shutdown command.
  static const String shutdownCommand = 'shutdown';

  /// Absolute path of the sidecar expected next to the running executable.
  static String defaultExecutablePath() {
    final directory = File(Platform.resolvedExecutable).parent.path;
    return '$directory${Platform.pathSeparator}FusionPlayCore.exe';
  }

  /// Maps the Dart playback enum onto the wire values of §6.
  ///
  /// The Windows payload keeps the Dart enum name (`playPause`/`previous`/
  /// `next`), like Android's MethodChannel; the sidecar normalizes
  /// `playPause` to its internal `play_pause` value.
  static String playbackWireValue(PlaybackCommand command) => command.name;

  final Duration requestTimeout;
  final Duration shutdownTimeout;
  final Duration killTimeout;

  /// How long to wait for the sidecar's first event before surfacing
  /// `接收器未响应` (UI startup itself never blocks on it).
  final Duration initializeTimeout;

  SidecarTransport? _transport;
  final String? _executablePath;

  final List<SidecarLogEntry> _logs = <SidecarLogEntry>[];
  final List<SidecarLogEntry> _diagnostics = <SidecarLogEntry>[];
  final List<StreamSubscription<String>> _subscriptions =
      <StreamSubscription<String>>[];
  final Map<String, Completer<void>> _pending = <String, Completer<void>>{};

  @override
  AppState state = const AppState();

  SidecarReadyInfo? _ready;
  bool _coreRunning = false;
  bool _started = false;
  bool _processExited = false;
  bool _disposed = false;
  bool _exitExpected = false;
  bool _fatalReceived = false;
  Timer? _firstEventTimer;
  Future<void>? _shutdownFuture;
  int _requestSequence = 0;

  /// Version/data directory/log file reported by the sidecar `ready` event.
  SidecarReadyInfo? get ready => _ready;

  /// Whether the sidecar process is currently alive.
  bool get coreRunning => _coreRunning;

  /// Most recent sidecar `log` entries (bounded by [maxRetainedLogEntries]).
  List<SidecarLogEntry> get recentLogs =>
      List<SidecarLogEntry>.unmodifiable(_logs);

  /// Most recent stderr (or unparsable stdout) diagnostic lines.
  List<SidecarLogEntry> get recentDiagnostics =>
      List<SidecarLogEntry>.unmodifiable(_diagnostics);

  /// Resolved sidecar executable path (useful for diagnostics/tests).
  String get executablePath => _executablePath ?? defaultExecutablePath();

  SidecarTransport get _resolvedTransport =>
      _transport ??= ProcessSidecarTransport(executablePath: executablePath);

  @override
  Future<void> initialize() async {
    if (_started || _disposed) return;
    final transport = _resolvedTransport;
    try {
      await transport.start();
    } on SidecarLaunchException catch (error) {
      _setCoreRunning(false);
      _setError(error.message);
      return;
    } catch (error) {
      _setCoreRunning(false);
      _setError('无法启动 FusionPlayCore.exe：$error');
      return;
    }
    if (_disposed) {
      // The UI was torn down while the process was spawning.
      await transport.kill();
      return;
    }
    _started = true;
    _setCoreRunning(true);
    _subscriptions.add(
      transport.outputLines.listen(
        _handleOutputLine,
        onError: (Object error) => _appendDiagnostic('stdout', '$error'),
      ),
    );
    _subscriptions.add(
      transport.errorLines.listen(
        (line) => _appendDiagnostic('stderr', line),
        onError: (Object error) => _appendDiagnostic('stderr', '$error'),
      ),
    );
    unawaited(
      transport.exited.then(
        _handleProcessExit,
        onError: (Object _) => _handleProcessExit(-1),
      ),
    );
    _firstEventTimer = Timer(initializeTimeout, () {
      if (_disposed || _processExited || !_coreRunning) return;
      _setError('接收器未响应');
    });
  }

  // ------------------------------------------------------------------
  // Commands (§6)
  // ------------------------------------------------------------------

  @override
  Future<void> setReceiverName(String value) => _sendCommand(
    'setReceiverName',
    fields: <String, Object?>{'value': value},
  );

  @override
  Future<void> setStartupEnabled(bool value) => _sendCommand(
    'setStartupEnabled',
    fields: <String, Object?>{'value': value},
  );

  @override
  Future<void> setAutoWakeEnabled(bool value) => _sendCommand(
    'setAutoWakeEnabled',
    fields: <String, Object?>{'value': value},
  );

  @override
  Future<void> setAdvancedEffectsEnabled(bool value) => _sendCommand(
    'setAdvancedEffectsEnabled',
    fields: <String, Object?>{'value': value},
  );

  @override
  Future<void> setProtocolEnabled(String protocol, bool value) => _sendCommand(
    'setProtocolEnabled',
    fields: <String, Object?>{'protocol': protocol, 'value': value},
  );

  @override
  Future<void> setMiPlayDeviceIdentity(MiPlayDeviceIdentity identity) =>
      _sendCommand(
        'setMiPlayDeviceIdentity',
        fields: <String, Object?>{'value': identity.persistedValue},
      );

  @override
  Future<void> playback(PlaybackCommand command) => _sendCommand(
    'playback',
    // The NDJSON envelope already uses `command` for the command name, so
    // the payload of the Android `playback` method lives in `value` (§6).
    fields: <String, Object?>{'value': playbackWireValue(command)},
  );

  @override
  Future<void> seek(int positionMs) =>
      _sendCommand('seek', fields: <String, Object?>{'positionMs': positionMs});

  @override
  Future<void> volume(int percent) =>
      _sendCommand('volume', fields: <String, Object?>{'percent': percent});

  @override
  Future<void> exportLogs() => _sendCommand('exportLogs');

  @override
  Future<void> clearError() async {
    _clearErrorState();
    // Clearing an error must never produce a new one, even when the sidecar
    // is already gone (the local toast still has to disappear).
    if (!_started || !_coreRunning) return;
    await _sendCommand('clearError');
  }

  Future<void> _sendCommand(
    String command, {
    Map<String, Object?> fields = const <String, Object?>{},
  }) async {
    if (_disposed) return;
    if (!_started || !_coreRunning) {
      _setError('FusionPlayCore 未运行');
      return;
    }
    final payload = <String, Object?>{'command': command, ...fields};
    final requestId = 'c-${++_requestSequence}';
    payload['requestId'] = requestId;
    final completer = Completer<void>();
    _pending[requestId] = completer;
    _resolvedTransport.sendLine(jsonEncode(payload));
    try {
      await completer.future.timeout(requestTimeout);
    } on TimeoutException {
      _pending.remove(requestId);
      _setError('FusionPlayCore 未响应命令：$command');
    }
  }

  // ------------------------------------------------------------------
  // Incoming events (§5)
  // ------------------------------------------------------------------

  void _handleOutputLine(String line) {
    final text = line.trim();
    if (text.isEmpty || _disposed) return;
    Object? decoded;
    try {
      decoded = jsonDecode(text);
    } on FormatException {
      _appendDiagnostic('stdout', text);
      return;
    }
    if (decoded is! Map) {
      _appendDiagnostic('stdout', text);
      return;
    }
    _firstEventTimer?.cancel();
    _firstEventTimer = null;
    final event = decoded.cast<Object?, Object?>();
    switch (event['type']) {
      case 'ready':
        _handleReady(event);
      case 'app_state':
        _handleAppState(event['state']);
      case 'log':
        _handleLog(event);
      case 'response':
        _handleResponse(event);
      case 'fatal':
        _handleFatal(event);
      case 'exiting':
        _handleExiting();
      default:
        _appendDiagnostic('stdout', text);
    }
  }

  void _handleReady(Map<Object?, Object?> event) {
    _ready = SidecarReadyInfo(
      app: event['app']?.toString() ?? 'FusionPlay',
      role: event['role']?.toString() ?? 'core',
      version: event['version']?.toString() ?? '',
      dataDir: event['dataDir']?.toString() ?? '',
      logFile: event['logFile']?.toString() ?? '',
      nullAudio: event['nullAudio'] == true,
    );
    _setCoreRunning(true);
  }

  void _handleAppState(Object? raw) {
    if (raw is! Map) return;
    final next = AppState.fromMap(raw.cast<Object?, Object?>());
    if (next == state) return;
    state = next;
    _notify();
  }

  void _handleLog(Map<Object?, Object?> event) {
    _appendLog(
      SidecarLogEntry(
        timestamp: DateTime.tryParse(event['timestamp']?.toString() ?? ''),
        level: event['level']?.toString() ?? 'info',
        source: event['source']?.toString() ?? 'core',
        message: event['message']?.toString() ?? '',
      ),
    );
  }

  void _handleResponse(Map<Object?, Object?> event) {
    final requestId = event['requestId']?.toString();
    final ok = event['ok'] != false;
    final message = event['message']?.toString();
    if (requestId == null) {
      if (!ok) _setError(message ?? '核心命令执行失败');
      return;
    }
    final completer = _pending.remove(requestId);
    if (!ok) _setError(message ?? '核心命令执行失败');
    if (completer != null && !completer.isCompleted) completer.complete();
  }

  void _handleFatal(Map<Object?, Object?> event) {
    _fatalReceived = true;
    final message = event['message']?.toString();
    _setCoreRunning(false);
    _setError(message ?? 'FusionPlayCore 发生不可恢复错误');
    _failPendingRequests();
  }

  void _handleExiting() {
    _exitExpected = true;
    _setCoreRunning(false);
    _failPendingRequests();
  }

  void _handleProcessExit(int exitCode) {
    if (_processExited) return;
    _processExited = true;
    _firstEventTimer?.cancel();
    _firstEventTimer = null;
    _setCoreRunning(false);
    _failPendingRequests();
    if (!_exitExpected && !_fatalReceived && !_disposed) {
      _setError('FusionPlayCore 已退出（退出码 $exitCode）');
    }
  }

  void _failPendingRequests() {
    final pending = List<Completer<void>>.of(_pending.values);
    _pending.clear();
    for (final completer in pending) {
      if (!completer.isCompleted) completer.complete();
    }
  }

  // ------------------------------------------------------------------
  // Retained diagnostics
  // ------------------------------------------------------------------

  void _appendLog(SidecarLogEntry entry) {
    _logs.add(entry);
    _trim(_logs);
  }

  void _appendDiagnostic(String source, String line) {
    final message = line.trim();
    if (message.isEmpty) return;
    _diagnostics.add(
      SidecarLogEntry(
        level: 'debug',
        source: source,
        message: message,
        fromStderr: source == 'stderr',
      ),
    );
    _trim(_diagnostics);
  }

  void _trim(List<SidecarLogEntry> entries) {
    final overflow = entries.length - maxRetainedLogEntries;
    if (overflow > 0) entries.removeRange(0, overflow);
  }

  // ------------------------------------------------------------------
  // State helpers
  // ------------------------------------------------------------------

  AppState _copyState({
    bool? coreRunning,
    String? lastError,
    bool clearLastError = false,
  }) {
    final current = state;
    return AppState(
      initialized: current.initialized,
      busy: current.busy,
      coreRunning: coreRunning ?? current.coreRunning,
      receiverReady: current.receiverReady,
      receiverPort: current.receiverPort,
      receiverDeviceId: current.receiverDeviceId,
      connectedClient: current.connectedClient,
      activeMediaSource: current.activeMediaSource,
      selectedCoreMediaSource: current.selectedCoreMediaSource,
      receiverName: current.receiverName,
      settings: current.settings,
      playback: current.playback,
      remoteControl: current.remoteControl,
      lastError: clearLastError ? null : (lastError ?? current.lastError),
    );
  }

  void _setCoreRunning(bool value) {
    _coreRunning = value;
    final next = _copyState(coreRunning: value);
    if (next == state) return;
    state = next;
    _notify();
  }

  void _setError(String message) {
    final next = _copyState(lastError: message);
    if (next == state) return;
    state = next;
    _notify();
  }

  void _clearErrorState() {
    if (state.lastError == null) return;
    final next = _copyState(clearLastError: true);
    if (next == state) return;
    state = next;
    _notify();
  }

  void _notify() {
    if (_disposed) return;
    notifyListeners();
  }

  // ------------------------------------------------------------------
  // Lifecycle
  // ------------------------------------------------------------------

  /// Sends `shutdown`, waits up to [shutdownTimeout] for the sidecar to exit,
  /// then kills it. Idempotent and awaited by tests.
  Future<void> shutdown() => _shutdownFuture ??= _runShutdown();

  Future<void> _runShutdown() async {
    _exitExpected = true;
    _firstEventTimer?.cancel();
    _firstEventTimer = null;
    if (!_started || _processExited) {
      _setCoreRunning(false);
      return;
    }
    _resolvedTransport.sendLine(jsonEncode({'command': shutdownCommand}));
    try {
      await _resolvedTransport.exited.timeout(shutdownTimeout);
    } on TimeoutException {
      await _killSidecar();
    } catch (_) {
      // Exit-code future failed; fall through and force-kill below.
      if (!_processExited) await _killSidecar();
    }
    _setCoreRunning(false);
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    _subscriptions.clear();
  }

  Future<void> _killSidecar() async {
    try {
      await _resolvedTransport.kill();
    } catch (_) {
      return;
    }
    try {
      await _resolvedTransport.exited.timeout(killTimeout);
    } catch (_) {
      // The process is unresponsive; nothing else can be done.
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _firstEventTimer?.cancel();
    _firstEventTimer = null;
    unawaited(shutdown());
    super.dispose();
  }
}
