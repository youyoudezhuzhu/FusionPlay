import 'dart:async';

import 'package:flutter/foundation.dart';

/// Preserve Flutter's existing reporting while also recording a bounded trace.
void installUiDiagnostics(FutureOr<void> Function(String) record) {
  String? previous;
  DateTime? lastSeen;
  void report(Object error, StackTrace? stack) {
    final message = sanitizeDiagnostic('$error', 800);
    final frames = sanitizeDiagnostic('${stack ?? ''}', 1500);
    final text = '${error.runtimeType}: $message\n$frames';
    final now = DateTime.now();
    if (text == previous &&
        lastSeen != null &&
        now.difference(lastSeen!) < const Duration(seconds: 30)) {
      return;
    }
    previous = text;
    lastSeen = now;
    unawaited(Future<void>.sync(() => record(text)).catchError((Object _) {}));
  }

  final framework = FlutterError.onError;
  FlutterError.onError = (details) {
    report(details.exception, details.stack);
    framework?.call(details);
  };
  final dispatcher = PlatformDispatcher.instance;
  final uncaught = dispatcher.onError;
  dispatcher.onError = (error, stack) {
    report(error, stack);
    return uncaught?.call(error, stack) ?? false;
  };
}

String sanitizeDiagnostic(String text, int limit) {
  final prefix = text.length > limit ? text.substring(0, limit) : text;
  return prefix
      .replaceAll(
        RegExp(
          r'(?:(?:https?|rtsp|file)://|data:)[^\s]+',
          caseSensitive: false,
        ),
        '<URL>',
      )
      .replaceAll(
        RegExp(
          r'(?:authorization|cookie)\s*[:=]\s*[^\r\n]+|(?:password|token|secret)\s*[:=]\s*[^\s,;]+',
          caseSensitive: false,
        ),
        '<redacted>',
      );
}
