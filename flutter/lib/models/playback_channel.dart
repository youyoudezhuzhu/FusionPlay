/// Transport/channel of the current playback, derived from `playback.protocol`.
///
/// The raw protocol strings come from different senders ("airplay",
/// "AirPlay", "xiaomi_miplay", "小米妙播", "dlna", ...), so this is a tolerant
/// string classification rather than a strict enum parse.
enum PlaybackChannel { airPlay, dlna, miPlay, other }

PlaybackChannel playbackChannelForProtocol(String? protocol) {
  final value = protocol?.trim() ?? '';
  if (value.isEmpty) return PlaybackChannel.other;
  final normalized = value.toLowerCase();
  if (normalized.contains('miplay') ||
      normalized.contains('xiaomi') ||
      value == '小米妙播') {
    return PlaybackChannel.miPlay;
  }
  if (normalized.contains('airplay')) return PlaybackChannel.airPlay;
  if (normalized.contains('dlna')) return PlaybackChannel.dlna;
  return PlaybackChannel.other;
}

/// User-facing channel label (AirPlay is shown as "AirPlay 2" because the
/// receiver implements the AirPlay 2 protocol).
String? playbackProtocolLabel(String? protocol) {
  final value = protocol?.trim();
  if (value == null || value.isEmpty) return null;
  return switch (playbackChannelForProtocol(protocol)) {
    PlaybackChannel.airPlay => 'AirPlay 2',
    PlaybackChannel.dlna => 'DLNA',
    PlaybackChannel.miPlay => '小米妙播',
    PlaybackChannel.other => value,
  };
}
