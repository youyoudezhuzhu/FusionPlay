import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';

/// Injectable wall clock used by [LyricsPlaybackClock] (tests pass a fake).
typedef LyricsClockNow = DateTime Function();

/// Smooths the 10 Hz `playback.positionMs` samples from the sender into a
/// per-frame position for karaoke word highlighting.
///
/// The clock re-anchors on every state sample, extrapolates from the last
/// anchor with a wall clock, ignores small backward samples (jitter) and
/// re-anchors on real seeks. It only drives a [Ticker] (and therefore only
/// notifies) while it is visible and playing.
class LyricsPlaybackClock extends ChangeNotifier {
  LyricsPlaybackClock({
    LyricsClockNow? now,
    this.smallBackwardTolerance = const Duration(milliseconds: 750),
  }) : _now = now ?? DateTime.now;

  final LyricsClockNow _now;

  /// Backward jumps larger than this are treated as a seek instead of jitter.
  final Duration smallBackwardTolerance;

  Ticker? _ticker;
  bool _visible = false;
  bool _playing = false;
  bool _disposed = false;
  int _baseMs = 0;
  DateTime? _anchor;
  int? _durationMs;
  Object? _trackKey;

  /// Extrapolated playback position in milliseconds.
  int get positionMs {
    final duration = _durationMs;
    if (!_playing || _anchor == null) {
      return _clamp(_baseMs, duration);
    }
    final elapsed = _now().difference(_anchor!).inMilliseconds;
    return _clamp(_baseMs + (elapsed < 0 ? 0 : elapsed), duration);
  }

  bool get isPlaying => _playing;

  bool get isVisible => _visible;

  bool get isTicking => _ticker?.isActive ?? false;

  /// The state sample last used to (re-)anchor the clock.
  int get anchoredPositionMs => _baseMs;

  /// Attaches a ticker so the position advances between state samples.
  ///
  /// Call from a [TickerProvider] (the lyrics view keeps the clock local to the
  /// visible lyrics viewport). Tests can omit it and only call [update].
  void attachTicker(TickerProvider vsync) {
    _ticker ??= vsync.createTicker(_onTick);
    _syncTicker();
  }

  /// Marks the lyrics viewport as visible/hidden; hidden clocks stop ticking.
  void setVisible(bool visible) {
    if (_visible == visible) return;
    _visible = visible;
    _syncTicker();
  }

  /// Feeds one `playback.positionMs` sample.
  ///
  /// [trackKey] changes (new track/source) reset the anchor; [durationMs]
  /// clamps the extrapolated position.
  void update({
    required int positionMs,
    required bool isPlaying,
    int? durationMs,
    Object? trackKey,
  }) {
    if (_disposed) return;
    final trackChanged = trackKey != null && trackKey != _trackKey;
    _trackKey = trackKey ?? _trackKey;
    _durationMs = durationMs;

    var anchored = positionMs;
    if (!trackChanged && _playing && isPlaying && _anchor != null) {
      final previous = this.positionMs;
      if (positionMs < previous) {
        final backwards = previous - positionMs;
        if (backwards <= smallBackwardTolerance.inMilliseconds) {
          // Jitter: keep the monotonic extrapolated position.
          anchored = previous;
        }
      }
    }
    _baseMs = _clamp(anchored, durationMs);
    _anchor = _now();
    _playing = isPlaying;
    _syncTicker();
    notifyListeners();
  }

  void _onTick(Duration _) {
    if (_disposed || !_playing) return;
    notifyListeners();
  }

  void _syncTicker() {
    final ticker = _ticker;
    if (ticker == null) return;
    final shouldRun = _visible && _playing && !_disposed;
    if (shouldRun && !ticker.isActive) {
      ticker.start();
    } else if (!shouldRun && ticker.isActive) {
      ticker.stop();
    }
  }

  int _clamp(int value, int? durationMs) {
    if (value < 0) return 0;
    if (durationMs != null && durationMs > 0 && value > durationMs) {
      return durationMs;
    }
    return value;
  }

  @override
  void dispose() {
    _disposed = true;
    _ticker?.dispose();
    _ticker = null;
    super.dispose();
  }
}
