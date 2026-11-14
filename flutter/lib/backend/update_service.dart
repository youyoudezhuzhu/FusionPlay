import 'package:flutter/services.dart';

/// Result of an in-app update check (`com.fusionplay.android/update`).
enum UpdateCheckStatus { available, upToDate, unavailable, error }

/// Payload of the Android `check` update-channel method.
class UpdateCheckResult {
  const UpdateCheckResult({
    required this.status,
    this.versionName,
    this.versionCode,
    this.notes,
    this.publishedAt,
    this.apkSize,
    this.message,
  });

  /// The latest version found on the update server, when `available`.
  final String? versionName;

  /// Monotonic Play-style version code of [versionName].
  final int? versionCode;

  /// Release notes (may be long and multi-line).
  final String? notes;

  /// ISO-8601 publication timestamp, when known.
  final String? publishedAt;

  /// APK size in bytes, when known.
  final int? apkSize;

  /// User-facing hint for `unavailable` / `error` results.
  final String? message;

  final UpdateCheckStatus status;

  factory UpdateCheckResult.fromMap(Map<Object?, Object?> map) {
    final status = switch (_string(map['status'])?.toLowerCase()) {
      'available' => UpdateCheckStatus.available,
      'up_to_date' => UpdateCheckStatus.upToDate,
      'unavailable' => UpdateCheckStatus.unavailable,
      _ => UpdateCheckStatus.error,
    };
    return UpdateCheckResult(
      status: status,
      versionName: _string(map['versionName']),
      versionCode: _integer(map['versionCode']),
      notes: _string(map['notes']),
      publishedAt: _string(map['publishedAt']),
      apkSize: _integer(map['apkSize']),
      message: _string(map['message']),
    );
  }

  @override
  String toString() =>
      'UpdateCheckResult(status: ${status.name}, '
      'versionName: $versionName, versionCode: $versionCode)';
}

/// Result of asking Android to download and start installing an update.
enum UpdateInstallStatus { started, permissionRequired, error }

/// Payload of the Android `downloadAndInstall` update-channel method.
class UpdateInstallResult {
  const UpdateInstallResult({required this.status, this.message});

  final UpdateInstallStatus status;

  /// User-facing hint for `permissionRequired` / `error` results.
  final String? message;

  factory UpdateInstallResult.fromMap(Map<Object?, Object?> map) {
    final status = switch (_string(map['status'])?.toLowerCase()) {
      'started' => UpdateInstallStatus.started,
      'permission_required' => UpdateInstallStatus.permissionRequired,
      _ => UpdateInstallStatus.error,
    };
    return UpdateInstallResult(
      status: status,
      message: _string(map['message']),
    );
  }

  @override
  String toString() => 'UpdateInstallResult(status: ${status.name})';
}

/// Thin client of the Android update MethodChannel.
///
/// The Kotlin side owns the network request, APK download/verification and the
/// system installer intent; Dart only renders the result.
class AndroidUpdateService {
  AndroidUpdateService({MethodChannel? channel})
    : _channel = channel ?? const MethodChannel(channelName);

  static const String channelName = 'com.fusionplay.android/update';

  final MethodChannel _channel;

  Future<UpdateCheckResult> check() async {
    try {
      final response = await _channel.invokeMethod<Object?>('check');
      return UpdateCheckResult.fromMap(_map(response));
    } on MissingPluginException {
      return const UpdateCheckResult(
        status: UpdateCheckStatus.error,
        message: '当前安装包不支持在线更新',
      );
    } on PlatformException catch (error) {
      return UpdateCheckResult(
        status: UpdateCheckStatus.error,
        message: error.message?.trim().isNotEmpty == true
            ? error.message
            : '检查更新失败',
      );
    } catch (error) {
      return UpdateCheckResult(
        status: UpdateCheckStatus.error,
        message: '检查更新失败：$error',
      );
    }
  }

  Future<UpdateInstallResult> downloadAndInstall() async {
    try {
      final response = await _channel.invokeMethod<Object?>(
        'downloadAndInstall',
      );
      return UpdateInstallResult.fromMap(_map(response));
    } on MissingPluginException {
      return const UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: '当前安装包不支持在线更新',
      );
    } on PlatformException catch (error) {
      return UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: error.message?.trim().isNotEmpty == true
            ? error.message
            : '下载更新失败',
      );
    } catch (error) {
      return UpdateInstallResult(
        status: UpdateInstallStatus.error,
        message: '下载更新失败：$error',
      );
    }
  }
}

Map<Object?, Object?> _map(Object? value) =>
    value is Map ? value.cast<Object?, Object?>() : const <Object?, Object?>{};

String? _string(Object? value) {
  final text = value?.toString().trim();
  return text == null || text.isEmpty ? null : text;
}

int? _integer(Object? value) =>
    value is num ? value.round() : int.tryParse(value?.toString() ?? '');
