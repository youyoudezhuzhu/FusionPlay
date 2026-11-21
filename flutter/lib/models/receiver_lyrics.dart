final _stamp = RegExp(r'^\[(\d{1,3}):([0-5]\d)(?:[.:](\d{1,3}))?\]');
final _word = RegExp(r'\[(\d{1,3}):([0-5]\d)(?:[.:](\d{1,3}))?\]\{([^}]*)\}');

int _milliseconds(RegExpMatch match) =>
    (int.parse(match[1]!) * 60 + int.parse(match[2]!)) * 1000 +
    int.parse((match[3] ?? '').padRight(3, '0'));

/// Accept the timed word stream used by Xiaomi alongside LRC and plain text.
Map<String, Object?>? receiverLyrics(String? input) {
  if (input == null || input.trim().isEmpty) return null;
  final lines = <Map<String, Object?>>[];
  var synced = false;
  for (var line in input.split(RegExp(r'\r?\n'))) {
    line = line.trim();
    if (line.isEmpty ||
        RegExp(r'^\[[a-z]+:', caseSensitive: false).hasMatch(line)) {
      continue;
    }
    final tokens = _word.allMatches(line).toList();
    if (tokens.isNotEmpty && tokens.map((m) => m[0]).join() == line) {
      final words = [
        for (final m in tokens)
          if (m[4]!.isNotEmpty) {'startMs': _milliseconds(m), 'text': m[4]!},
      ];
      if (words.isNotEmpty) {
        lines.add({
          'startMs': words.first['startMs'],
          'text': words.map((w) => w['text']).join(),
          'words': words,
        });
        synced = true;
      }
      continue;
    }
    final times = <int>[];
    for (
      var match = _stamp.firstMatch(line);
      match != null;
      match = _stamp.firstMatch(line)
    ) {
      times.add(_milliseconds(match));
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
