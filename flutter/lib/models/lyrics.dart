import 'package:flutter/foundation.dart';

/// One karaoke word/token inside a lyric line (MiPlay / AirPlay word timing).
class LyricsWord {
  const LyricsWord({required this.startMs, this.endMs, required this.text});

  /// Start of the word in milliseconds.
  final int startMs;

  /// End of the word in milliseconds; may be missing when the sender only
  /// provides word starts.
  final int? endMs;

  final String text;

  factory LyricsWord.fromMap(Map<Object?, Object?> map) => LyricsWord(
    startMs: _integer(map['startMs']) ?? 0,
    endMs: _integer(map['endMs']),
    text: _string(map['text']) ?? '',
  );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LyricsWord &&
          startMs == other.startMs &&
          endMs == other.endMs &&
          text == other.text;

  @override
  int get hashCode => Object.hash(startMs, endMs, text);

  @override
  String toString() => 'LyricsWord($startMs..${endMs ?? '?'} "$text")';
}

/// One line of lyrics. [startMs] is null for untimed/static lyrics.
class LyricLine {
  const LyricLine({this.startMs, required this.text, this.words = const []});

  final int? startMs;
  final String text;
  final List<LyricsWord> words;

  factory LyricLine.fromMap(Map<Object?, Object?> map) => LyricLine(
    startMs: _integer(map['startMs']),
    text: _string(map['text']) ?? '',
    words: (map['words'] as List<Object?>? ?? const <Object?>[])
        .whereType<Map>()
        .map((word) => LyricsWord.fromMap(word.cast<Object?, Object?>()))
        .toList(growable: false),
  );

  bool get isBlank =>
      text.trim().isEmpty && words.every((w) => w.text.trim().isEmpty);

  bool get hasWords => words.any((word) => word.text.trim().isNotEmpty);

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LyricLine &&
          startMs == other.startMs &&
          text == other.text &&
          listEquals(words, other.words);

  @override
  int get hashCode => Object.hash(startMs, text, Object.hashAll(words));

  @override
  String toString() => 'LyricLine(startMs: $startMs, "$text")';
}

/// A parsed lyrics document (`playback.lyrics` in the state map).
class LyricsDocument {
  const LyricsDocument({
    this.origin,
    this.synced = false,
    this.lines = const <LyricLine>[],
  });

  /// `embedded` | `metadata_uri` | `sidecar_uri` when reported.
  final String? origin;

  /// Whether the sender reported timing information.
  final bool synced;

  final List<LyricLine> lines;

  factory LyricsDocument.fromMap(Map<Object?, Object?> map) {
    final synced = map['synced'] as bool? ?? false;
    final parsed = (map['lines'] as List<Object?>? ?? const <Object?>[])
        .whereType<Map>()
        .map((line) => LyricLine.fromMap(line.cast<Object?, Object?>()))
        .toList(growable: true);
    if (synced) {
      // Tolerate unsorted/duplicated sender payloads; untimed lines keep their
      // relative position after the timed ones.
      parsed.sort((a, b) {
        final left = a.startMs;
        final right = b.startMs;
        if (left == null && right == null) return 0;
        if (left == null) return 1;
        if (right == null) return -1;
        return left.compareTo(right);
      });
    }
    return LyricsDocument(
      origin: _string(map['origin']),
      synced: synced,
      lines: List<LyricLine>.unmodifiable(parsed),
    );
  }

  /// True when there is at least one non-empty line or word.
  bool get hasLyrics => lines.any((line) => !line.isBlank);

  /// True when timing can be used for follow/highlight.
  bool get isSynced => synced && lines.any((line) => line.startMs != null);

  /// True when at least one line carries word timings.
  bool get hasWordTiming =>
      isSynced && lines.any((line) => line.startMs != null && line.hasWords);

  LyricsDocument copyWith({
    String? origin,
    bool? synced,
    List<LyricLine>? lines,
  }) => LyricsDocument(
    origin: origin ?? this.origin,
    synced: synced ?? this.synced,
    lines: lines ?? this.lines,
  );

  /// Index of the line displayed at [positionMs].
  ///
  /// Returns -1 when the document is not synced; clamps to the first/last
  /// timed line so the UI never loses its highlight at the edges.
  int activeLineIndex(int positionMs) {
    if (!isSynced) return -1;
    var low = 0;
    var high = lines.length - 1;
    var result = -1;
    while (low <= high) {
      final mid = (low + high) >> 1;
      final start = _latestTimedStartAt(mid);
      if (start != null && start <= positionMs) {
        result = mid;
        low = mid + 1;
      } else {
        high = mid - 1;
      }
    }
    // Walk back to a timed line (null-start lines can trail the last timed one).
    while (result >= 0 && lines[result].startMs == null) {
      result--;
    }
    if (result >= 0) return result;
    final firstTimed = lines.indexWhere((line) => line.startMs != null);
    return firstTimed;
  }

  /// Start of the next line that has timing information after [lineIndex].
  int? nextTimedLineStartMs(int lineIndex) {
    for (var index = lineIndex + 1; index < lines.length; index++) {
      final start = lines[index].startMs;
      if (start != null) return start;
    }
    return null;
  }

  /// End of a line used as fallback for the last word.
  int? lineEndMs(int lineIndex, {int? durationMs}) {
    if (lineIndex < 0 || lineIndex >= lines.length) return null;
    final line = lines[lineIndex];
    final start = line.startMs;
    if (start == null) return null;
    final next = nextTimedLineStartMs(lineIndex);
    if (next != null && next > start) return next;
    final duration = durationMs;
    if (duration != null && duration > start) return duration;
    return start + 4000;
  }

  /// Fill progress (0..1) of `lines[lineIndex].words[wordIndex]` at
  /// [positionMs]. Handles missing `endMs` by falling back to the next word,
  /// then the next line, then [durationMs].
  double wordProgressAt(
    int lineIndex,
    int wordIndex,
    int positionMs, {
    int? durationMs,
  }) {
    if (lineIndex < 0 || lineIndex >= lines.length) return 0;
    final line = lines[lineIndex];
    if (wordIndex < 0 || wordIndex >= line.words.length) return 0;
    final word = line.words[wordIndex];
    final end =
        word.endMs ??
        _wordFallbackEndMs(line, lineIndex, wordIndex, durationMs);
    final start = word.startMs;
    if (end <= start) return positionMs >= start ? 1 : 0;
    return ((positionMs - start) / (end - start)).clamp(0.0, 1.0).toDouble();
  }

  int _wordFallbackEndMs(
    LyricLine line,
    int lineIndex,
    int wordIndex,
    int? durationMs,
  ) {
    final start = line.words[wordIndex].startMs;
    for (var index = wordIndex + 1; index < line.words.length; index++) {
      final next = line.words[index].startMs;
      if (next > start) return next;
    }
    final nextLine = nextTimedLineStartMs(lineIndex);
    if (nextLine != null && nextLine > start) return nextLine;
    final duration = durationMs;
    if (duration != null && duration > start) return duration;
    return start + 4000;
  }

  int? _latestTimedStartAt(int index) {
    for (var cursor = index; cursor >= 0; cursor--) {
      final start = lines[cursor].startMs;
      if (start != null) return start;
    }
    return null;
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LyricsDocument &&
          origin == other.origin &&
          synced == other.synced &&
          listEquals(lines, other.lines);

  @override
  int get hashCode => Object.hash(origin, synced, Object.hashAll(lines));

  @override
  String toString() =>
      'LyricsDocument(origin: $origin, synced: $synced, lines: ${lines.length})';
}

Map<Object?, Object?> _map(Object? value) =>
    value is Map ? value.cast<Object?, Object?>() : const <Object?, Object?>{};

String? _string(Object? value) {
  final text = value?.toString();
  if (text == null) return null;
  return text;
}

int? _integer(Object? value) =>
    value is num ? value.round() : int.tryParse(value?.toString() ?? '');

/// Parses a `playback.lyrics` payload, tolerating missing/null entries.
LyricsDocument? parseLyrics(Object? value) {
  if (value is! Map) return null;
  final map = _map(value);
  final lines = map['lines'];
  if (lines is! List || lines.isEmpty) {
    // A document with no lines is treated as "no lyrics" so the UI can show
    // the empty state instead of an empty scroll view.
    return null;
  }
  return LyricsDocument.fromMap(map);
}
