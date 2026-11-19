/// Convert sender text or timestamped LRC to the shared player's lyrics model.
Map<String, Object?>? receiverLyrics(String? input) {
  if (input == null || input.trim().isEmpty) return null;
  final lines = <Map<String, Object?>>[];
  final stamp = RegExp(r'^\[(\d+):([0-5]\d)(?:[.:](\d{1,3}))?\]');
  var synced = false;
  for (var line in input.split(RegExp(r'\r?\n'))) {
    line = line.trim();
    if (line.isEmpty ||
        RegExp(r'^\[[a-z]+:', caseSensitive: false).hasMatch(line)) {
      continue;
    }
    final times = <int>[];
    for (
      var match = stamp.firstMatch(line);
      match != null;
      match = stamp.firstMatch(line)
    ) {
      times.add(
        (int.parse(match[1]!) * 60 + int.parse(match[2]!)) * 1000 +
            int.parse((match[3] ?? '').padRight(3, '0')),
      );
      line = line.substring(match.end);
    }
    if (line.isEmpty) continue;
    if (times.isEmpty) {
      lines.add({'text': line});
    } else {
      synced = true;
      for (final ms in times) {
        lines.add({'text': line, 'startMs': ms});
      }
    }
  }
  return lines.isEmpty
      ? null
      : {'origin': 'embedded', 'synced': synced, 'lines': lines};
}
