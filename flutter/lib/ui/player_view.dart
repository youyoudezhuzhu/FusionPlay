import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models/app_state.dart';
import '../theme/fusion_theme.dart';
import 'animated_press_scale.dart';
import 'artwork.dart';
import 'floating_album_art.dart';
import 'g2_shape.dart';
import 'liquid_glass.dart';

import 'lyrics_view.dart';
import 'remote_focus.dart';
import 'rounded_progress_bar.dart';
import 'vector_icon.dart';

class _AirPlayTitleCapture {
  const _AirPlayTitleCapture(this.text);

  final String text;
}

/// Advance an AirPlay progress anchor by wall-clock elapsed time.
///
/// AirPlay now-playing updates are sparse, so the UI must interpolate between
/// them; this pure helper is covered by a unit test.
int airPlayInterpolatedPositionMs({
  required int anchorMs,
  required Duration elapsed,
  int? durationMs,
}) {
  var next = anchorMs + elapsed.inMilliseconds.clamp(0, 1 << 30);
  if (durationMs != null && durationMs > 0) {
    next = next.clamp(0, durationMs);
  }
  return next;
}

/// YAQMC-style player surface: full-bleed artwork background (owned by the
/// host), top bar with source chips + liquid-glass action menu trigger, a large
/// artwork/title column plus a large lyrics column, and a floating glass
/// transport dock with progress and volume.
class PlayerView extends StatefulWidget {
  const PlayerView({
    super.key,
    required this.state,
    required this.onPlayback,
    required this.onSeek,
    required this.onVolume,
    this.artworkSource,
    this.artworkIdentity,
    this.progressFocusNode,
    this.settingsFocusNode,
    this.menuFocusNode,
    this.onOpenMenu,
    this.menuOpen = false,
    this.overlayOpen = false,
    this.menuButtonKey,
    this.volumeFocusNode,
    this.forceGlassFallback = false,
  });

  final AppState state;
  final ValueChanged<PlaybackCommand> onPlayback;
  final ValueChanged<int> onSeek;
  final ValueChanged<int> onVolume;
  final String? artworkSource;
  final String? artworkIdentity;
  final FocusNode? progressFocusNode;

  /// Legacy menu-trigger focus node (kept for existing call sites).
  final FocusNode? settingsFocusNode;

  /// Focus node of the three-dot trigger in the top bar.
  final FocusNode? menuFocusNode;

  /// Opens the action menu; the trigger is hidden when null.
  final VoidCallback? onOpenMenu;

  final bool menuOpen;

  /// True while the morph overlay (menu/settings/closing) is on screen. The
  /// three-dot trigger fades/shrinks out so the overlay appears to absorb it.
  final bool overlayOpen;

  /// Optional key for the three-dot trigger; the morph overlay reads its rect.
  final GlobalKey? menuButtonKey;

  final FocusNode? volumeFocusNode;

  /// Test hook: render LiquidGlass surfaces without the backdrop shader.
  final bool forceGlassFallback;

  @override
  State<PlayerView> createState() => _PlayerViewState();
}

class _PlayerViewState extends State<PlayerView> {
  final FocusNode _fallbackProgressFocus = FocusNode(
    debugLabel: 'player-progress',
  );
  final FocusNode _fallbackMenuFocus = FocusNode(debugLabel: 'player-menu');
  final FocusNode _previousFocus = FocusNode(debugLabel: 'player-previous');
  final FocusNode _playPauseFocus = FocusNode(debugLabel: 'player-play-pause');
  final FocusNode _nextFocus = FocusNode(debugLabel: 'player-next');
  final FocusNode _fallbackVolumeFocus = FocusNode(debugLabel: 'player-volume');
  final ValueNotifier<bool> _previousPressed = ValueNotifier(false);
  final ValueNotifier<bool> _playPausePressed = ValueNotifier(false);
  final ValueNotifier<bool> _nextPressed = ValueNotifier(false);
  final ValueNotifier<bool> _menuPressed = ValueNotifier(false);
  final FocusNode _lyricsToggleFocus = FocusNode(
    debugLabel: 'player-lyrics-toggle',
  );
  final ValueNotifier<bool> _lyricsTogglePressed = ValueNotifier(false);

  /// Whether the lyrics column is shown for the current track. Defaults to
  /// true and resets on track/source changes or when lyrics appear.
  bool _lyricsVisible = true;
  late String _lastTrackKey;
  final List<LyricLine> _streamLyrics = <LyricLine>[];
  String? _streamLastSignature;
  String? _streamSessionKey;

  bool seeking = false;
  double? seekPositionMs;
  int? pendingSeekTargetMs;
  Timer? pendingSeekTimer;
  // AirPlay progress events are sparse (often only when now-playing/lyrics
  // changes), so interpolate between them with the local monotonic wall clock.
  Timer? _progressTicker;
  int _progressAnchorMs = 0;
  DateTime _progressAnchorAt = DateTime.now();
  int _displayedPositionMs = 0;
  int _lastStatePositionMs = 0;
  bool _lastPlaying = false;
  // NetEase (and some other AirPlay senders) put the current lyric line in the
  // now-playing *title* field and the song/composer in artist. Detect a rapidly
  // changing AirPlay title and treat the captured titles as lyrics.
  final List<_AirPlayTitleCapture> _airPlayTitleCaptures = [];
  bool _airPlayTitleLyrics = false;
  String? _lastAirPlayTitle;
  String? _lastAirPlayArtist;
  String? _lastAirPlayAlbum;
  ArtworkFlipDirection artworkFlipDirection = ArtworkFlipDirection.right;
  bool artworkFlipDirectionPending = false;
  Timer? artworkFlipDirectionTimer;
  final List<String> _artworkHistory = [];
  int _artworkHistoryIndex = -1;

  double? _volumeDrag;
  int? _optimisticVolume;
  int? _lastAudibleVolume;
  Timer? _volumeOptimisticTimer;

  FocusNode get _progressFocus =>
      widget.progressFocusNode ?? _fallbackProgressFocus;

  FocusNode get _menuFocus =>
      widget.menuFocusNode ?? widget.settingsFocusNode ?? _fallbackMenuFocus;

  FocusNode get _volumeFocus => widget.volumeFocusNode ?? _fallbackVolumeFocus;

  bool get _reduceMotion =>
      MediaQuery.maybeOf(context)?.disableAnimations ?? false;

  PlaybackControls get _controls => widget.state.controls;

  int get _durationMs => math.max(0, widget.state.playback.durationMs ?? 0);

  bool get _hasPlayback {
    final playback = widget.state.playback;
    return playback.streamActive ||
        (playback.protocol?.trim().isNotEmpty ?? false) ||
        (playback.mediaUrl?.trim().isNotEmpty ?? false) ||
        (playback.title?.trim().isNotEmpty ?? false) ||
        (playback.artist?.trim().isNotEmpty ?? false) ||
        playback.durationMs != null;
  }

  bool get _lyricsSupported =>
      lyricsAvailabilityForProtocol(widget.state.playback.protocol) ==
      LyricsAvailability.supported;

  bool _hasDisplayableLyricsFor(PlaybackSnapshot playback) {
    if (_isAirPlayProtocol(playback) && _airPlayTitleLyrics) return true;
    final lyrics = playback.lyrics;
    return playback.lyricsLoading || (lyrics != null && lyrics.hasLyrics);
  }

  bool get _hasDisplayableLyrics =>
      _hasDisplayableLyricsFor(widget.state.playback);

  bool _isAirPlayProtocol(PlaybackSnapshot playback) =>
      (playback.protocol ?? '').toLowerCase().contains('airplay');

  void _observeAirPlayTitle(PlaybackSnapshot playback) {
    if (!_isAirPlayProtocol(playback)) {
      _airPlayTitleCaptures.clear();
      _airPlayTitleLyrics = false;
      _lastAirPlayTitle = null;
      _lastAirPlayArtist = null;
      _lastAirPlayAlbum = null;
      return;
    }
    final artist = playback.artist?.trim();
    final album = playback.album?.trim();
    if (artist != _lastAirPlayArtist || album != _lastAirPlayAlbum) {
      // New track: restart the title/lyrics history.
      _airPlayTitleCaptures.clear();
      _airPlayTitleLyrics = false;
      _lastAirPlayArtist = artist;
      _lastAirPlayAlbum = album;
      _lastAirPlayTitle = null;
    }
    final title = playback.title?.trim();
    if (title == null || title.isEmpty || title == _lastAirPlayTitle) return;
    _lastAirPlayTitle = title;
    if (_airPlayTitleCaptures.isEmpty ||
        _airPlayTitleCaptures.last.text != title) {
      _airPlayTitleCaptures.add(_AirPlayTitleCapture(title));
      if (_airPlayTitleCaptures.length > 400) {
        _airPlayTitleCaptures.removeAt(0);
      }
    }
    // Three distinct changes is enough to distinguish a lyric line stream from
    // a normal static title. The first one may be the song name.
    if (_airPlayTitleCaptures.length >= 3) {
      _airPlayTitleLyrics = true;
    }
  }

  /// Protocols such as some AirPlay senders only deliver the current line.
  /// Keep a short bounded history keyed by the current playback session; never
  /// invent future lines or word timestamps.
  void _observeStreamLyrics(PlaybackSnapshot playback) {
    final session = _trackKeyOf(playback);
    if (_streamSessionKey != session) {
      _streamSessionKey = session;
      _streamLyrics.clear();
      _streamLastSignature = null;
    }
    final lyrics = playback.lyrics;
    if (lyrics == null || lyrics.synced || lyrics.lines.length > 1) {
      // Full timeline/static document: the normal LyricsView owns display.
      _streamLyrics.clear();
      _streamLastSignature = null;
      return;
    }
    final line = lyrics.lines
        .where((candidate) => !candidate.isBlank)
        .map((candidate) => candidate.text.trim())
        .where((text) => text.isNotEmpty)
        .firstOrNull;
    final source = lyrics.lines
        .where((candidate) => !candidate.isBlank)
        .firstOrNull;
    if (line == null || source == null) return;
    final signature = '${source.startMs}|$line';
    if (signature == _streamLastSignature) return;
    _streamLastSignature = signature;
    _streamLyrics.add(source);
    if (_streamLyrics.length > 7) {
      _streamLyrics.removeRange(0, _streamLyrics.length - 7);
    }
  }

  List<LyricLine> _streamHistoryFor(PlaybackSnapshot playback) {
    if (_isAirPlayProtocol(playback) && _airPlayTitleLyrics) {
      return _airPlayTitleCaptures
          .map((capture) => capture.text.trim())
          .where((text) => text.isNotEmpty)
          .map((text) => LyricLine(text: text))
          .toList(growable: false);
    }
    final lyrics = playback.lyrics;
    if (lyrics != null &&
        !lyrics.synced &&
        lyrics.lines.length <= 1 &&
        _streamLyrics.isNotEmpty) {
      return List<LyricLine>.unmodifiable(_streamLyrics);
    }
    return const <LyricLine>[];
  }

  bool _isStreamingLyrics(PlaybackSnapshot playback) {
    if (_isAirPlayProtocol(playback) && _airPlayTitleLyrics) return true;
    final lyrics = playback.lyrics;
    return lyrics != null &&
        !lyrics.synced &&
        lyrics.lines.length <= 1 &&
        _streamLyrics.isNotEmpty;
  }

  LyricsDocument? _effectiveLyricsFor(PlaybackSnapshot playback) {
    if (_isAirPlayProtocol(playback) && _airPlayTitleLyrics) {
      return LyricsDocument(
        origin: 'airplay_title',
        synced: false,
        lines: _airPlayTitleCaptures
            .map((capture) => LyricLine(text: capture.text))
            .toList(growable: false),
      );
    }
    return playback.lyrics;
  }

  /// Some senders append a composer marker ("作曲", "Composer") to the artist
  /// field. The player shows the performer only.
  String? _cleanArtist(String? raw) {
    final value = raw?.trim();
    if (value == null || value.isEmpty) return null;
    var cleaned = value
        .replaceAll(
          RegExp(r'\s*[·•|｜/]\s*(作曲|Composer)\s*$', caseSensitive: false),
          '',
        )
        .replaceAll(
          RegExp(r'^\s*(作曲|Composer)\s*[·•|｜/]?\s*', caseSensitive: false),
          '',
        )
        .trim();
    if (cleaned.isEmpty) cleaned = value;
    return cleaned;
  }

  (String, String?) _displayMetadata(PlaybackSnapshot playback) {
    if (!_isAirPlayProtocol(playback) || !_airPlayTitleLyrics) {
      final title = playback.title?.trim();
      final artist = playback.artist?.trim();
      return (
        title != null && title.isNotEmpty ? title : '暂无音乐',
        artist != null && artist.isNotEmpty ? artist : null,
      );
    }
    // In AirPlay title-as-lyrics mode the title is a lyric line and artist is
    // normally "song name - composer". Prefer the song name for line one and
    // keep the remaining composer text as line two.
    final artist = playback.artist?.trim();
    if (artist == null || artist.isEmpty) {
      final album = playback.album?.trim();
      return (album != null && album.isNotEmpty ? album : 'AirPlay', null);
    }
    for (final separator in const [' - ', ' – ', ' — ', ' / ', '｜', '|', '·']) {
      final index = artist.indexOf(separator);
      if (index > 0 && index < artist.length - separator.length) {
        final title = artist.substring(0, index).trim();
        final rest = artist.substring(index + separator.length).trim();
        if (title.isNotEmpty) {
          return (title, rest.isEmpty ? null : rest);
        }
      }
    }
    return (artist, null);
  }

  bool get _lyricsToggleVisible => _lyricsSupported && _hasDisplayableLyrics;

  /// Two-column lyrics layout: only when the channel supports lyrics and the
  /// user has not hidden them. While loading we keep the two-column shimmer
  /// placeholder even if the user had hidden lyrics before.
  bool get _showLyricsLayout =>
      _lyricsSupported &&
      _hasDisplayableLyrics &&
      (widget.state.playback.lyricsLoading || _lyricsVisible);

  String _trackKeyOf(PlaybackSnapshot playback) {
    final identity = playback.trackIdentity?.trim();
    if (identity != null && identity.isNotEmpty) return identity;
    final epoch = playback.sourceEpoch;
    if (epoch != null) return 'epoch:$epoch';
    return '${playback.protocol ?? ''}\u001f${playback.title ?? ''}'
        '\u001f${playback.artist ?? ''}\u001f${playback.mediaUrl ?? ''}';
  }

  bool get _playVisible => _hasPlayback && _controls.playPause;

  bool get _previousVisible => _hasPlayback && _controls.previous;

  bool get _nextVisible => _hasPlayback && _controls.next;

  bool get _seekable => _durationMs > 0 && _controls.seek;

  bool get _menuVisible =>
      widget.onOpenMenu != null ||
      widget.menuFocusNode != null ||
      widget.settingsFocusNode != null;

  @override
  void initState() {
    super.initState();
    _lastTrackKey = _trackKeyOf(widget.state.playback);
    _lastStatePositionMs = widget.state.playback.positionMs;
    _displayedPositionMs = _lastStatePositionMs;
    _progressAnchorMs = _lastStatePositionMs;
    _lastPlaying = widget.state.playback.isPlaying;
    _observeAirPlayTitle(widget.state.playback);
    _observeStreamLyrics(widget.state.playback);
    _progressTicker = Timer.periodic(
      const Duration(milliseconds: 250),
      (_) => _tickProgress(),
    );
    final identity = _artworkHistoryIdentity(widget);
    if (identity != null) {
      _artworkHistory.add(identity);
      _artworkHistoryIndex = 0;
    }
  }

  @override
  void didUpdateWidget(covariant PlayerView oldWidget) {
    super.didUpdateWidget(oldWidget);
    // CoreEvent::Volume read-back: once the host echoes the committed value we
    // can drop the optimistic override immediately (same value => no visual
    // jump). A different read-back is ignored until the optimistic window ends
    // so the thumb does not fight the echo mid-gesture.
    final optimistic = _optimisticVolume;
    final externalPercent = widget.state.playback.volumePercent;
    if (optimistic != null &&
        externalPercent != null &&
        externalPercent.clamp(0, 100) == optimistic) {
      _volumeOptimisticTimer?.cancel();
      _volumeOptimisticTimer = null;
      _optimisticVolume = null;
    }
    _observeAirPlayTitle(widget.state.playback);
    _observeStreamLyrics(widget.state.playback);
    final nextTrackKey = _trackKeyOf(widget.state.playback);
    final previousHadLyrics = _hasDisplayableLyricsFor(
      oldWidget.state.playback,
    );
    final nextHasLyrics = _hasDisplayableLyricsFor(widget.state.playback);
    if (nextTrackKey != _lastTrackKey ||
        (!previousHadLyrics && nextHasLyrics)) {
      // New track/source or lyrics became available: show them by default.
      _lyricsVisible = true;
    }
    _lastTrackKey = nextTrackKey;
    final sourceChanged = widget.artworkSource != oldWidget.artworkSource;
    final hasSource = widget.artworkSource?.trim().isNotEmpty ?? false;
    // Metadata identity is only a fallback for sources without artwork bytes
    // (tests/legacy channels). With a real source, only a source change flips.
    final artworkTransitionChanged =
        sourceChanged ||
        (!hasSource && widget.artworkIdentity != oldWidget.artworkIdentity);
    if (artworkTransitionChanged) {
      final identity = _artworkHistoryIdentity(widget);
      if (artworkFlipDirectionPending) {
        if (identity != null) {
          _recordArtworkDirection(identity, artworkFlipDirection);
        }
      } else {
        final reportedDirection =
            switch (widget.state.playback.trackChangeDirection) {
              TrackChangeDirection.previous => ArtworkFlipDirection.right,
              TrackChangeDirection.next => ArtworkFlipDirection.left,
              null => null,
            };
        artworkFlipDirection =
            reportedDirection ??
            (identity == null
                ? ArtworkFlipDirection.left
                : _inferArtworkDirection(identity));
        if (identity != null && reportedDirection != null) {
          _recordArtworkDirection(identity, reportedDirection);
        }
      }
      artworkFlipDirectionPending = false;
      artworkFlipDirectionTimer?.cancel();
    }
    if (!seeking) {
      final pending = pendingSeekTargetMs;
      if (pending == null ||
          (widget.state.playback.positionMs - pending).abs() <= 1500) {
        pendingSeekTimer?.cancel();
        pendingSeekTargetMs = null;
        seekPositionMs = widget.state.playback.positionMs.toDouble();
      } else {
        seekPositionMs = pending.toDouble();
      }
    }
    final playing = widget.state.playback.isPlaying;
    if (!playing || playing != _lastPlaying) {
      _syncProgressAnchor(force: true);
    } else {
      _syncProgressAnchor();
    }
    _lastPlaying = playing;
  }

  bool get _isAirPlayPlayback =>
      (widget.state.playback.protocol ?? '').toLowerCase().contains('airplay');

  void _syncProgressAnchor({bool force = false}) {
    final reported = widget.state.playback.positionMs;
    if (force || reported != _lastStatePositionMs) {
      _lastStatePositionMs = reported;
      _progressAnchorMs = reported;
      _progressAnchorAt = DateTime.now();
      _displayedPositionMs = reported;
    }
  }

  void _tickProgress() {
    if (!mounted) return;
    final playback = widget.state.playback;
    if (!playback.isPlaying ||
        !_isAirPlayPlayback ||
        seeking ||
        pendingSeekTargetMs != null) {
      if (_displayedPositionMs != playback.positionMs) {
        setState(() => _displayedPositionMs = playback.positionMs);
      }
      return;
    }
    final elapsed = DateTime.now().difference(_progressAnchorAt).inMilliseconds;
    int next = _progressAnchorMs + math.max(0, elapsed).toInt();
    final duration = playback.durationMs ?? 0;
    if (duration > 0) next = next.clamp(0, duration).toInt();
    if ((next - _displayedPositionMs).abs() >= 100) {
      setState(() => _displayedPositionMs = next);
    }
  }

  int get _visiblePositionMs {
    if (seeking || pendingSeekTargetMs != null) {
      return (seekPositionMs ?? widget.state.playback.positionMs).round();
    }
    return _isAirPlayPlayback
        ? _displayedPositionMs
        : widget.state.playback.positionMs;
  }

  @override
  void dispose() {
    pendingSeekTimer?.cancel();
    _progressTicker?.cancel();
    artworkFlipDirectionTimer?.cancel();
    _volumeOptimisticTimer?.cancel();
    _fallbackProgressFocus.dispose();
    _fallbackMenuFocus.dispose();
    _previousFocus.dispose();
    _playPauseFocus.dispose();
    _nextFocus.dispose();
    _fallbackVolumeFocus.dispose();
    _previousPressed.dispose();
    _playPausePressed.dispose();
    _nextPressed.dispose();
    _menuPressed.dispose();
    _lyricsToggleFocus.dispose();
    _lyricsTogglePressed.dispose();
    super.dispose();
  }

  String? _artworkHistoryIdentity(PlayerView view) {
    // Prefer the actual artwork source fingerprint. Metadata identities change
    // on every now-playing update (AirPlay especially) even when the cover
    // image is identical, which must not rotate/flip the artwork.
    final source = view.artworkSource?.trim();
    final identity = (source != null && source.isNotEmpty)
        ? source
        : view.artworkIdentity?.trim();
    if (identity == null || identity.isEmpty) return null;
    return identity;
  }

  /// Identity handed to [ArtworkCard]: follows the actual image source when
  /// present, so metadata-only updates never trigger a flip.
  String? get _artworkCardIdentity {
    final source = widget.artworkSource?.trim();
    if (source != null && source.isNotEmpty) return source;
    return widget.artworkIdentity;
  }

  ArtworkFlipDirection _inferArtworkDirection(String identity) {
    if (_artworkHistoryIndex < 0 || _artworkHistory.isEmpty) {
      _artworkHistory
        ..clear()
        ..add(identity);
      _artworkHistoryIndex = 0;
      return ArtworkFlipDirection.left;
    }

    for (var index = _artworkHistoryIndex - 1; index >= 0; index--) {
      if (_artworkHistory[index] == identity) {
        _artworkHistoryIndex = index;
        return ArtworkFlipDirection.right;
      }
    }
    for (
      var index = _artworkHistoryIndex + 1;
      index < _artworkHistory.length;
      index++
    ) {
      if (_artworkHistory[index] == identity) {
        _artworkHistoryIndex = index;
        return ArtworkFlipDirection.left;
      }
    }

    _recordArtworkDirection(identity, ArtworkFlipDirection.left);
    return ArtworkFlipDirection.left;
  }

  void _recordArtworkDirection(
    String identity,
    ArtworkFlipDirection direction,
  ) {
    if (_artworkHistoryIndex < 0 || _artworkHistory.isEmpty) {
      _artworkHistory
        ..clear()
        ..add(identity);
      _artworkHistoryIndex = 0;
      return;
    }
    if (_artworkHistory[_artworkHistoryIndex] == identity) return;

    if (direction == ArtworkFlipDirection.right) {
      if (_artworkHistoryIndex > 0 &&
          _artworkHistory[_artworkHistoryIndex - 1] == identity) {
        _artworkHistoryIndex--;
      } else {
        _artworkHistory.insert(_artworkHistoryIndex, identity);
      }
    } else if (_artworkHistoryIndex + 1 < _artworkHistory.length &&
        _artworkHistory[_artworkHistoryIndex + 1] == identity) {
      _artworkHistoryIndex++;
    } else {
      _artworkHistory.removeRange(
        _artworkHistoryIndex + 1,
        _artworkHistory.length,
      );
      _artworkHistory.add(identity);
      _artworkHistoryIndex++;
    }

    const maximumHistoryLength = 32;
    if (_artworkHistory.length > maximumHistoryLength) {
      final overflow = _artworkHistory.length - maximumHistoryLength;
      _artworkHistory.removeRange(0, overflow);
      _artworkHistoryIndex = (_artworkHistoryIndex - overflow).clamp(
        0,
        _artworkHistory.length - 1,
      );
    }
  }

  @override
  Widget build(BuildContext context) => _liquid21PlayerLayout();

  // ------------------------------------------------------------------
  // Top bar
  // ------------------------------------------------------------------

  void _toggleLyricsVisibility() {
    if (!_lyricsToggleVisible) return;
    setState(() => _lyricsVisible = !_lyricsVisible);
  }

  Widget _chip(String text) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
    decoration: ShapeDecoration(
      color: const Color(0x26FFFFFF),
      shape: StadiumBorder(
        side: BorderSide(color: Colors.white.withValues(alpha: .16)),
      ),
    ),
    child: Text(
      text,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(
        fontSize: 12,
        color: Colors.white.withValues(alpha: .84),
        fontWeight: FusionTypography.mediumFontWeight,
        fontVariations: FusionTypography.medium,
      ),
    ),
  );

  // ------------------------------------------------------------------
  // 21:9 two-column layout (left glass player, right immersive lyrics)
  // ------------------------------------------------------------------

  Widget _liquid21PlayerLayout() {
    final showLyrics = _showLyricsLayout;
    return Column(
      children: [
        _liquid21TopBar(),
        Expanded(
          child: ClipRect(
            child: AnimatedSwitcher(
              key: const ValueKey('player-body-switcher'),
              duration: _reduceMotion
                  ? Duration.zero
                  : const Duration(milliseconds: 300),
              switchInCurve: Curves.easeOutCubic,
              switchOutCurve: Curves.easeInCubic,
              layoutBuilder: (current, previous) => Stack(
                fit: StackFit.expand,
                children: [if (!showLyrics) ...previous, ?current],
              ),
              child: KeyedSubtree(
                key: ValueKey<bool>(showLyrics),
                child: showLyrics
                    ? _liquid21LyricsStage()
                    : _liquid21NoLyricsStage(),
              ),
            ),
          ),
        ),
      ],
    );
  }

  ({double left, double gap, double right}) _liquid21PanelWidths(
    Size available,
  ) {
    final minLeft = math.min(
      260.0,
      available.width * (available.width < 600 ? .46 : .40),
    );
    final maxLeft = math.max(minLeft, available.width * .46);
    // Wider than 16:9: the left panel can take a little more room, but the
    // lyrics column must keep a readable minimum width.
    final preferred = available.width * .40;
    final left = preferred.clamp(minLeft, maxLeft).toDouble();
    final gap = (available.width * .022).clamp(12.0, 28.0).toDouble();
    return (
      left: left,
      gap: gap,
      right: math.max(0, available.width - left - gap),
    );
  }

  Widget _liquid21LyricsStage() => LayoutBuilder(
    key: const ValueKey('player-layout-horizontal'),
    builder: (context, content) {
      if (content.maxHeight > content.maxWidth) {
        return Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Column(
            children: [
              Expanded(child: _liquid21PlayerPanel()),
              Expanded(
                child: _lyricsColumn(
                  activeFontSize: 24,
                  inactiveFontSize: 16,
                  alignCenter: true,
                ),
              ),
            ],
          ),
        );
      }
      final cappedWidth =
          (content.maxWidth > 1400
                  ? math.min(content.maxWidth, 1600)
                  : content.maxWidth)
              .toDouble();
      final widths = _liquid21PanelWidths(Size(cappedWidth, content.maxHeight));
      final gap = widths.gap;
      final leftWidth = widths.left;
      final rightWidth = widths.right;
      final activeFont = (rightWidth * .038).clamp(21.0, 42.0).toDouble();
      return Padding(
        padding: const EdgeInsets.fromLTRB(14, 0, 20, 10),
        child: Center(
          child: SizedBox(
            width: cappedWidth,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(width: leftWidth, child: _liquid21PlayerPanel()),
                SizedBox(width: gap),
                Expanded(
                  child: _lyricsColumn(
                    activeFontSize: activeFont,
                    inactiveFontSize: activeFont * .63,
                    alignCenter: false,
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    },
  );

  Widget _liquid21PlayerPanel() => LayoutBuilder(
    builder: (context, constraints) {
      final panelWidth = constraints.maxWidth;
      final panelHeight = constraints.maxHeight;
      final compact = panelHeight < 560 || panelWidth < 300;
      final dense = compact && panelHeight < 330;
      final padding = compact ? 8.0 : 16.0;
      // Budget for the actual compact controls: title (~21), artist (~15),
      // timeline (40 + time labels), transport (~56), volume (28-30),
      // and symmetric gaps. Avoid the previous 248px over-reservation,
      // which shrank the cover disproportionately on 21:9 phones.
      final controlsBudget = dense ? 204.0 : (compact ? 220.0 : 286.0);
      final cover = math
          .min(
            panelWidth - padding * 2,
            math.max(64.0, panelHeight - padding * 2 - controlsBudget),
          )
          .clamp(64.0, 500.0)
          .toDouble();
      final playback = widget.state.playback;
      final (title, artist) = _displayMetadata(playback);
      final artistLine = _cleanArtist(artist);
      // On short ultrawide displays, stacking artwork above all controls
      // limits the cover to ~114px at 900x400. Use the available horizontal
      // space instead; the right column has its own centered control stack.
      final innerWidth = panelWidth - padding * 2;
      // Reserve the complete compact transport row before allocating artwork.
      final wide = MediaQuery.sizeOf(context).width >= 1500;
      final transportWidth = compact ? 164.0 : (wide ? 250.0 : 214.0);
      final sideCover = math.min(
        innerWidth - transportWidth - 8,
        math.min(innerWidth * .47, panelHeight - padding * 2),
      );
      if (sideCover >= 96) {
        final artworkSize = sideCover;
        return Padding(
          padding: EdgeInsets.all(padding),
          child: Row(
            key: const ValueKey('player-controls-column'),
            children: [
              SizedBox(
                width: artworkSize,
                child: Center(
                  child: _breathingArtwork(
                    size: artworkSize,
                    child: _artworkCard(artworkSize, radius: 16),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      textAlign: TextAlign.center,
                      style: TextStyle(
                        fontSize: compact ? 17 : 24,
                        height: 1.12,
                        color: Colors.white,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    if (artistLine != null && artistLine.isNotEmpty)
                      Text(
                        artistLine,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          fontSize: compact ? 11 : 14,
                          color: Colors.white.withValues(alpha: .64),
                        ),
                      ),
                    const SizedBox(height: 4),
                    _liquidTimeline(compact: compact),
                    const SizedBox(height: 5),
                    Transform.translate(
                      offset: const Offset(0, -5),
                      child: Center(child: _liquidTransport(compact: compact)),
                    ),
                    const SizedBox(height: 5),
                    _liquid21Volume(compact: compact),
                  ],
                ),
              ),
            ],
          ),
        );
      }
      return Padding(
        padding: EdgeInsets.all(padding),
        child: Column(
          key: const ValueKey('player-controls-column'),
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            _breathingArtwork(
              size: cover,
              child: _artworkCard(cover, radius: compact ? 16 : 20),
            ),
            SizedBox(height: compact ? 6 : 12),
            Text(
              title,
              maxLines: compact ? 1 : 2,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
              style: TextStyle(
                fontSize: compact ? 18 : 24,
                height: 1.12,
                color: Colors.white,
                fontWeight: FusionTypography.semiBoldFontWeight,
                fontVariations: FusionTypography.semiBold,
              ),
            ),
            if (artistLine != null && artistLine.isNotEmpty)
              Text(
                artistLine,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: compact ? 11 : 14,
                  color: Colors.white.withValues(alpha: .64),
                ),
              ),
            SizedBox(height: compact ? 1 : 4),
            _liquidTimeline(compact: compact),
            SizedBox(height: dense ? 7 : (compact ? 10 : 16)),
            Center(child: _liquidTransport(compact: compact)),
            SizedBox(height: dense ? 7 : (compact ? 10 : 16)),
            _liquid21Volume(compact: compact),
          ],
        ),
      );
    },
  );

  Widget _liquid21Volume({required bool compact}) {
    final percent = (_displayVolume * 100).round();
    final icon = percent == 0
        ? Icons.volume_off_rounded
        : (percent < 50 ? Icons.volume_down_rounded : Icons.volume_up_rounded);
    return ConstrainedBox(
      constraints: BoxConstraints(
        maxWidth: compact
            ? 360
            : (MediaQuery.sizeOf(context).width >= 1500 ? 560 : 460),
      ),
      child: SizedBox(
        key: const ValueKey('player-volume-capsule'),
        height: compact
            ? (MediaQuery.sizeOf(context).height < 500 ? 28 : 30)
            : 40,
        child: LiquidGlass(
          interactive: true,
          forceFallback: widget.forceGlassFallback,
          borderRadius: 999,
          padding: EdgeInsets.symmetric(horizontal: compact ? 7 : 10),
          child: Row(
            children: [
              InkResponse(
                key: const ValueKey('volume-mute-button'),
                onTap: _toggleMute,
                radius: 16,
                child: Icon(
                  icon,
                  size: compact ? 16 : 18,
                  color: Colors.white70,
                ),
              ),
              const SizedBox(width: 4),
              Expanded(
                child: RoundedProgressBar(
                  key: const ValueKey('volume-slider'),
                  value: _displayVolume,
                  enabled: true,
                  seekEnabled: true,
                  showThumb: true,
                  focusNode: _volumeFocus,
                  upFocusNode: _playVisible ? _playPauseFocus : null,
                  focusBorderColor: Colors.white,
                  activeColor: Colors.white.withValues(alpha: .82),
                  inactiveColor: Colors.white.withValues(alpha: .18),
                  onChanged: (value) => setState(() => _volumeDrag = value),
                  onChangeEnd: (value) => _commitVolume((value * 100).round()),
                ),
              ),
              SizedBox(width: compact ? 5 : 8),
              Text(
                '$percent',
                style: TextStyle(
                  fontSize: compact ? 9 : 10,
                  color: Colors.white.withValues(alpha: .48),
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _liquid21NoLyricsStage() => LayoutBuilder(
    key: const ValueKey('player-layout-horizontal'),
    builder: (context, content) {
      final compact = content.maxHeight < 360;
      final cappedWidth =
          (content.maxWidth > 1400
                  ? math.min(content.maxWidth, 1600)
                  : content.maxWidth)
              .toDouble();
      final groupWidth = math
          .min(cappedWidth * .82, 980.0)
          .clamp(math.min(cappedWidth, 360), cappedWidth)
          .toDouble();
      // Portrait / very narrow: cover above info instead of a cramped row.
      final portrait = cappedWidth < 560;
      // Make the cover the visual anchor without consuming control space.
      final cover = math
          .min(groupWidth * .39, content.maxHeight * (compact ? .72 : .76))
          .clamp(88.0, 420.0)
          .toDouble();
      return Padding(
        key: const ValueKey('player-centered-layout'),
        padding: const EdgeInsets.fromLTRB(18, 0, 18, 10),
        child: Center(
          child: SizedBox(
            width: groupWidth,
            child: portrait
                ? Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      _breathingArtwork(
                        size: cover,
                        child: _artworkCard(cover, radius: 20),
                      ),
                      const SizedBox(height: 14),
                      Expanded(
                        child: _liquid21InfoPanel(
                          compact: true,
                          dense: content.maxHeight < 330,
                        ),
                      ),
                    ],
                  )
                : Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(
                        flex: 5,
                        child: Center(
                          child: _breathingArtwork(
                            size: cover,
                            child: _artworkCard(
                              cover,
                              radius: compact ? 20 : 26,
                            ),
                          ),
                        ),
                      ),
                      SizedBox(width: compact ? 16 : 28),
                      Expanded(
                        flex: 6,
                        child: _liquid21InfoPanel(
                          compact: compact,
                          dense: content.maxHeight < 330,
                        ),
                      ),
                    ],
                  ),
          ),
        ),
      );
    },
  );

  Widget _liquid21InfoPanel({required bool compact, bool dense = false}) {
    final playback = widget.state.playback;
    final (title, artist) = _displayMetadata(playback);
    return Padding(
      padding: EdgeInsets.all(compact ? (dense ? 4 : 6) : 18),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            maxLines: compact ? 1 : 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: compact ? 22 : 32,
              height: 1.1,
              color: Colors.white,
              fontWeight: FusionTypography.semiBoldFontWeight,
              fontVariations: FusionTypography.semiBold,
            ),
          ),
          if (artist != null && artist.isNotEmpty) ...[
            const SizedBox(height: 3),
            Text(
              _cleanArtist(artist) ?? '',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: compact ? 12 : 15,
                color: Colors.white70,
              ),
            ),
          ],
          SizedBox(height: dense ? 2 : (compact ? 6 : 20)),
          _liquidTimeline(compact: compact),
          SizedBox(height: dense ? 7 : (compact ? 10 : 16)),
          Center(child: _liquidTransport(compact: compact)),
          SizedBox(height: dense ? 7 : (compact ? 10 : 16)),
          _liquid21Volume(compact: compact),
        ],
      ),
    );
  }

  Widget _liquid21TopBar() {
    final protocol = playbackProtocolLabel(widget.state.playback.protocol);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 14, 2),
      child: Row(
        children: [
          if (protocol != null) _chip(protocol),
          const Spacer(),
          if (_lyricsToggleVisible)
            _liquidBarIconButton(
              key: const ValueKey('playback-lyrics-toggle-button'),
              focusNode: _lyricsToggleFocus,
              icon: _lyricsVisible
                  ? Icons.subtitles_rounded
                  : Icons.subtitles_off_rounded,
              onTap: _toggleLyricsVisibility,
              upFocusNode: null,
              leftFocusNode: null,
              rightFocusNode: _menuVisible ? _menuFocus : null,
              downFocusNode: _seekable ? _progressFocus : null,
            ),
          if (_menuVisible) ...[
            const SizedBox(width: 4),
            _hiddenMenuWhileOverlay(
              _liquidBarIconButton(
                key: const ValueKey('playback-menu-button'),
                focusNode: _menuFocus,
                icon: Icons.more_horiz_rounded,
                onTap: widget.onOpenMenu,
                menuKey: widget.menuButtonKey,
                autofocus: true,
                leftFocusNode: _lyricsToggleVisible ? _lyricsToggleFocus : null,
                downFocusNode: _seekable ? _progressFocus : null,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _liquidTimeline({required bool compact}) {
    final playback = widget.state.playback;
    final duration = _durationMs;
    final position = _visiblePositionMs.clamp(
      0,
      duration == 0 ? playback.positionMs : duration,
    );
    final progress = duration > 0 ? position / duration : 0.0;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TweenAnimationBuilder<double>(
          tween: Tween(end: progress),
          duration: _reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 220),
          curve: Curves.easeOutCubic,
          builder: (context, value, _) => RoundedProgressBar(
            key: const ValueKey('player-progress-bar'),
            value: value,
            enabled: duration > 0,
            seekEnabled: _seekable,
            showThumb: false,
            focusNode: _progressFocus,
            upFocusNode: _menuVisible ? _menuFocus : null,
            downFocusNode: _playVisible ? _playPauseFocus : null,
            focusBorderColor: Colors.white,
            activeColor: Colors.white,
            inactiveColor: Colors.white.withValues(alpha: .18),
            onChangeStart: (_) => setState(() => seeking = true),
            onChanged: (value) => setState(() {
              seeking = true;
              seekPositionMs = duration * value;
            }),
            onChangeEnd: (value) {
              final target = (duration * value).round();
              setState(() {
                seeking = false;
                seekPositionMs = target.toDouble();
                pendingSeekTargetMs = target;
                _progressAnchorMs = target;
                _progressAnchorAt = DateTime.now();
                _displayedPositionMs = target;
              });
              widget.onSeek(target);
            },
          ),
        ),
        // Move labels closer to the painted track (centered in its 40px
        // hit target) without reducing the seekable touch area.
        SizedBox(height: compact ? 0 : 2),
        Transform.translate(
          offset: Offset(0, compact ? -13 : -11),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                _hasPlayback ? _formatDuration(position) : '0:00',
                style: TextStyle(
                  fontSize: 10.5,
                  color: Colors.white.withValues(alpha: .42),
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
              Text(
                _hasPlayback
                    ? '-${_formatDuration(math.max(0, duration - position))}'
                    : '-0:00',
                style: TextStyle(
                  fontSize: 10.5,
                  color: Colors.white.withValues(alpha: .42),
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _liquidTransport({required bool compact}) {
    final reduceMotion = _reduceMotion;
    final children = <Widget>[];
    if (_previousVisible) {
      children.add(
        _ControlEntrance(
          index: 0,
          reduceMotion: reduceMotion,
          child: _liquidIconButton(
            key: const ValueKey('playback-previous-button'),
            focusNode: _previousFocus,
            icon: FusionIcons.previous,
            onTap: () => _changeTrack(
              PlaybackCommand.previous,
              ArtworkFlipDirection.right,
            ),
            rightFocusNode: _playVisible ? _playPauseFocus : null,
            compact: compact,
          ),
        ),
      );
    }
    if (_playVisible) {
      if (children.isNotEmpty) {
        children.add(SizedBox(width: compact ? 18 : 28));
      }
      children.add(
        _ControlEntrance(
          index: 1,
          reduceMotion: reduceMotion,
          child: _playPauseButton(reduceMotion, compact: compact),
        ),
      );
    }
    if (_nextVisible) {
      if (children.isNotEmpty) {
        children.add(SizedBox(width: compact ? 18 : 28));
      }
      children.add(
        _ControlEntrance(
          index: 2,
          reduceMotion: reduceMotion,
          child: _liquidIconButton(
            key: const ValueKey('playback-next-button'),
            focusNode: _nextFocus,
            icon: FusionIcons.next,
            onTap: () =>
                _changeTrack(PlaybackCommand.next, ArtworkFlipDirection.left),
            leftFocusNode: _playVisible ? _playPauseFocus : null,
            rightFocusNode: _volumeFocus,
            downFocusNode: _volumeFocus,
            compact: compact,
          ),
        ),
      );
    }
    if (children.isEmpty) return const SizedBox(height: 56);
    return Row(mainAxisSize: MainAxisSize.min, children: children);
  }

  Widget _liquidIconButton({
    required Key key,
    required FocusNode focusNode,
    required String icon,
    required VoidCallback onTap,
    FocusNode? leftFocusNode,
    FocusNode? rightFocusNode,
    FocusNode? upFocusNode,
    FocusNode? downFocusNode,
    required bool compact,
  }) {
    final tv = MediaQuery.sizeOf(context).width >= 1500;
    final size = compact ? 34.0 : (tv ? 56.0 : 44.0);
    return AnimatedPressScale(
      key: key,
      externalPressed: focusNode == _previousFocus
          ? _previousPressed
          : _nextPressed,
      child: RemoteFocusFrame(
        focusNode: focusNode,
        onActivate: onTap,
        borderColor: Colors.white,
        shape: const CircleBorder(),
        shortcuts: <ShortcutActivator, Intent>{
          if (leftFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowLeft):
                const DirectionalFocusIntent(TraversalDirection.left),
          if (rightFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowRight):
                const DirectionalFocusIntent(TraversalDirection.right),
          if (upFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowUp):
                const DirectionalFocusIntent(TraversalDirection.up),
          if (downFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowDown):
                const DirectionalFocusIntent(TraversalDirection.down),
        },
        actions: <Type, Action<Intent>>{
          DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
            onInvoke: (intent) {
              switch (intent.direction) {
                case TraversalDirection.left:
                  leftFocusNode?.requestFocus();
                case TraversalDirection.right:
                  rightFocusNode?.requestFocus();
                case TraversalDirection.up:
                  upFocusNode?.requestFocus();
                case TraversalDirection.down:
                  downFocusNode?.requestFocus();
              }
              return null;
            },
          ),
        },
        child: SizedBox.square(
          dimension: size,
          child: LiquidGlass(
            interactive: true,
            forceFallback: widget.forceGlassFallback,
            borderRadius: 999,
            child: Material(
              color: Colors.transparent,
              shape: const CircleBorder(),
              clipBehavior: Clip.antiAlias,
              child: InkResponse(
                onTap: onTap,
                radius: size / 2,
                child: Center(
                  child: FusionVectorIcon(
                    icon,
                    size: compact ? 18 : 22,
                    color: Colors.white.withValues(alpha: .9),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _hiddenMenuWhileOverlay(Widget child) {
    final hidden = widget.overlayOpen;
    final duration = _reduceMotion
        ? Duration.zero
        : const Duration(milliseconds: 140);
    return IgnorePointer(
      ignoring: hidden,
      child: ExcludeFocus(
        excluding: hidden,
        child: AnimatedOpacity(
          opacity: hidden ? 0 : 1,
          duration: duration,
          curve: Curves.easeOutCubic,
          child: AnimatedScale(
            scale: hidden ? .86 : 1,
            duration: duration,
            curve: Curves.easeOutCubic,
            child: child,
          ),
        ),
      ),
    );
  }

  Widget _liquidBarIconButton({
    required Key key,
    required FocusNode focusNode,
    required IconData icon,
    required VoidCallback? onTap,
    GlobalKey? menuKey,
    FocusNode? downFocusNode,
    FocusNode? upFocusNode,
    FocusNode? leftFocusNode,
    FocusNode? rightFocusNode,
    bool autofocus = false,
  }) {
    final button = RemoteFocusFrame(
      autofocus: autofocus,
      focusNode: focusNode,
      onActivate: onTap,
      borderColor: Colors.white,
      shape: const CircleBorder(),
      shortcuts: <ShortcutActivator, Intent>{
        if (downFocusNode != null)
          const SingleActivator(LogicalKeyboardKey.arrowDown):
              const DirectionalFocusIntent(TraversalDirection.down),
        if (upFocusNode != null)
          const SingleActivator(LogicalKeyboardKey.arrowUp):
              const DirectionalFocusIntent(TraversalDirection.up),
        if (leftFocusNode != null)
          const SingleActivator(LogicalKeyboardKey.arrowLeft):
              const DirectionalFocusIntent(TraversalDirection.left),
        if (rightFocusNode != null)
          const SingleActivator(LogicalKeyboardKey.arrowRight):
              const DirectionalFocusIntent(TraversalDirection.right),
      },
      actions: <Type, Action<Intent>>{
        DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
          onInvoke: (intent) {
            switch (intent.direction) {
              case TraversalDirection.down:
                downFocusNode?.requestFocus();
              case TraversalDirection.up:
                upFocusNode?.requestFocus();
              case TraversalDirection.left:
                leftFocusNode?.requestFocus();
              case TraversalDirection.right:
                rightFocusNode?.requestFocus();
            }
            return null;
          },
        ),
      },
      child: Semantics(
        button: true,
        child: LiquidGlass(
          interactive: true,
          forceFallback: widget.forceGlassFallback,
          borderRadius: 999,
          child: IconButton(
            padding: EdgeInsets.zero,
            onPressed: onTap,
            icon: Icon(
              icon,
              size: 19,
              color: Colors.white.withValues(alpha: .90),
            ),
          ),
        ),
      ),
    );
    if (menuKey != null) {
      return SizedBox.square(
        key: menuKey,
        dimension: 34,
        child: SizedBox.square(dimension: 34, key: key, child: button),
      );
    }
    return SizedBox.square(dimension: 34, key: key, child: button);
  }

  // ------------------------------------------------------------------
  // Main content
  // ------------------------------------------------------------------

  Widget _artworkCard(double size, {double radius = 26}) => FloatingAlbumArt(
    size: size,
    radius: radius,
    child: ArtworkCard(
      source: widget.artworkSource,
      size: size,
      hasPlayback: _hasPlayback,
      identity: _artworkCardIdentity,
      flipDirection: artworkFlipDirection,
    ),
  );

  Widget _breathingArtwork({required double size, required Widget child}) {
    final enabled = widget.state.playback.isPlaying && !_reduceMotion;
    return _ArtworkBreath(enabled: enabled, child: child);
  }

  /// Apple Music-style no-lyrics landscape: one large cover with the real
  /// transport/progress/volume column beside it. No empty lyrics column, and no
  /// bottom dock (the same inline controls are used when lyrics are present).

  /// Single-column centered layout used when no lyrics can be displayed:
  /// channel without lyrics (AirPlay), nothing provided this time, or the user
  /// hid the lyrics. It never shows a "waiting/loading" empty state.

  Widget _lyricsColumn({
    required double activeFontSize,
    required double inactiveFontSize,
    bool alignCenter = false,
  }) {
    final playback = widget.state.playback;
    final emptyCopy = lyricsEmptyCopyForChannel(
      playbackChannelForProtocol(playback.protocol),
    );
    if (_isStreamingLyrics(playback)) {
      final history = _streamHistoryFor(playback);
      return _StreamingLyricsView(
        key: const ValueKey('player-lyrics-view'),
        lines: history.length > 7
            ? history.sublist(history.length - 7)
            : history,
        activeFontSize: activeFontSize,
        reduceMotion: _reduceMotion,
      );
    }
    return LyricsView(
      key: const ValueKey('player-lyrics-view'),
      lyrics: _effectiveLyricsFor(playback),
      loading: playback.lyricsLoading,
      isPlaying: playback.isPlaying,
      positionMs: _visiblePositionMs,
      durationMs: playback.durationMs,
      trackKey: _trackKeyOf(playback),
      foreground: Colors.white,
      muted: Colors.white.withValues(alpha: .50),
      activeColor: Colors.white,
      activeFontSize: activeFontSize,
      inactiveFontSize: inactiveFontSize,
      lineGap: (activeFontSize * .30).clamp(7.0, 13.0),
      textShadow: const [Shadow(blurRadius: 18, color: Color(0x66000000))],
      emptyTitle: emptyCopy.title,
      emptyDescription: emptyCopy.description,
      semanticLabel: '歌词',
    );
  }

  // ------------------------------------------------------------------
  // Shared transport pieces
  // ------------------------------------------------------------------

  // ------------------------------------------------------------------
  // Bottom dock
  // ------------------------------------------------------------------

  Widget _playPauseButton(bool reduceMotion, {required bool compact}) {
    final radius = compact ? 23.0 : 29.0;
    final short = MediaQuery.sizeOf(context).height < 400;
    final glyph = short ? 20.0 : (compact ? 24.0 : 30.0);
    final hasPlayback = _hasPlayback;
    return AnimatedPressScale(
      key: const ValueKey('playback-play-pause-button'),
      enabled: hasPlayback,
      externalPressed: _playPausePressed,
      child: RemoteFocusFrame(
        enabled: hasPlayback,
        focusNode: _playPauseFocus,
        onActivate: hasPlayback
            ? () => widget.onPlayback(PlaybackCommand.playPause)
            : null,
        onPressChange: (pressed) => _playPausePressed.value = pressed,
        borderColor: Colors.white,
        shape: G2ContinuousBorder(radius: radius),
        shortcuts: <ShortcutActivator, Intent>{
          const SingleActivator(LogicalKeyboardKey.arrowUp):
              const DirectionalFocusIntent(TraversalDirection.up),
          if (_previousVisible)
            const SingleActivator(LogicalKeyboardKey.arrowLeft):
                const DirectionalFocusIntent(TraversalDirection.left),
          if (_nextVisible)
            const SingleActivator(LogicalKeyboardKey.arrowRight):
                const DirectionalFocusIntent(TraversalDirection.right),
          const SingleActivator(LogicalKeyboardKey.arrowDown):
              const DirectionalFocusIntent(TraversalDirection.down),
        },
        actions: <Type, Action<Intent>>{
          DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
            onInvoke: (intent) {
              final target = switch (intent.direction) {
                TraversalDirection.up =>
                  _lyricsToggleVisible
                      ? _lyricsToggleFocus
                      : (_seekable
                            ? _progressFocus
                            : (_menuVisible ? _menuFocus : null)),
                TraversalDirection.left =>
                  _previousVisible ? _previousFocus : null,
                TraversalDirection.right =>
                  _nextVisible ? _nextFocus : _volumeFocus,
                TraversalDirection.down => _volumeFocus,
              };
              target?.requestFocus();
              return null;
            },
          ),
        },
        child: AnimatedOpacity(
          opacity: hasPlayback ? 1 : .45,
          duration: reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 160),
          child: SizedBox.square(
            dimension: short
                ? 48
                : (compact
                      ? 60
                      : (MediaQuery.sizeOf(context).width >= 1500 ? 82 : 70)),
            // Reference design: the primary action is an opaque white disc.
            // Only previous/next and the volume capsule use refractive glass.
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: const Color(0xF2FFFFFF),
                shape: BoxShape.circle,
                boxShadow: [
                  BoxShadow(
                    color: Colors.black.withValues(alpha: .12),
                    blurRadius: 20,
                    offset: const Offset(0, 5),
                  ),
                ],
              ),
              child: Material(
                color: Colors.transparent,
                shape: const CircleBorder(),
                clipBehavior: Clip.antiAlias,
                child: InkResponse(
                  onTap: hasPlayback
                      ? () => widget.onPlayback(PlaybackCommand.playPause)
                      : null,
                  radius: 60,
                  child: Center(
                    child: AnimatedSwitcher(
                      duration: reduceMotion
                          ? Duration.zero
                          : const Duration(milliseconds: 170),
                      switchInCurve: Curves.easeOutCubic,
                      transitionBuilder: (child, animation) => FadeTransition(
                        opacity: animation,
                        child: ScaleTransition(
                          scale: Tween<double>(begin: .82, end: 1).animate(
                            CurvedAnimation(
                              parent: animation,
                              curve: Curves.easeOutCubic,
                            ),
                          ),
                          child: child,
                        ),
                      ),
                      child: FusionVectorIcon(
                        widget.state.playback.isPlaying
                            ? FusionIcons.pause
                            : FusionIcons.play,
                        key: ValueKey<bool>(widget.state.playback.isPlaying),
                        size: glyph,
                        color: const Color(0xE61A1C22),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  // ------------------------------------------------------------------
  // Volume
  // ------------------------------------------------------------------

  int get _externalVolume =>
      (widget.state.playback.volumePercent ?? 100).clamp(0, 100);

  double get _displayVolume {
    final drag = _volumeDrag;
    if (drag != null) return drag;
    final optimistic = _optimisticVolume;
    return ((optimistic ?? _externalVolume).clamp(0, 100)) / 100;
  }

  void _toggleMute() {
    final current = (_displayVolume * 100).round();
    if (current > 0) {
      _lastAudibleVolume = current;
      _applyVolume(0);
    } else {
      _applyVolume((_lastAudibleVolume ?? 100).clamp(1, 100));
    }
  }

  void _applyVolume(int percent) {
    final clamped = percent.clamp(0, 100);
    setState(() {
      _volumeDrag = null;
      _optimisticVolume = clamped;
    });
    _volumeOptimisticTimer?.cancel();
    _volumeOptimisticTimer = Timer(const Duration(milliseconds: 900), () {
      if (mounted) setState(() => _optimisticVolume = null);
    });
    widget.onVolume(clamped);
  }

  /// Exactly one volume command per finished drag / mute toggle.
  void _commitVolume(int percent) {
    final clamped = percent.clamp(0, 100);
    // Drop duplicate commits for the value already in flight/applied (e.g.
    // tap-without-drag or an onChangeEnd after a RoundedProgressBar nudge).
    if (_volumeDrag == null &&
        _optimisticVolume != null &&
        _optimisticVolume == clamped) {
      return;
    }
    _applyVolume(clamped);
  }

  // ------------------------------------------------------------------
  // Artwork direction helpers
  // ------------------------------------------------------------------

  void _changeTrack(PlaybackCommand command, ArtworkFlipDirection direction) {
    setState(() {
      artworkFlipDirection = direction;
      artworkFlipDirectionPending = true;
    });
    artworkFlipDirectionTimer?.cancel();
    artworkFlipDirectionTimer = Timer(const Duration(seconds: 3), () {
      if (!mounted || !artworkFlipDirectionPending) return;
      setState(() {
        artworkFlipDirection = ArtworkFlipDirection.right;
        artworkFlipDirectionPending = false;
      });
    });
    widget.onPlayback(command);
  }

  String _formatDuration(int milliseconds) {
    final seconds = milliseconds ~/ 1000;
    return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
  }
}

// ----------------------------------------------------------------------
// Small internal widgets
// ----------------------------------------------------------------------

/// One-shot staggered entrance for transport controls.
class _ControlEntrance extends StatelessWidget {
  const _ControlEntrance({
    required this.index,
    required this.reduceMotion,
    required this.child,
  });

  final int index;
  final bool reduceMotion;
  final Widget child;

  @override
  Widget build(BuildContext context) => TweenAnimationBuilder<double>(
    tween: Tween<double>(begin: 0, end: 1),
    duration: reduceMotion ? Duration.zero : const Duration(milliseconds: 300),
    curve: Interval(
      (index * .08).clamp(0.0, .6),
      1,
      curve: Curves.easeOutCubic,
    ),
    builder: (context, reveal, child) => Opacity(
      opacity: reveal,
      child: Transform.translate(
        offset: Offset(0, 5 * (1 - reveal)),
        child: child,
      ),
    ),
    child: child,
  );
}

/// Very light breathing scale for the artwork while playing (≤0.8%).
class _ArtworkBreath extends StatefulWidget {
  const _ArtworkBreath({required this.enabled, required this.child});

  final bool enabled;
  final Widget child;

  @override
  State<_ArtworkBreath> createState() => _ArtworkBreathState();
}

class _ArtworkBreathState extends State<_ArtworkBreath>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 3200),
  );

  @override
  void initState() {
    super.initState();
    if (widget.enabled) _controller.repeat(reverse: true);
  }

  @override
  void didUpdateWidget(covariant _ArtworkBreath oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.enabled == oldWidget.enabled) return;
    if (widget.enabled) {
      _controller.repeat(reverse: true);
    } else {
      _controller.stop();
      _controller.value = 0;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: _controller,
    builder: (context, child) {
      final progress = widget.enabled
          ? Curves.easeInOut.transform(_controller.value)
          : 0.0;
      return Transform.scale(scale: 1 + .008 * progress, child: child);
    },
    child: widget.child,
  );
}

/// Bounded current-line stream used when the sender provides one lyric at a
/// time. Older lines drift up and fade; the newest line stays in the visual
/// focus area. No future lines are fabricated.
class _StreamingLyricsView extends StatelessWidget {
  const _StreamingLyricsView({
    super.key,
    required this.lines,
    required this.activeFontSize,
    required this.reduceMotion,
  });

  final List<LyricLine> lines;
  final double activeFontSize;
  final bool reduceMotion;

  @override
  Widget build(BuildContext context) {
    final visible = lines.length > 7 ? lines.sublist(lines.length - 7) : lines;
    return LayoutBuilder(
      builder: (context, constraints) {
        final children = <Widget>[];
        for (var index = 0; index < visible.length; index++) {
          final isCurrent = index == visible.length - 1;
          final age = visible.length - 1 - index;
          final opacity = isCurrent ? 1.0 : (0.58 - age * .075).clamp(.20, .58);
          final fontSize = isCurrent
              ? activeFontSize
              : (activeFontSize * .62).clamp(13.0, 22.0);
          final duration = reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 260);
          children.add(
            AnimatedOpacity(
              key: ValueKey<String>(
                'stream-line-$index-${visible[index].text}',
              ),
              opacity: opacity,
              duration: duration,
              curve: Curves.easeOutCubic,
              child: AnimatedSlide(
                offset: isCurrent ? Offset.zero : const Offset(0, -.045),
                duration: duration,
                curve: Curves.easeOutCubic,
                child: Padding(
                  padding: EdgeInsets.only(
                    top: isCurrent ? activeFontSize * .15 : 2,
                    bottom: isCurrent ? 2 : activeFontSize * .12,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        visible[index].text,
                        style: TextStyle(
                          color: Colors.white,
                          fontSize: fontSize,
                          height: 1.22,
                          fontWeight: isCurrent
                              ? FusionTypography.semiBoldFontWeight
                              : FusionTypography.regularFontWeight,
                          fontVariations: isCurrent
                              ? FusionTypography.semiBold
                              : FusionTypography.regular,
                          shadows: const [
                            Shadow(blurRadius: 18, color: Color(0x66000000)),
                          ],
                        ),
                      ),
                      if ((visible[index].translation ?? '').trim().isNotEmpty)
                        Text(
                          visible[index].translation!.trim(),
                          style: TextStyle(
                            color: Colors.white.withValues(
                              alpha: isCurrent ? .80 : opacity * .85,
                            ),
                            fontSize: (fontSize * .58).clamp(11.0, 20.0),
                            height: 1.22,
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          );
        }
        return Padding(
          padding: const EdgeInsets.only(right: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Spacer(flex: 3),
              ...children,
              const Spacer(flex: 2),
            ],
          ),
        );
      },
    );
  }
}
