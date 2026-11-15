import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models/app_state.dart';
import '../theme/fusion_theme.dart';
import 'animated_press_scale.dart';
import 'artwork.dart';
import 'g2_shape.dart';
import 'player_layout.dart';
import 'lyrics_view.dart';
import 'rounded_progress_bar.dart';
import 'remote_focus.dart';
import 'vector_icon.dart';

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
  });
  final AppState state;
  final ValueChanged<PlaybackCommand> onPlayback;
  final ValueChanged<int> onSeek;
  final ValueChanged<int> onVolume;
  final String? artworkSource;
  final String? artworkIdentity;
  final FocusNode? progressFocusNode;
  final FocusNode? settingsFocusNode;

  @override
  State<PlayerView> createState() => _PlayerViewState();
}

class _PlayerViewState extends State<PlayerView> {
  final FocusNode _fallbackProgressFocus = FocusNode(
    debugLabel: 'player-progress',
  );
  final FocusNode _previousFocus = FocusNode(debugLabel: 'player-previous');
  final FocusNode _playPauseFocus = FocusNode(debugLabel: 'player-play-pause');
  final FocusNode _nextFocus = FocusNode(debugLabel: 'player-next');
  final FocusNode _lyricsFocus = FocusNode(debugLabel: 'player-lyrics');
  final ValueNotifier<bool> _previousPressed = ValueNotifier(false);
  final ValueNotifier<bool> _playPausePressed = ValueNotifier(false);
  final ValueNotifier<bool> _nextPressed = ValueNotifier(false);
  final ValueNotifier<bool> _lyricsPressed = ValueNotifier(false);
  bool lyricsMode = false;
  bool seeking = false;
  double? seekPositionMs;
  int? pendingSeekTargetMs;
  Timer? pendingSeekTimer;
  ArtworkFlipDirection artworkFlipDirection = ArtworkFlipDirection.right;
  bool artworkFlipDirectionPending = false;
  Timer? artworkFlipDirectionTimer;
  final List<String> _artworkHistory = [];
  int _artworkHistoryIndex = -1;

  FocusNode get _progressFocus =>
      widget.progressFocusNode ?? _fallbackProgressFocus;

  @override
  void initState() {
    super.initState();
    final identity = _artworkHistoryIdentity(widget);
    if (identity != null) {
      _artworkHistory.add(identity);
      _artworkHistoryIndex = 0;
    }
  }

  @override
  void didUpdateWidget(covariant PlayerView oldWidget) {
    super.didUpdateWidget(oldWidget);
    final artworkTransitionChanged = widget.artworkIdentity != null
        ? widget.artworkIdentity != oldWidget.artworkIdentity
        : widget.artworkSource != oldWidget.artworkSource;
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
    if (lyricsMode && !_lyricsAvailable) {
      // Lyrics vanished while the page was open (sender stopped / new track
      // without lyrics): leave the mode without a jarring rebuild.
      lyricsMode = false;
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
  }

  String? _artworkHistoryIdentity(PlayerView view) {
    final identity = (view.artworkIdentity ?? view.artworkSource)?.trim();
    if (identity == null || identity.isEmpty) return null;
    final protocol = view.state.playback.protocol?.trim().toLowerCase() ?? '';
    return '$protocol\u001f$identity';
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
  void dispose() {
    pendingSeekTimer?.cancel();
    artworkFlipDirectionTimer?.cancel();
    _fallbackProgressFocus.dispose();
    _previousFocus.dispose();
    _playPauseFocus.dispose();
    _nextFocus.dispose();
    _lyricsFocus.dispose();
    _previousPressed.dispose();
    _playPausePressed.dispose();
    _nextPressed.dispose();
    _lyricsPressed.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, outer) {
        final landscape = outer.maxWidth > outer.maxHeight;
        final portrait = !landscape;
        final viewportInsets = playerViewportInsets(
          outer.maxWidth,
          outer.maxHeight,
        );
        final sidePadding = portrait ? 16.0 : viewportInsets.horizontal;
        final verticalPadding = portrait ? 16.0 : viewportInsets.vertical;
        return Padding(
          padding: EdgeInsets.fromLTRB(
            sidePadding,
            verticalPadding,
            sidePadding,
            verticalPadding,
          ),
          child: LayoutBuilder(
            builder: (context, box) {
              final layout = playerLayoutMetrics(
                box.maxWidth,
                box.maxHeight,
                landscape: landscape,
              );
              return layout.horizontal
                  ? _horizontal(layout, availableHeight: box.maxHeight)
                  : _compact(
                      layout,
                      centerTrackText: portrait,
                      availableHeight: box.maxHeight,
                    );
            },
          ),
        );
      },
    );
  }

  Widget _horizontal(
    PlayerLayoutMetrics layout, {
    required double availableHeight,
  }) {
    final reduceMotion = _reduceMotion;
    return Center(
      child: Row(
        key: const ValueKey('player-layout-horizontal'),
        mainAxisSize: MainAxisSize.min,
        children: [
          _animatedArtwork(
            baseSize: layout.artworkSize,
            shrinkScale: .82,
            reduceMotion: reduceMotion,
          ),
          SizedBox(width: layout.contentGap),
          ConstrainedBox(
            constraints: BoxConstraints(
              minWidth: layout.detailsMinWidth,
              maxWidth: layout.detailsMaxWidth,
            ),
            child: _details(
              textScale: layout.textScale,
              spacingScale: layout.spacingScale,
              controlScale: layout.controlScale,
              contentGap: layout.contentGap,
              lyricsViewportHeight: _lyricsViewportHeight(
                availableHeight,
                compact: false,
              ),
              compactLayout: false,
            ),
          ),
        ],
      ),
    );
  }

  Widget _compact(
    PlayerLayoutMetrics layout, {
    required bool centerTrackText,
    required double availableHeight,
  }) {
    final reduceMotion = _reduceMotion;
    return Center(
      child: SingleChildScrollView(
        child: Column(
          key: const ValueKey('player-layout-compact'),
          mainAxisSize: MainAxisSize.min,
          children: [
            _animatedArtwork(
              baseSize: layout.artworkSize,
              shrinkScale: .80,
              reduceMotion: reduceMotion,
            ),
            SizedBox(height: layout.contentGap),
            SizedBox(
              width: layout.detailsMaxWidth,
              child: _details(
                textScale: layout.textScale,
                spacingScale: layout.spacingScale,
                controlScale: layout.controlScale,
                contentGap: layout.contentGap,
                centerTrackText: centerTrackText,
                lyricsViewportHeight: _lyricsViewportHeight(
                  availableHeight,
                  compact: true,
                ),
                compactLayout: true,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// Artwork shrinks (and drifts slightly left) when lyrics open; the scale
  /// animates on a shared easeOutCubic curve with the rest of the transition.
  Widget _animatedArtwork({
    required double baseSize,
    required double shrinkScale,
    required bool reduceMotion,
  }) {
    final target = _showLyrics ? shrinkScale : 1.0;
    return TweenAnimationBuilder<double>(
      tween: Tween(end: target),
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 240),
      curve: Curves.easeOutCubic,
      builder: (context, scale, child) => Transform.translate(
        offset: Offset(-8 * (1 - scale), 0),
        child: _BreathingArtwork(
          enabled: widget.state.playback.isPlaying && !reduceMotion,
          child: ArtworkCard(
            source: widget.artworkSource,
            size: baseSize * scale,
            hasPlayback: _hasPlayback,
            identity: widget.artworkIdentity,
            flipDirection: artworkFlipDirection,
          ),
        ),
      ),
    );
  }

  Widget _details({
    required double textScale,
    required double spacingScale,
    required double controlScale,
    required double contentGap,
    required double lyricsViewportHeight,
    required bool compactLayout,
    bool centerTrackText = false,
  }) {
    final playback = widget.state.playback;
    final hasArtwork = widget.artworkSource?.trim().isNotEmpty ?? false;
    final hasPlayback = _hasPlayback;
    final foreground = hasArtwork ? Colors.white : const Color(0xFF171717);
    final muted = hasArtwork
        ? Colors.white.withValues(alpha: .74)
        : const Color(0xFF5F5F5F);
    final reduceMotion = _reduceMotion;
    final showLyrics = _showLyrics;
    final duration = math.max(0, playback.durationMs ?? 0);
    final position = (seekPositionMs?.round() ?? playback.positionMs).clamp(
      0,
      duration == 0 ? playback.positionMs : duration,
    );
    final canSeek = duration > 0 && _can('seek');
    final progress = duration > 0 ? position / duration : 0.0;
    return Column(
      crossAxisAlignment: centerTrackText
          ? CrossAxisAlignment.center
          : CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        _trackHeader(
          textScale: textScale,
          spacingScale: spacingScale,
          centerTrackText: centerTrackText,
          compactHeader: showLyrics,
          foreground: foreground,
          muted: muted,
        ),
        _animatedLyricsSlot(
          visible: showLyrics,
          reduceMotion: reduceMotion,
          height: lyricsViewportHeight,
          topGap: 8 * spacingScale,
          child: _lyricsPanel(
            foreground: foreground,
            muted: hasArtwork
                ? Colors.white.withValues(alpha: .55)
                : const Color(0xFF6B6B6B),
            compact: compactLayout,
          ),
        ),
        SizedBox(
          height: centerTrackText
              ? contentGap
              : (showLyrics ? 12 * spacingScale : 30 * spacingScale),
        ),
        Row(
          children: [
            SizedBox(
              width: 48 * spacingScale,
              child: Transform.translate(
                offset: Offset(centerTrackText ? -4 : 0, 0),
                child: Text(
                  hasPlayback ? _duration(position) : '1:14',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    color: muted,
                    fontSize: 12 * textScale,
                    fontWeight: FusionTypography.mediumFontWeight,
                    fontVariations: FusionTypography.medium,
                  ),
                ),
              ),
            ),
            Expanded(
              child: TweenAnimationBuilder<double>(
                tween: Tween(end: progress),
                duration: reduceMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 220),
                curve: Curves.easeOutCubic,
                builder: (context, animatedProgress, _) => RoundedProgressBar(
                  value: animatedProgress,
                  enabled: duration > 0,
                  seekEnabled: canSeek,
                  showThumb: false,
                  focusNode: _progressFocus,
                  upFocusNode: widget.settingsFocusNode,
                  downFocusNode: _playPauseFocus,
                  focusBorderColor: hasArtwork ? Colors.white : Colors.black,
                  activeColor: hasPlayback
                      ? foreground
                      : const Color(0x1A1A1A1A),
                  inactiveColor: duration <= 0
                      ? (!hasPlayback
                            ? const Color(0x1A1A1A1A)
                            : hasArtwork
                            ? Colors.white.withValues(alpha: .56)
                            : const Color(0x331A1A1A))
                      : (hasArtwork
                            ? Colors.white.withValues(alpha: .28)
                            : const Color(0xFFE7E7E7)),
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
            SizedBox(
              width: 48 * spacingScale,
              child: Transform.translate(
                offset: Offset(centerTrackText ? 4 : 0, 0),
                child: Text(
                  hasPlayback ? _duration(duration) : '5:14',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    color: muted,
                    fontSize: 12 * textScale,
                    fontWeight: FusionTypography.mediumFontWeight,
                    fontVariations: FusionTypography.medium,
                  ),
                ),
              ),
            ),
          ],
        ),
        SizedBox(height: showLyrics ? 12 * spacingScale : 20 * spacingScale),
        _controls(controlScale, spacingScale, hasPlayback),
      ],
    );
  }

  /// Reveals/hides the lyrics viewport. Reduced motion swaps the animated
  /// size for a plain slot: a zero-duration [AnimatedSize] would re-dirty
  /// itself during layout.
  Widget _animatedLyricsSlot({
    required bool visible,
    required bool reduceMotion,
    required double height,
    required double topGap,
    required Widget child,
  }) {
    final slot = visible
        ? Padding(
            padding: EdgeInsets.only(top: topGap),
            child: SizedBox(height: height, child: child),
          )
        : const SizedBox(width: double.infinity, height: 0);
    if (reduceMotion) return slot;
    return AnimatedSize(
      duration: const Duration(milliseconds: 240),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: slot,
    );
  }

  /// Title/artist header. Shrinks to a single compact line while lyrics are
  /// open so the lyrics viewport can own the middle of the details column.
  Widget _trackHeader({
    required double textScale,
    required double spacingScale,
    required bool centerTrackText,
    required bool compactHeader,
    required Color foreground,
    required Color muted,
  }) {
    final playback = widget.state.playback;
    final reduceMotion = _reduceMotion;
    final title = playback.title?.trim().isNotEmpty == true
        ? playback.title!.trim()
        : '暂无音乐';
    final artist = playback.artist?.trim();
    return TweenAnimationBuilder<double>(
      tween: Tween(end: compactHeader ? 1 : 0),
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 240),
      curve: Curves.easeOutCubic,
      builder: (context, compact, _) {
        final titleSize = (28 + (15 - 28) * compact) * textScale;
        final artistSize = (16 + (12 - 16) * compact) * textScale;
        final titleAlign = centerTrackText ? TextAlign.center : TextAlign.start;
        return Column(
          crossAxisAlignment: centerTrackText
              ? CrossAxisAlignment.center
              : CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              title,
              maxLines: compact >= .5 ? 1 : 2,
              overflow: TextOverflow.ellipsis,
              textAlign: titleAlign,
              style: Theme.of(context).textTheme.headlineMedium?.copyWith(
                fontSize: titleSize,
                height: 1.25,
                color: foreground,
                fontWeight: FusionTypography.semiBoldFontWeight,
                fontVariations: FusionTypography.semiBold,
              ),
            ),
            if (artist != null && artist.isNotEmpty) ...[
              SizedBox(height: (6 + (2 - 6) * compact) * spacingScale),
              Text(
                artist,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: titleAlign,
                style: Theme.of(context).textTheme.titleMedium?.copyWith(
                  fontSize: artistSize,
                  color: muted,
                  fontWeight: FusionTypography.regularFontWeight,
                  fontVariations: FusionTypography.regular,
                ),
              ),
            ],
          ],
        );
      },
    );
  }

  Widget _lyricsPanel({
    required Color foreground,
    required Color muted,
    required bool compact,
  }) {
    final playback = widget.state.playback;
    final reduceMotion = _reduceMotion;
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 260),
      curve: Curves.easeOutCubic,
      builder: (context, progress, child) => Opacity(
        opacity: progress,
        child: Transform.translate(
          offset: Offset(0, 12 * (1 - progress)),
          child: Transform.scale(
            scale: .985 + .015 * progress,
            alignment: Alignment.topCenter,
            child: child,
          ),
        ),
      ),
      child: LyricsView(
        key: const ValueKey('player-lyrics-view'),
        lyrics: playback.lyrics,
        loading: playback.lyricsLoading,
        isPlaying: playback.isPlaying,
        positionMs: playback.positionMs,
        durationMs: playback.durationMs,
        trackKey: _lyricsTrackKey,
        foreground: foreground,
        muted: muted,
        compact: compact,
      ),
    );
  }

  Widget _controls(double controlScale, double spacingScale, bool hasPlayback) {
    final small = 50 * controlScale;
    final play = 62 * controlScale;
    final gap = 18 * spacingScale;
    final reduceMotion = _reduceMotion;
    final lyricsAvailable = _lyricsAvailable;
    const idleElement = Color(0x1A1A1A1A);
    final tonal = hasPlayback
        ? Colors.black.withValues(alpha: .28)
        : idleElement;
    final disabledContainer = hasPlayback
        ? Colors.black.withValues(alpha: .28)
        : idleElement;
    final disabledContent = hasPlayback
        ? Colors.white.withValues(alpha: .38)
        : const Color(0x61151515);
    final playBackground = hasPlayback ? Colors.white : idleElement;
    final playForeground = hasPlayback ? Colors.black : const Color(0xFF151515);
    final playDisabledBackground = hasPlayback ? Colors.white : idleElement;
    final playDisabledForeground = const Color(0x61151515);
    final smallForeground = hasPlayback
        ? Colors.white
        : const Color(0xFF151515);
    final coreControls = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _ControlEntrance(
          index: 0,
          reduceMotion: reduceMotion,
          child: _controlButton(
            FusionIcons.previous,
            _previousFocus,
            _previousPressed,
            small,
            24 * controlScale,
            tonal,
            smallForeground,
            disabledContainer,
            disabledContent,
            hasPlayback
                ? () => _changeTrack(
                    PlaybackCommand.previous,
                    ArtworkFlipDirection.right,
                  )
                : null,
            controlKey: const ValueKey('playback-previous-button'),
          ),
        ),
        SizedBox(width: gap),
        _ControlEntrance(
          index: 1,
          reduceMotion: reduceMotion,
          child: _controlButton(
            widget.state.playback.isPlaying
                ? FusionIcons.pause
                : FusionIcons.play,
            _playPauseFocus,
            _playPausePressed,
            play,
            30 * controlScale,
            playBackground,
            playForeground,
            playDisabledBackground,
            playDisabledForeground,
            hasPlayback
                ? () => widget.onPlayback(PlaybackCommand.playPause)
                : null,
            upFocusNode: _progressFocus,
            leftFocusNode: _previousFocus,
            rightFocusNode: _nextFocus,
            controlKey: const ValueKey('playback-play-pause-button'),
          ),
        ),
        SizedBox(width: gap),
        _ControlEntrance(
          index: 2,
          reduceMotion: reduceMotion,
          child: _controlButton(
            FusionIcons.next,
            _nextFocus,
            _nextPressed,
            small,
            24 * controlScale,
            tonal,
            smallForeground,
            disabledContainer,
            disabledContent,
            hasPlayback
                ? () => _changeTrack(
                    PlaybackCommand.next,
                    ArtworkFlipDirection.left,
                  )
                : null,
            rightFocusNode: lyricsAvailable ? _lyricsFocus : null,
            controlKey: const ValueKey('playback-next-button'),
          ),
        ),
        if (lyricsAvailable) ...[
          SizedBox(width: gap * .7),
          _ControlEntrance(
            index: 3,
            reduceMotion: reduceMotion,
            child: _lyricsButton(controlScale, hasPlayback),
          ),
        ],
      ],
    );
    return SizedBox(
      height: play,
      child: Center(
        child: FittedBox(fit: BoxFit.scaleDown, child: coreControls),
      ),
    );
  }

  Widget _lyricsButton(double controlScale, bool hasPlayback) {
    final size = 46 * controlScale;
    final reduceMotion = _reduceMotion;
    final active = lyricsMode;
    final hasArtwork = widget.artworkSource?.trim().isNotEmpty ?? false;
    final accent = hasPlayback ? Colors.white : Colors.black;
    final background = hasPlayback
        ? (active ? Colors.white : Colors.black.withValues(alpha: .30))
        : (active ? Colors.white : const Color(0x1A1A1A1A));
    final iconColor = hasPlayback
        ? (active ? Colors.black : Colors.white)
        : const Color(0xFF151515);
    return AnimatedPressScale(
      key: const ValueKey('playback-lyrics-button'),
      enabled: true,
      externalPressed: _lyricsPressed,
      child: RemoteFocusFrame(
        enabled: true,
        focusNode: _lyricsFocus,
        onActivate: _toggleLyricsMode,
        onPressChange: (pressed) => _lyricsPressed.value = pressed,
        shortcuts: const <ShortcutActivator, Intent>{
          SingleActivator(LogicalKeyboardKey.arrowUp): DirectionalFocusIntent(
            TraversalDirection.up,
          ),
          SingleActivator(LogicalKeyboardKey.arrowLeft): DirectionalFocusIntent(
            TraversalDirection.left,
          ),
        },
        actions: <Type, Action<Intent>>{
          DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
            onInvoke: (intent) {
              if (intent.direction == TraversalDirection.up) {
                _progressFocus.requestFocus();
              } else if (intent.direction == TraversalDirection.left) {
                _nextFocus.requestFocus();
              }
              return null;
            },
          ),
        },
        borderColor: hasArtwork ? Colors.white : Colors.black,
        shape: G2ContinuousBorder(radius: size / 2),
        child: Stack(
          alignment: Alignment.center,
          children: [
            AnimatedContainer(
              duration: reduceMotion
                  ? Duration.zero
                  : const Duration(milliseconds: 220),
              curve: Curves.easeOutCubic,
              width: size,
              height: size,
              decoration: ShapeDecoration(
                color: background,
                shape: G2ContinuousBorder(radius: size / 2),
                shadows: active && !reduceMotion
                    ? <BoxShadow>[
                        BoxShadow(
                          color: accent.withValues(alpha: .30),
                          blurRadius: 16,
                          spreadRadius: 1,
                        ),
                      ]
                    : null,
              ),
              child: Material(
                color: Colors.transparent,
                shape: G2ContinuousBorder(radius: size / 2),
                clipBehavior: Clip.antiAlias,
                child: InkWell(
                  onTap: _toggleLyricsMode,
                  child: Center(
                    child: AnimatedSwitcher(
                      duration: reduceMotion
                          ? Duration.zero
                          : const Duration(milliseconds: 180),
                      child: Icon(
                        active ? Icons.lyrics_rounded : Icons.lyrics_outlined,
                        key: ValueKey<bool>(active),
                        size: 22 * controlScale,
                        color: iconColor,
                      ),
                    ),
                  ),
                ),
              ),
            ),
            if (!active)
              Positioned(
                top: size * .16,
                right: size * .16,
                child: Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    color: accent.withValues(alpha: hasPlayback ? .9 : .6),
                    shape: BoxShape.circle,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  void _toggleLyricsMode() {
    if (!_lyricsAvailable) return;
    setState(() => lyricsMode = !lyricsMode);
  }

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

  Widget _controlButton(
    String icon,
    FocusNode focusNode,
    ValueNotifier<bool> remotePressed,
    double size,
    double iconSize,
    Color background,
    Color foreground,
    Color disabledBackground,
    Color disabledForeground,
    VoidCallback? onPressed, {
    required Key controlKey,
    FocusNode? upFocusNode,
    FocusNode? leftFocusNode,
    FocusNode? rightFocusNode,
  }) {
    final lightFocus = widget.artworkSource?.trim().isNotEmpty ?? false;
    final reduceMotion = _reduceMotion;
    final hasDirectionalTargets =
        upFocusNode != null || leftFocusNode != null || rightFocusNode != null;
    return AnimatedPressScale(
      key: controlKey,
      enabled: onPressed != null,
      externalPressed: remotePressed,
      child: RemoteFocusFrame(
        enabled: onPressed != null,
        focusNode: focusNode,
        onActivate: onPressed,
        onPressChange: (pressed) => remotePressed.value = pressed,
        shortcuts: <ShortcutActivator, Intent>{
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
        actions: !hasDirectionalTargets
            ? const <Type, Action<Intent>>{}
            : <Type, Action<Intent>>{
                DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
                  onInvoke: (intent) {
                    final target = switch (intent.direction) {
                      TraversalDirection.up => upFocusNode,
                      TraversalDirection.left => leftFocusNode,
                      TraversalDirection.right => rightFocusNode,
                      _ => null,
                    };
                    target?.requestFocus();
                    return null;
                  },
                ),
              },
        borderColor: lightFocus ? Colors.white : Colors.black,
        shape: G2ContinuousBorder(radius: size / 2),
        child: SizedBox.square(
          dimension: size,
          child: Material(
            color: onPressed == null ? disabledBackground : background,
            shape: G2ContinuousBorder(radius: size / 2),
            clipBehavior: Clip.antiAlias,
            child: IconButton(
              padding: EdgeInsets.zero,
              onPressed: onPressed,
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
                  icon,
                  key: ValueKey<String>(icon),
                  size: iconSize,
                  color: onPressed == null ? disabledForeground : foreground,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  bool get _reduceMotion =>
      MediaQuery.maybeOf(context)?.disableAnimations ?? false;

  LyricsDocument? get _lyrics => widget.state.playback.lyrics;

  bool get _lyricsAvailable {
    final lyrics = _lyrics;
    return widget.state.playback.lyricsLoading ||
        (lyrics != null && lyrics.lines.isNotEmpty && lyrics.hasLyrics);
  }

  bool get _showLyrics => lyricsMode && _lyricsAvailable;

  Object get _lyricsTrackKey {
    final playback = widget.state.playback;
    return playback.trackIdentity ??
        playback.sourceEpoch ??
        playback.mediaUrl ??
        playback.protocol ??
        'fusionplay-track';
  }

  /// Lyrics viewport height: 190-240 px on a regular landscape layout and a
  /// graceful minimum on short layouts so the details column never overflows.
  double _lyricsViewportHeight(
    double availableHeight, {
    required bool compact,
  }) {
    if (compact) {
      return (availableHeight * .30).clamp(140.0, 220.0).toDouble();
    }
    // Header + progress row + controls keep roughly 196 px; on 660 px-tall
    // landscapes the .34 share lands in the 190-240 px band, on short
    // landscapes it shrinks instead of overflowing the details column.
    final maxAllowed = math.max(64.0, availableHeight - 196.0);
    final upper = math.min(240.0, maxAllowed);
    final lower = math.min(140.0, upper);
    return (availableHeight * .34).clamp(lower, upper).toDouble();
  }

  bool _can(String command) {
    if (!widget.state.remoteControl.available) return false;
    final commands = widget.state.remoteControl.commands;
    return commands.contains(command) ||
        (command == 'previous' && commands.contains('previous_track')) ||
        (command == 'next' && commands.contains('next_track'));
  }

  bool get _hasPlayback {
    final playback = widget.state.playback;
    return playback.streamActive ||
        (playback.protocol?.trim().isNotEmpty ?? false) ||
        (playback.mediaUrl?.trim().isNotEmpty ?? false) ||
        (playback.title?.trim().isNotEmpty ?? false) ||
        (playback.artist?.trim().isNotEmpty ?? false) ||
        playback.durationMs != null;
  }

  String _duration(int milliseconds) {
    final seconds = milliseconds ~/ 1000;
    return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
  }
}

/// Tiny one-shot staggered entrance for the transport controls.
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
    duration: reduceMotion ? Duration.zero : const Duration(milliseconds: 320),
    curve: Interval(
      (index * .09).clamp(0.0, .65),
      1,
      curve: Curves.easeOutCubic,
    ),
    builder: (context, reveal, child) => Opacity(
      opacity: reveal,
      child: Transform.translate(
        offset: Offset(0, 6 * (1 - reveal)),
        child: child,
      ),
    ),
    child: child,
  );
}

/// Very light "breathing" scale for the artwork while audio is playing.
/// Amplitude stays under 1%; disabled with reduced motion.
class _BreathingArtwork extends StatefulWidget {
  const _BreathingArtwork({required this.enabled, required this.child});

  final bool enabled;
  final Widget child;

  @override
  State<_BreathingArtwork> createState() => _BreathingArtworkState();
}

class _BreathingArtworkState extends State<_BreathingArtwork>
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
  void didUpdateWidget(covariant _BreathingArtwork oldWidget) {
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
