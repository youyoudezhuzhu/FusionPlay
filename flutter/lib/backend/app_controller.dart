import 'package:flutter/foundation.dart';

import '../models/app_state.dart';
import 'update_service.dart';

/// The UI-facing contract shared by every FusionPlay backend.
///
/// Android talks to the Kotlin runtime through a `MethodChannel`; the Windows
/// desktop build talks to `FusionPlayCore.exe` over NDJSON (see
/// `docs/WINDOWS_ARCHITECTURE.md` §5/§6). Both implement this interface so the
/// Flutter widgets in `lib/ui` and `lib/main.dart` stay platform agnostic.
abstract class FusionPlayController extends ChangeNotifier {
  /// Last state received from the backend.
  AppState get state;

  /// Starts the backend and begins streaming state updates.
  Future<void> initialize();

  Future<void> setReceiverName(String value);

  Future<void> setStartupEnabled(bool value);

  Future<void> setAutoWakeEnabled(bool value);

  Future<void> setAdvancedEffectsEnabled(bool value);

  Future<void> setProtocolEnabled(String protocol, bool value);

  Future<void> setMiPlayDeviceIdentity(MiPlayDeviceIdentity identity);

  Future<void> playback(PlaybackCommand command);

  Future<void> seek(int positionMs);

  Future<void> volume(int percent);

  Future<void> exportLogs();

  Future<void> clearError();

  /// Checks for an in-app update.
  ///
  /// Defaults to "unavailable"; only Android overrides this (see
  /// [AndroidUpdateService]).
  Future<UpdateCheckResult> checkForUpdate() async =>
      const UpdateCheckResult(status: UpdateCheckStatus.unavailable);

  Future<void> downloadUpdate(String key, DownloadProgress progress) =>
      Future.error(UnsupportedError('当前平台不支持在线更新'));

  Future<UpdateInstallResult> installDownloadedUpdate(String key) async =>
      const UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: '当前平台不支持在线更新',
      );

  /// Downloads and starts the system installer for an available update.
  ///
  /// Platforms without an installer (Windows) report an error.
  Future<UpdateInstallResult> downloadAndInstallUpdate() async =>
      const UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: '当前平台不支持在线更新',
      );

  @override
  void dispose();
}
