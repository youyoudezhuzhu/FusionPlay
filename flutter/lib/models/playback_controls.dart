/// Per-source transport capabilities reported by the host (`stateMap.controls`).
///
/// AirPlay/DLNA only expose skip/seek when the sender advertises the matching
/// remote commands; MiPlay speakers support reverse control for all of them.
/// When the host does not report `controls` at all (old Windows builds, legacy
/// states) [PlaybackControls.fallback] derives a sensible value.
class PlaybackControls {
  const PlaybackControls({
    this.playPause = true,
    this.previous = true,
    this.next = true,
    this.seek = true,
  });

  final bool playPause;
  final bool previous;
  final bool next;
  final bool seek;

  static const all = PlaybackControls();

  static const none = PlaybackControls(
    playPause: false,
    previous: false,
    next: false,
    seek: false,
  );

  factory PlaybackControls.fromMap(Map<Object?, Object?> map) =>
      PlaybackControls(
        playPause: map['playPause'] as bool? ?? true,
        previous: map['previous'] as bool? ?? true,
        next: map['next'] as bool? ?? true,
        seek: map['seek'] as bool? ?? true,
      );

  /// Derives capabilities for states that do not carry an explicit `controls`
  /// map:
  /// * MiPlay / 小米妙播: reverse control supports play-pause, skip and seek.
  /// * AirPlay / DLNA: only what `remoteControl.commands` advertises.
  /// * No protocol at all (old Windows / placeholder state): keep the legacy
  ///   permissive behaviour so existing remote UI stays usable.
  factory PlaybackControls.fallback({
    String? protocol,
    bool remoteControlAvailable = false,
    Set<String> remoteCommands = const <String>{},
    int? durationMs,
    bool hasPlayback = false,
  }) {
    final normalized = protocol?.trim().toLowerCase() ?? '';
    final miPlay =
        normalized.contains('miplay') ||
        normalized.contains('xiaomi') ||
        protocol?.trim() == '小米妙播';
    if (miPlay) return PlaybackControls.all;

    final canSeek = durationMs != null && durationMs > 0;
    if (normalized.isEmpty && !remoteControlAvailable) {
      return PlaybackControls(
        playPause: hasPlayback,
        previous: hasPlayback,
        next: hasPlayback,
        seek: canSeek,
      );
    }
    if (remoteControlAvailable) {
      return PlaybackControls(
        playPause: hasPlayback,
        previous:
            remoteCommands.contains('previous') ||
            remoteCommands.contains('previous_track'),
        next:
            remoteCommands.contains('next') ||
            remoteCommands.contains('next_track'),
        seek: canSeek,
      );
    }
    return PlaybackControls(
      playPause: hasPlayback,
      previous: false,
      next: false,
      seek: canSeek,
    );
  }

  PlaybackControls copyWith({
    bool? playPause,
    bool? previous,
    bool? next,
    bool? seek,
  }) => PlaybackControls(
    playPause: playPause ?? this.playPause,
    previous: previous ?? this.previous,
    next: next ?? this.next,
    seek: seek ?? this.seek,
  );

  bool get hasSkip => previous || next;

  bool get any => playPause || previous || next || seek;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is PlaybackControls &&
          playPause == other.playPause &&
          previous == other.previous &&
          next == other.next &&
          seek == other.seek;

  @override
  int get hashCode => Object.hash(playPause, previous, next, seek);

  @override
  String toString() =>
      'PlaybackControls(playPause: $playPause, previous: $previous, '
      'next: $next, seek: $seek)';
}
