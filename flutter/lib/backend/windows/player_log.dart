import 'dart:io';

import '../ui_diagnostics.dart';

/// Remains available even if the Windows sidecar fails to launch.
void recordWindowsPlayerError(String message) {
  if (!Platform.isWindows) return;
  try {
    final base =
        Platform.environment['LOCALAPPDATA'] ??
        (Platform.environment['USERPROFILE'] == null
            ? Directory.systemTemp.path
            : '${Platform.environment['USERPROFILE']}/AppData/Local');
    final dir = Directory('$base/FusionPlay/logs')..createSync(recursive: true);
    final file = File('${dir.path}/player.log');
    final previous = File('${dir.path}/player.previous.log');
    if (file.existsSync() && file.lengthSync() >= 256 * 1024) {
      if (previous.existsSync()) previous.deleteSync();
      file.renameSync(previous.path);
    }
    file.writeAsStringSync(
      '${DateTime.now().toUtc().toIso8601String()} ERROR ${sanitizeDiagnostic(message, 2400)}\n',
      mode: FileMode.append,
    );
  } on FileSystemException {
    // Console reporting is still handled by Flutter.
  }
}
