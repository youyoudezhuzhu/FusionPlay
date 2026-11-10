import 'dart:async';

import 'package:flutter/services.dart';

import '../models/app_state.dart';
import 'app_controller.dart';

/// Android backend: bridges to the Kotlin runtime through MethodChannel.
class NativeAppController extends FusionPlayController {
  static const _methods = MethodChannel('com.fusionplay.android/runtime');
  static const _events = EventChannel('com.fusionplay.android/runtime_events');

  @override
  AppState state = const AppState();
  StreamSubscription<Object?>? _subscription;

  @override
  Future<void> initialize() async {
    try {
      _subscription = _events.receiveBroadcastStream().listen(
        _acceptState,
        onError: (Object error) => _acceptError(error),
      );
      _acceptState(await _methods.invokeMethod<Object?>('state'));
    } catch (error) {
      _acceptError(error);
    }
  }

  void _acceptState(Object? raw) {
    if (raw is! Map) return;
    final next = AppState.fromMap(raw.cast<Object?, Object?>());
    if (next == state) return;
    state = next;
    notifyListeners();
  }

  @override
  Future<void> setReceiverName(String value) =>
      _invoke('setReceiverName', {'value': value});

  @override
  Future<void> setStartupEnabled(bool value) =>
      _invoke('setStartupEnabled', {'value': value});

  @override
  Future<void> setAutoWakeEnabled(bool value) =>
      _invoke('setAutoWakeEnabled', {'value': value});

  @override
  Future<void> setAdvancedEffectsEnabled(bool value) =>
      _invoke('setAdvancedEffectsEnabled', {'value': value});

  @override
  Future<void> setProtocolEnabled(String protocol, bool value) =>
      _invoke('setProtocolEnabled', {'protocol': protocol, 'value': value});

  @override
  Future<void> setMiPlayDeviceIdentity(MiPlayDeviceIdentity identity) =>
      _invoke('setMiPlayDeviceIdentity', {'value': identity.persistedValue});

  @override
  Future<void> playback(PlaybackCommand command) =>
      _invoke('playback', {'command': command.name});

  @override
  Future<void> seek(int positionMs) =>
      _invoke('seek', {'positionMs': positionMs});

  @override
  Future<void> volume(int percent) => _invoke('volume', {'percent': percent});

  @override
  Future<void> exportLogs() => _invoke('exportLogs');

  @override
  Future<void> clearError() => _invoke('clearError');

  Future<void> _invoke(String method, [Map<String, Object?>? arguments]) async {
    try {
      final response = await _methods.invokeMethod<Object?>(method, arguments);
      _acceptState(response);
    } catch (error) {
      _acceptError(error);
    }
  }

  void _acceptError(Object error) {
    final next = state.copyWith(lastError: error.toString());
    if (next == state) return;
    state = next;
    notifyListeners();
  }

  @override
  void dispose() {
    _subscription?.cancel();
    super.dispose();
  }
}
