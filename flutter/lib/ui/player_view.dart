import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models/app_state.dart';
import '../theme/fusion_theme.dart';
import 'animated_press_scale.dart';
import 'artwork.dart';
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
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, outer) {
        final landscape = outer.maxWidth > outer.maxHeight;
        // Phones in landscape are only ~350-420 logical px tall; the regular
        // dock alone is ~130px and used to push the empty/centered content
        // under it. Use a denser chrome below 430px.
        final compactChrome = outer.maxHeight < 430;
        final reduceMotion = _reduceMotion;
        final showLyrics = _showLyricsLayout;
        final inlinePlayerControls = landscape;
        final body = landscape
            ? (showLyrics ? _landscapeBody() : _appleNoLyricsBody())
            : (showLyrics ? _portraitBody() : _centeredBody(landscape: false));
        return Column(
          children: [
            _topBar(landscape: landscape, compact: compactChrome),
            Expanded(
              child: ClipRect(
                child: AnimatedSwitcher(
                  key: const ValueKey('player-body-switcher'),
                  duration: reduceMotion
                      ? Duration.zero
                      : const Duration(milliseconds: 300),
                  reverseDuration: reduceMotion
                      ? Duration.zero
                      : const Duration(milliseconds: 240),
                  switchInCurve: Curves.easeOutCubic,
                  switchOutCurve: Curves.easeInCubic,
                  layoutBuilder: (current, previous) => Stack(
                    fit: StackFit.expand,
                    // While the lyrics layout is visible, never keep the old
                    // centered body (which carries the "no lyrics" hint) in
                    // the stack; AirPlay lyrics could otherwise appear next to
                    // a stale "音频流未提供歌词" message.
                    children: [if (!showLyrics) ...previous, ?current],
                  ),
                  transitionBuilder: (child, animation) {
                    if (reduceMotion) return child;
                    final curved = CurvedAnimation(
                      parent: animation,
                      curve: Curves.easeOutCubic,
                    );
                    return FadeTransition(
                      opacity: curved,
                      child: ScaleTransition(
                        scale: Tween<double>(
                          begin: .98,
                          end: 1,
                        ).animate(curved),
                        child: child,
                      ),
                    );
                  },
                  child: KeyedSubtree(
                    key: ValueKey<bool>(showLyrics),
                    child: body,
                  ),
                ),
              ),
            ),
            if (!inlinePlayerControls) _dock(compact: compactChrome),
          ],
        );
      },
    );
  }

  // ------------------------------------------------------------------
  // Top bar
  // ------------------------------------------------------------------

  Widget _topBar({required bool landscape, required bool compact}) {
    final protocol = playbackProtocolLabel(widget.state.playback.protocol);
    final quality = widget.state.playback.qualityText?.trim();
    final qualityLabel = (quality == null || quality.isEmpty)
        ? null
        : quality.split('·').last.trim();
    final reduceMotion = _reduceMotion;
    final entrance = reduceMotion
        ? Duration.zero
        : const Duration(milliseconds: 240);
    return Padding(
      padding: EdgeInsets.fromLTRB(
        compact ? (landscape ? 16 : 12) : (landscape ? 30 : 18),
        compact ? 6 : (landscape ? 16 : 12),
        compact ? 12 : (landscape ? 22 : 12),
        compact ? 2 : 6,
      ),
      child: Row(
        children: [
          Expanded(
            child: TweenAnimationBuilder<double>(
              tween: Tween(begin: 0, end: 1),
              duration: entrance,
              curve: Curves.easeOutCubic,
              builder: (context, reveal, child) => Opacity(
                opacity: reveal,
                child: Transform.translate(
                  offset: Offset(0, 4 * (1 - reveal)),
                  child: child,
                ),
              ),
              child: Wrap(
                spacing: 10,
                runSpacing: 6,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  if (protocol != null) _chip(protocol),
                  if (qualityLabel != null && qualityLabel.isNotEmpty)
                    _chip(qualityLabel),
                ],
              ),
            ),
          ),
          if (_lyricsToggleVisible) ...[
            _lyricsToggleButton(),
            const SizedBox(width: 8),
          ],
          if (_menuVisible) _menuButton(),
        ],
      ),
    );
  }

  Widget _lyricsToggleButton() {
    final active = _lyricsVisible;
    final focusVisible = _menuVisible;
    return AnimatedPressScale(
      key: const ValueKey('playback-lyrics-toggle-button'),
      externalPressed: _lyricsTogglePressed,
      child: RemoteFocusFrame(
        focusNode: _lyricsToggleFocus,
        onActivate: _toggleLyricsVisibility,
        onPressChange: (pressed) => _lyricsTogglePressed.value = pressed,
        borderColor: Colors.white,
        shape: const G2ContinuousBorder(radius: 999),
        shortcuts: <ShortcutActivator, Intent>{
          if (focusVisible)
            const SingleActivator(LogicalKeyboardKey.arrowRight):
                const DirectionalFocusIntent(TraversalDirection.right),
          const SingleActivator(LogicalKeyboardKey.arrowDown):
              const DirectionalFocusIntent(TraversalDirection.down),
        },
        actions: <Type, Action<Intent>>{
          DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
            onInvoke: (intent) {
              final target = switch (intent.direction) {
                TraversalDirection.right => focusVisible ? _menuFocus : null,
                TraversalDirection.down =>
                  _seekable
                      ? _progressFocus
                      : (_playVisible ? _playPauseFocus : null),
                _ => null,
              };
              target?.requestFocus();
              return null;
            },
          ),
        },
        child: LiquidGlass(
          level: GlassLevel.control,
          interactive: true,
          borderRadius: 999,
          tint: active ? const Color(0x3DFFFFFF) : null,
          forceFallback: widget.forceGlassFallback,
          child: Material(
            color: Colors.transparent,
            shape: const StadiumBorder(),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: _toggleLyricsVisibility,
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 9,
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      active
                          ? Icons.subtitles_rounded
                          : Icons.subtitles_off_rounded,
                      size: 18,
                      color: Colors.white.withValues(alpha: active ? .94 : .68),
                    ),
                    const SizedBox(width: 6),
                    Text(
                      '歌词',
                      style: TextStyle(
                        fontSize: 12,
                        color: Colors.white.withValues(
                          alpha: active ? .94 : .66,
                        ),
                        fontWeight: FusionTypography.mediumFontWeight,
                        fontVariations: FusionTypography.medium,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

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

  Widget _menuButton() => _wrapMenuButton(
    _hiddenWhileOverlay(
      AnimatedPressScale(
        key: const ValueKey('playback-menu-button'),
        externalPressed: _menuPressed,
        child: RemoteFocusFrame(
          autofocus: true,
          focusNode: _menuFocus,
          onActivate: widget.onOpenMenu,
          onPressChange: (pressed) => _menuPressed.value = pressed,
          borderColor: Colors.white,
          shape: const G2ContinuousBorder(radius: 999),
          shortcuts: <ShortcutActivator, Intent>{
            const SingleActivator(LogicalKeyboardKey.arrowDown):
                const DirectionalFocusIntent(TraversalDirection.down),
            if (_lyricsToggleVisible)
              const SingleActivator(LogicalKeyboardKey.arrowLeft):
                  const DirectionalFocusIntent(TraversalDirection.left),
          },
          actions: <Type, Action<Intent>>{
            DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
              onInvoke: (intent) {
                if (intent.direction == TraversalDirection.left &&
                    _lyricsToggleVisible) {
                  _lyricsToggleFocus.requestFocus();
                } else {
                  _focusAfterTopBar();
                }
                return null;
              },
            ),
          },
          child: SizedBox.square(
            dimension: 46,
            child: LiquidGlass(
              level: GlassLevel.control,
              interactive: true,
              borderRadius: 999,
              tint: widget.menuOpen ? const Color(0x40FFFFFF) : null,
              forceFallback: widget.forceGlassFallback,
              child: Material(
                color: Colors.transparent,
                shape: const CircleBorder(),
                clipBehavior: Clip.antiAlias,
                child: InkWell(
                  onTap: widget.onOpenMenu,
                  child: Center(
                    child: Icon(
                      Icons.more_horiz_rounded,
                      size: 24,
                      color: Colors.white.withValues(alpha: .94),
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

  Widget _hiddenWhileOverlay(Widget child) {
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

  Widget _wrapMenuButton(Widget button) {
    final key = widget.menuButtonKey;
    if (key == null) return button;
    return SizedBox.square(dimension: 46, key: key, child: button);
  }

  void _focusAfterTopBar() {
    if (_seekable) {
      _progressFocus.requestFocus();
    } else if (_playVisible) {
      _playPauseFocus.requestFocus();
    } else if (_controls.any) {
      _volumeFocus.requestFocus();
    }
  }

  // ------------------------------------------------------------------
  // Main content
  // ------------------------------------------------------------------

  Widget _landscapeBody() {
    return Padding(
      key: const ValueKey('player-layout-horizontal'),
      padding: const EdgeInsets.fromLTRB(22, 2, 22, 16),
      child: LayoutBuilder(
        builder: (context, content) {
          final tight = content.maxHeight < 360;
          final veryTight = content.maxHeight < 280;
          final leftWidth = content.maxWidth * (tight ? .42 : .43);
          final rightWidth = content.maxWidth - leftWidth - (tight ? 18 : 28);
          final lyricsFont = (rightWidth * .035).clamp(22.0, 36.0).toDouble();
          return Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: leftWidth,
                child: RepaintBoundary(
                  child: _appleMusicPlayerColumn(
                    tight: tight,
                    veryTight: veryTight,
                    availableHeight: content.maxHeight,
                    availableWidth: content.maxWidth * (tight ? .42 : .43),
                  ),
                ),
              ),
              SizedBox(width: tight ? 18 : 28),
              Expanded(
                child: _lyricsGlassPanel(
                  activeFontSize: lyricsFont,
                  inactiveFontSize: lyricsFont * .64,
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Widget _appleMusicPlayerColumn({
    required bool tight,
    required bool veryTight,
    required double availableHeight,
    required double availableWidth,
  }) {
    final playback = widget.state.playback;
    final (title, artist) = _displayMetadata(playback);
    final lyricsChip = _lyricsStatusLabel(playback);
    final titleSize = veryTight ? 18.0 : (tight ? 22.0 : 28.0);
    final artistSize = veryTight ? 12.0 : (tight ? 14.0 : 16.0);
    final coverSize = math
        .min(
          MediaQuery.sizeOf(context).width * .24,
          MediaQuery.sizeOf(context).height *
              (veryTight ? .30 : (tight ? .34 : .42)),
        )
        .clamp(74.0, 360.0)
        .toDouble();
    final column = Column(
      key: const ValueKey('apple-music-player-column'),
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        _breathingArtwork(
          size: coverSize,
          child: _artworkCard(coverSize, radius: tight ? 18 : 24),
        ),
        SizedBox(height: veryTight ? 8 : (tight ? 12 : 18)),
        Text(
          title,
          maxLines: veryTight ? 1 : 2,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: titleSize,
            height: 1.15,
            color: Colors.white,
            fontWeight: FusionTypography.semiBoldFontWeight,
            fontVariations: FusionTypography.semiBold,
            shadows: const [Shadow(blurRadius: 22, color: Color(0x66000000))],
          ),
        ),
        if (artist != null && artist.isNotEmpty) ...[
          SizedBox(height: veryTight ? 1 : 4),
          Text(
            artist,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: artistSize,
              color: Colors.white.withValues(alpha: .70),
              fontWeight: FusionTypography.regularFontWeight,
              fontVariations: FusionTypography.regular,
            ),
          ),
        ],
        if (!tight && lyricsChip != null) ...[
          SizedBox(height: 8),
          _chip(lyricsChip),
        ],
        SizedBox(height: veryTight ? 4 : (tight ? 7 : 14)),
        _progressRow(compact: tight),
        SizedBox(height: veryTight ? 3 : (tight ? 6 : 10)),
        _transport(compact: tight),
        SizedBox(height: veryTight ? 2 : (tight ? 5 : 10)),
        _volumeControls(compact: tight),
      ],
    );
    // On 620x320/800x360 the natural control column is a few pixels taller
    // than the body. Scaling the whole column (rather than clipping controls)
    // keeps every button reachable and avoids a yellow overflow stripe.
    // Never clip transport controls: scale the whole player column down when
    // the available landscape height cannot fit the natural stack.
    return FittedBox(
      fit: BoxFit.scaleDown,
      alignment: Alignment.center,
      child: SizedBox(width: availableWidth, child: column),
    );
  }

  Widget _lyricsGlassPanel({
    required double activeFontSize,
    required double inactiveFontSize,
  }) {
    // Lyrics belong to the backdrop, not to a large glass card. No extra
    // full-size BackdropFilter/shader surface and no hard rectangular scrim.
    return RepaintBoundary(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(18, 14, 8, 12),
        child: _lyricsColumn(
          activeFontSize: activeFontSize,
          inactiveFontSize: inactiveFontSize,
          alignCenter: false,
        ),
      ),
    );
  }

  Widget _portraitBody() => LayoutBuilder(
    builder: (context, content) {
      final artwork = (content.maxWidth * .62).clamp(150.0, 300.0).toDouble();
      final lyricsHeight = (content.maxHeight * .32)
          .clamp(160.0, 280.0)
          .toDouble();
      return SingleChildScrollView(
        key: const ValueKey('player-layout-compact'),
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            _leftColumn(artwork: artwork, compact: true, center: true),
            const SizedBox(height: 18),
            SizedBox(
              height: lyricsHeight,
              child: _lyricsColumn(
                activeFontSize: 24,
                inactiveFontSize: 16.5,
                alignCenter: true,
              ),
            ),
          ],
        ),
      );
    },
  );

  Widget _leftColumn({
    required double artwork,
    required bool compact,
    required bool center,
    bool dense = false,
  }) {
    final playback = widget.state.playback;
    final (title, artist) = _displayMetadata(playback);
    final lyricsChip = _lyricsStatusLabel(playback);
    final titleSize = (artwork / 11.5).clamp(
      dense ? 17.0 : (compact ? 20.0 : 24.0),
      32.0,
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: center
          ? CrossAxisAlignment.center
          : CrossAxisAlignment.start,
      children: [
        _breathingArtwork(size: artwork, child: _artworkCard(artwork)),
        SizedBox(height: dense ? 8 : (compact ? 12 : 18)),
        Text(
          title,
          maxLines: dense ? 1 : 2,
          overflow: TextOverflow.ellipsis,
          textAlign: center ? TextAlign.center : TextAlign.start,
          style: TextStyle(
            fontSize: titleSize,
            height: 1.2,
            color: Colors.white,
            fontWeight: FusionTypography.semiBoldFontWeight,
            fontVariations: FusionTypography.semiBold,
            shadows: const [Shadow(blurRadius: 20, color: Color(0x66000000))],
          ),
        ),
        if (!dense && artist != null && artist.isNotEmpty) ...[
          const SizedBox(height: 4),
          Text(
            artist,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: center ? TextAlign.center : TextAlign.start,
            style: TextStyle(
              fontSize: compact ? 14 : 16,
              color: Colors.white.withValues(alpha: .68),
              fontWeight: FusionTypography.regularFontWeight,
              fontVariations: FusionTypography.regular,
            ),
          ),
        ],
        if (!dense && lyricsChip != null) ...[
          const SizedBox(height: 10),
          _chip(lyricsChip),
        ],
      ],
    );
  }

  Widget _artworkCard(double size, {double radius = 26}) => DecoratedBox(
    decoration: BoxDecoration(
      borderRadius: BorderRadius.circular(radius),
      boxShadow: [
        BoxShadow(
          color: Colors.black.withValues(alpha: .42),
          blurRadius: 38,
          spreadRadius: -6,
          offset: const Offset(0, 18),
        ),
      ],
    ),
    child: ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: ArtworkCard(
        source: widget.artworkSource,
        size: size,
        hasPlayback: _hasPlayback,
        identity: _artworkCardIdentity,
        flipDirection: artworkFlipDirection,
      ),
    ),
  );

  Widget _breathingArtwork({required double size, required Widget child}) {
    final enabled = widget.state.playback.isPlaying && !_reduceMotion;
    return _ArtworkBreath(enabled: enabled, child: child);
  }

  String _centeredHint(PlaybackSnapshot playback) {
    final channel = playbackChannelForProtocol(playback.protocol);
    final copy = lyricsEmptyCopyForChannel(channel);
    // Neutral runtime wording: never claim the protocol lacks lyric support.
    if (channel == PlaybackChannel.airPlay) return copy.description;
    return copy.title;
  }

  /// Apple Music-style no-lyrics landscape: one large cover with the real
  /// transport/progress/volume column beside it. No empty lyrics column, and no
  /// bottom dock (the same inline controls are used when lyrics are present).
  Widget _appleNoLyricsBody() {
    final playback = widget.state.playback;
    final (title, artist) = _displayMetadata(playback);
    final hiddenByUser = _lyricsSupported && _hasDisplayableLyrics;
    final hint = hiddenByUser ? null : _centeredHint(playback);
    return Padding(
      key: const ValueKey('player-layout-horizontal'),
      padding: const EdgeInsets.fromLTRB(22, 2, 22, 16),
      child: LayoutBuilder(
        builder: (context, content) {
          final compact = content.maxHeight < 360;
          final cover = math
              .min(content.maxWidth * .34, content.maxHeight * .62)
              .clamp(110.0, 380.0)
              .toDouble();
          final rightWidth = content.maxWidth - cover - (compact ? 24 : 40);
          final column = Column(
            key: const ValueKey('player-centered-layout'),
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                title,
                maxLines: compact ? 1 : 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: compact ? 24.0 : 32.0,
                  height: 1.18,
                  color: Colors.white,
                  fontWeight: FusionTypography.semiBoldFontWeight,
                  fontVariations: FusionTypography.semiBold,
                  shadows: const [
                    Shadow(blurRadius: 22, color: Color(0x66000000)),
                  ],
                ),
              ),
              if (artist != null && artist.isNotEmpty) ...[
                const SizedBox(height: 5),
                Text(
                  artist,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: compact ? 14.0 : 16.0,
                    color: Colors.white.withValues(alpha: .68),
                  ),
                ),
              ],
              if (hint != null) ...[
                const SizedBox(height: 8),
                Text(
                  hint,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    color: Colors.white.withValues(alpha: .48),
                  ),
                ),
              ],
              SizedBox(height: compact ? 16 : 24),
              _progressRow(compact: compact),
              SizedBox(height: compact ? 10 : 16),
              _transport(compact: compact),
              SizedBox(height: compact ? 8 : 14),
              Align(
                alignment: Alignment.centerLeft,
                child: _volumeControls(compact: compact),
              ),
            ],
          );
          return Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                flex: 5,
                child: Center(
                  child: _breathingArtwork(
                    size: cover,
                    child: _artworkCard(cover, radius: compact ? 22 : 30),
                  ),
                ),
              ),
              SizedBox(width: compact ? 24 : 40),
              Expanded(
                flex: 6,
                child: FittedBox(
                  fit: BoxFit.scaleDown,
                  alignment: Alignment.center,
                  child: SizedBox(width: rightWidth, child: column),
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  /// Single-column centered layout used when no lyrics can be displayed:
  /// channel without lyrics (AirPlay), nothing provided this time, or the user
  /// hid the lyrics. It never shows a "waiting/loading" empty state.
  Widget _centeredBody({required bool landscape}) {
    final playback = widget.state.playback;
    final (title, artist) = _displayMetadata(playback);
    final hiddenByUser = _lyricsSupported && _hasDisplayableLyrics;
    final hint = hiddenByUser ? null : _centeredHint(playback);
    return LayoutBuilder(
      builder: (context, content) {
        final available = content.maxHeight;
        // Phone landscape is roughly 350-420 logical px tall once the dock and
        // status bar are accounted for. Size the cover from the space that is
        // actually left after the text block instead of letting it overflow
        // under the dock (which used to clip the channel hint).
        // Keep the artist line visible on 360-high phones; only the extremely
        // short fallback (e.g. split-screen) drops it.
        final ultra = available < 190;
        final compact = available < 360;
        final titleSize = ultra
            ? 17.0
            : (compact ? 21.0 : (available < 520 ? 26.0 : 30.0));
        final artistSize = compact ? 12.0 : 15.0;
        final hintSize = compact ? 11.0 : 12.5;
        final showArtist = artist != null && artist.isNotEmpty && !ultra;
        final titleGap = compact ? 10.0 : 18.0;
        final hintGap = compact ? 4.0 : 8.0;

        var textHeight = titleSize * 1.2;
        if (showArtist) textHeight += (compact ? 3 : 5) + artistSize * 1.2;
        if (hint != null) textHeight += hintGap + hintSize * 1.4;
        final maxArtwork = available - textHeight - titleGap - 16;
        final artwork = maxArtwork >= 72
            ? math
                  .min(
                    math.min(
                      content.maxWidth * (compact ? .30 : .38),
                      maxArtwork,
                    ),
                    360.0,
                  )
                  .clamp(72.0, 360.0)
                  .toDouble()
            : 0.0;

        final column = Column(
          key: const ValueKey('player-centered-layout'),
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            if (artwork > 0)
              _breathingArtwork(size: artwork, child: _artworkCard(artwork))
            else
              Icon(
                Icons.music_note_rounded,
                size: compact ? 40 : 64,
                color: Colors.white.withValues(alpha: .24),
              ),
            SizedBox(height: artwork > 0 ? titleGap : (compact ? 6 : 10)),
            Padding(
              padding: EdgeInsets.symmetric(horizontal: content.maxWidth * .08),
              child: Text(
                title,
                maxLines: compact || ultra ? 1 : 2,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: titleSize,
                  height: 1.2,
                  color: Colors.white,
                  fontWeight: FusionTypography.semiBoldFontWeight,
                  fontVariations: FusionTypography.semiBold,
                  shadows: const [
                    Shadow(blurRadius: 20, color: Color(0x66000000)),
                  ],
                ),
              ),
            ),
            if (showArtist) ...[
              SizedBox(height: compact ? 3 : 5),
              Padding(
                padding: EdgeInsets.symmetric(
                  horizontal: content.maxWidth * .10,
                ),
                child: Text(
                  artist,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: artistSize,
                    color: Colors.white.withValues(alpha: .68),
                    fontWeight: FusionTypography.regularFontWeight,
                    fontVariations: FusionTypography.regular,
                  ),
                ),
              ),
            ],
            if (hint != null) ...[
              SizedBox(height: hintGap),
              Padding(
                padding: EdgeInsets.symmetric(
                  horizontal: content.maxWidth * .10,
                ),
                child: Text(
                  hint,
                  maxLines: ultra ? 1 : 2,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: hintSize,
                    height: 1.4,
                    color: Colors.white.withValues(alpha: .56),
                    fontWeight: FusionTypography.regularFontWeight,
                    fontVariations: FusionTypography.regular,
                  ),
                ),
              ),
            ],
          ],
        );
        return SizedBox.expand(
          key: ValueKey(
            landscape ? 'player-layout-horizontal' : 'player-layout-compact',
          ),
          child: Center(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 2, 16, 6),
              child: column,
            ),
          ),
        );
      },
    );
  }

  Widget _lyricsColumn({
    required double activeFontSize,
    required double inactiveFontSize,
    bool alignCenter = false,
  }) {
    final playback = widget.state.playback;
    final emptyCopy = lyricsEmptyCopyForChannel(
      playbackChannelForProtocol(playback.protocol),
    );
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

  String? _lyricsStatusLabel(PlaybackSnapshot playback) {
    // Availability is decided from the payload, not the protocol: AirPlay can
    // carry lyrics from senders that support it.
    final lyrics = _effectiveLyricsFor(playback);
    if (lyrics != null && lyrics.hasLyrics) {
      if (lyrics.hasWordTiming) return '逐字歌词';
      if (lyrics.isSynced) return '同步歌词';
      return '静态歌词';
    }
    return null;
  }

  // ------------------------------------------------------------------
  // Shared transport pieces
  // ------------------------------------------------------------------

  Widget _progressRow({required bool compact}) {
    final playback = widget.state.playback;
    final duration = _durationMs;
    final position = _visiblePositionMs;
    final clampedPosition = position.clamp(
      0,
      duration == 0 ? playback.positionMs : duration,
    );
    final progress = duration > 0 ? clampedPosition / duration : 0.0;
    final reduceMotion = _reduceMotion;
    return Row(
      key: const ValueKey('player-progress-row'),
      children: [
        _timeLabel(
          _hasPlayback ? _formatDuration(clampedPosition) : '0:00',
          compact: compact,
        ),
        SizedBox(width: compact ? 6 : 10),
        Expanded(
          child: TweenAnimationBuilder<double>(
            tween: Tween(end: progress),
            duration: reduceMotion
                ? Duration.zero
                : const Duration(milliseconds: 220),
            curve: Curves.easeOutCubic,
            builder: (context, animatedProgress, _) => RoundedProgressBar(
              key: const ValueKey('player-progress-bar'),
              value: animatedProgress,
              enabled: duration > 0,
              seekEnabled: _seekable,
              showThumb: false,
              focusNode: _progressFocus,
              upFocusNode: _menuVisible ? _menuFocus : null,
              downFocusNode: _playVisible ? _playPauseFocus : null,
              focusBorderColor: Colors.white,
              activeColor: Colors.white,
              inactiveColor: Colors.white.withValues(alpha: .24),
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
                pendingSeekTimer?.cancel();
                pendingSeekTimer = Timer(
                  const Duration(milliseconds: 3000),
                  () {
                    if (!mounted || pendingSeekTargetMs != target) return;
                    setState(() {
                      pendingSeekTargetMs = null;
                      seekPositionMs = widget.state.playback.positionMs
                          .toDouble();
                    });
                  },
                );
                widget.onSeek(target);
              },
            ),
          ),
        ),
        SizedBox(width: compact ? 6 : 10),
        _timeLabel(
          _hasPlayback ? _formatDuration(duration) : '0:00',
          compact: compact,
        ),
      ],
    );
  }

  // ------------------------------------------------------------------
  // Bottom dock
  // ------------------------------------------------------------------

  Widget _dock({required bool compact}) {
    final playback = widget.state.playback;
    final duration = _durationMs;
    final position = _visiblePositionMs;
    final clampedPosition = position.clamp(
      0,
      duration == 0 ? playback.positionMs : duration,
    );
    final progress = duration > 0 ? clampedPosition / duration : 0.0;
    final reduceMotion = _reduceMotion;
    return Padding(
      key: const ValueKey('player-dock'),
      padding: EdgeInsets.fromLTRB(
        compact ? 10 : 22,
        0,
        compact ? 10 : 22,
        compact ? 2 : 4,
      ),
      child: LiquidGlass(
        level: GlassLevel.panel,
        borderRadius: compact ? 22 : 28,
        blurSigma: compact ? 14 : 18,
        tint: const Color(0x6B101318),
        forceFallback: widget.forceGlassFallback,
        padding: EdgeInsets.fromLTRB(
          compact ? 12 : 20,
          compact ? 2 : 8,
          compact ? 12 : 20,
          // Together with the outer bottom margin this equals the gap above
          // the transport row, so the play button is visually centred between
          // the progress bar and the bottom of the screen.
          compact ? 6 : 12,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              children: [
                _timeLabel(
                  _hasPlayback ? _formatDuration(clampedPosition) : '0:00',
                  compact: compact,
                ),
                SizedBox(width: compact ? 6 : 10),
                Expanded(
                  child: TweenAnimationBuilder<double>(
                    tween: Tween(end: progress),
                    duration: reduceMotion
                        ? Duration.zero
                        : const Duration(milliseconds: 220),
                    curve: Curves.easeOutCubic,
                    builder: (context, animatedProgress, _) =>
                        RoundedProgressBar(
                          key: const ValueKey('player-progress-bar'),
                          value: animatedProgress,
                          enabled: duration > 0,
                          seekEnabled: _seekable,
                          showThumb: false,
                          focusNode: _progressFocus,
                          upFocusNode: _menuVisible ? _menuFocus : null,
                          downFocusNode: _playVisible ? _playPauseFocus : null,
                          focusBorderColor: Colors.white,
                          activeColor: Colors.white,
                          inactiveColor: Colors.white.withValues(alpha: .24),
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
                            pendingSeekTimer?.cancel();
                            pendingSeekTimer = Timer(
                              const Duration(milliseconds: 3000),
                              () {
                                if (!mounted || pendingSeekTargetMs != target) {
                                  return;
                                }
                                setState(() {
                                  pendingSeekTargetMs = null;
                                  seekPositionMs = widget
                                      .state
                                      .playback
                                      .positionMs
                                      .toDouble();
                                });
                              },
                            );
                            widget.onSeek(target);
                          },
                        ),
                  ),
                ),
                SizedBox(width: compact ? 6 : 10),
                _timeLabel(
                  _hasPlayback ? _formatDuration(duration) : '0:00',
                  compact: compact,
                ),
              ],
            ),
            SizedBox(height: compact ? 8 : 16),
            LayoutBuilder(
              builder: (context, dock) {
                final wide = dock.maxWidth >= 420;
                final transport = _transport(compact: compact);
                final volume = _volumeControls(compact: compact);
                if (wide) {
                  return Row(
                    children: [
                      if (!compact)
                        Expanded(
                          child: Align(
                            alignment: Alignment.centerLeft,
                            child: _dockStatus(),
                          ),
                        )
                      else
                        const Spacer(),
                      transport,
                      Expanded(
                        child: Align(
                          alignment: Alignment.centerRight,
                          child: volume,
                        ),
                      ),
                    ],
                  );
                }
                return Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    transport,
                    const SizedBox(height: 2),
                    Align(alignment: Alignment.centerRight, child: volume),
                  ],
                );
              },
            ),
          ],
        ),
      ),
    );
  }

  Widget _timeLabel(String text, {required bool compact}) => SizedBox(
    width: compact ? 36 : 46,
    child: Text(
      text,
      textAlign: TextAlign.center,
      style: TextStyle(
        fontSize: compact ? 10 : 12,
        color: Colors.white.withValues(alpha: .62),
        fontWeight: FusionTypography.mediumFontWeight,
        fontVariations: FusionTypography.medium,
      ),
    ),
  );

  Widget _dockStatus() {
    final playback = widget.state.playback;
    final status = _lyricsStatusLabel(playback) ?? '';
    if (status.isEmpty) return const SizedBox.shrink();
    return Text(
      status,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(
        fontSize: 11,
        color: Colors.white.withValues(alpha: .46),
        letterSpacing: .6,
        fontWeight: FusionTypography.mediumFontWeight,
        fontVariations: FusionTypography.medium,
      ),
    );
  }

  Widget _transport({required bool compact}) {
    final reduceMotion = _reduceMotion;
    final children = <Widget>[];
    if (_previousVisible) {
      children.add(
        _ControlEntrance(
          index: 0,
          reduceMotion: reduceMotion,
          child: _glassControlButton(
            icon: FusionIcons.previous,
            focusNode: _previousFocus,
            remotePressed: _previousPressed,
            key: const ValueKey('playback-previous-button'),
            onPressed: () => _changeTrack(
              PlaybackCommand.previous,
              ArtworkFlipDirection.right,
            ),
            rightFocusNode: _playVisible
                ? _playPauseFocus
                : (_nextVisible ? _nextFocus : null),
            compact: compact,
          ),
        ),
      );
    }
    if (_playVisible) {
      if (children.isNotEmpty) {
        children.add(SizedBox(width: compact ? 10 : 16));
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
        children.add(SizedBox(width: compact ? 10 : 16));
      }
      children.add(
        _ControlEntrance(
          index: 2,
          reduceMotion: reduceMotion,
          child: _glassControlButton(
            icon: FusionIcons.next,
            focusNode: _nextFocus,
            remotePressed: _nextPressed,
            key: const ValueKey('playback-next-button'),
            onPressed: () =>
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

  Widget _playPauseButton(bool reduceMotion, {required bool compact}) {
    final dimension = compact ? 46.0 : 58.0;
    final radius = compact ? 23.0 : 29.0;
    final glyph = compact ? 22.0 : 28.0;
    final hasPlayback = _hasPlayback;
    final background = hasPlayback ? Colors.white : const Color(0x1AFFFFFF);
    final foreground = hasPlayback
        ? const Color(0xFF151515)
        : const Color(0x61FFFFFF);
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
        child: SizedBox.square(
          dimension: dimension,
          child: Material(
            color: background,
            shape: const CircleBorder(),
            clipBehavior: Clip.antiAlias,
            child: IconButton(
              padding: EdgeInsets.zero,
              onPressed: hasPlayback
                  ? () => widget.onPlayback(PlaybackCommand.playPause)
                  : null,
              icon: AnimatedSwitcher(
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
                  color: foreground,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _glassControlButton({
    required String icon,
    required FocusNode focusNode,
    required ValueNotifier<bool> remotePressed,
    required Key key,
    required VoidCallback onPressed,
    FocusNode? leftFocusNode,
    FocusNode? rightFocusNode,
    FocusNode? downFocusNode,
    bool compact = false,
  }) {
    final dimension = compact ? 38.0 : 46.0;
    final radius = compact ? 19.0 : 23.0;
    final glyph = compact ? 18.0 : 22.0;
    return AnimatedPressScale(
      key: key,
      externalPressed: remotePressed,
      child: RemoteFocusFrame(
        focusNode: focusNode,
        onActivate: onPressed,
        onPressChange: (pressed) => remotePressed.value = pressed,
        borderColor: Colors.white,
        shape: G2ContinuousBorder(radius: radius),
        shortcuts: <ShortcutActivator, Intent>{
          const SingleActivator(LogicalKeyboardKey.arrowUp):
              const DirectionalFocusIntent(TraversalDirection.up),
          if (leftFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowLeft):
                const DirectionalFocusIntent(TraversalDirection.left),
          if (rightFocusNode != null)
            const SingleActivator(LogicalKeyboardKey.arrowRight):
                const DirectionalFocusIntent(TraversalDirection.right),
          if (downFocusNode != null)
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
                TraversalDirection.left => leftFocusNode,
                TraversalDirection.right => rightFocusNode,
                TraversalDirection.down => downFocusNode,
              };
              target?.requestFocus();
              return null;
            },
          ),
        },
        child: SizedBox.square(
          dimension: dimension,
          child: LiquidGlass(
            level: GlassLevel.control,
            interactive: true,
            borderRadius: 999,
            tint: const Color(0x26FFFFFF),
            forceFallback: widget.forceGlassFallback,
            child: Material(
              color: Colors.transparent,
              shape: const CircleBorder(),
              clipBehavior: Clip.antiAlias,
              child: InkWell(
                onTap: onPressed,
                child: Center(
                  child: FusionVectorIcon(
                    icon,
                    size: glyph,
                    color: Colors.white.withValues(alpha: .92),
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

  Widget _volumeControls({required bool compact}) {
    final volume = _displayVolume;
    final percent = (volume * 100).round();
    final icon = percent == 0
        ? Icons.volume_off_rounded
        : (percent < 50 ? Icons.volume_down_rounded : Icons.volume_up_rounded);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        ExcludeFocus(
          child: Semantics(
            label: percent == 0 ? '取消静音' : '静音',
            button: true,
            child: InkResponse(
              key: const ValueKey('volume-mute-button'),
              onTap: _toggleMute,
              radius: compact ? 18 : 22,
              child: SizedBox.square(
                dimension: compact ? 32 : 40,
                child: Center(
                  child: Icon(
                    icon,
                    size: compact ? 17 : 20,
                    color: Colors.white.withValues(alpha: .82),
                  ),
                ),
              ),
            ),
          ),
        ),
        SizedBox(width: compact ? 4 : 6),
        SizedBox(
          width: compact ? 78 : 96,
          child: RoundedProgressBar(
            key: const ValueKey('volume-slider'),
            value: volume,
            enabled: true,
            seekEnabled: true,
            showThumb: true,
            focusNode: _volumeFocus,
            upFocusNode: _playVisible ? _playPauseFocus : null,
            focusBorderColor: Colors.white,
            activeColor: Colors.white.withValues(alpha: .82),
            inactiveColor: Colors.white.withValues(alpha: .22),
            // Drag only updates the local thumb (no volume command). The
            // single command is sent exactly once on release; sending live
            // updates made AirPlay report "unsupported remote command" on
            // every drag tick.
            onChanged: (value) => setState(() => _volumeDrag = value),
            onChangeEnd: (value) => _commitVolume((value * 100).round()),
          ),
        ),
        SizedBox(
          width: compact ? 28 : 34,
          child: Text(
            '$percent',
            textAlign: TextAlign.right,
            style: TextStyle(
              fontSize: 11,
              color: Colors.white.withValues(alpha: .52),
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ),
      ],
    );
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
