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

  bool seeking = false;
  double? seekPositionMs;
  int? pendingSeekTargetMs;
  Timer? pendingSeekTimer;
  ArtworkFlipDirection artworkFlipDirection = ArtworkFlipDirection.right;
  bool artworkFlipDirectionPending = false;
  Timer? artworkFlipDirectionTimer;
  final List<String> _artworkHistory = [];
  int _artworkHistoryIndex = -1;

  double? _volumeDrag;
  int? _optimisticVolume;
  int? _lastAudibleVolume;
  Timer? _volumeDebounceTimer;
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

  @override
  void dispose() {
    pendingSeekTimer?.cancel();
    artworkFlipDirectionTimer?.cancel();
    _volumeDebounceTimer?.cancel();
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
    super.dispose();
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
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, outer) {
        final landscape = outer.maxWidth > outer.maxHeight;
        return Column(
          children: [
            _topBar(landscape: landscape),
            Expanded(child: landscape ? _landscapeBody() : _portraitBody()),
            _dock(),
          ],
        );
      },
    );
  }

  // ------------------------------------------------------------------
  // Top bar
  // ------------------------------------------------------------------

  Widget _topBar({required bool landscape}) {
    final playback = widget.state.playback;
    final protocol = _protocolLabel(playback.protocol);
    final quality = playback.qualityText?.trim();
    final reduceMotion = _reduceMotion;
    final entrance = reduceMotion
        ? Duration.zero
        : const Duration(milliseconds: 240);
    return Padding(
      padding: EdgeInsets.fromLTRB(
        landscape ? 30 : 18,
        landscape ? 16 : 12,
        landscape ? 22 : 12,
        6,
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
                  const Text(
                    '正在播放',
                    style: TextStyle(
                      fontSize: 12,
                      letterSpacing: 1.8,
                      color: Color(0x9EFFFFFF),
                      fontWeight: FusionTypography.semiBoldFontWeight,
                      fontVariations: FusionTypography.semiBold,
                    ),
                  ),
                  if (protocol != null) _chip(protocol),
                  if (quality != null && quality.isNotEmpty) _chip(quality),
                ],
              ),
            ),
          ),
          if (_menuVisible) _menuButton(),
        ],
      ),
    );
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

  Widget _menuButton() => AnimatedPressScale(
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
      },
      actions: <Type, Action<Intent>>{
        DirectionalFocusIntent: CallbackAction<DirectionalFocusIntent>(
          onInvoke: (_) {
            _focusAfterTopBar();
            return null;
          },
        ),
      },
      child: SizedBox.square(
        dimension: 46,
        child: LiquidGlass(
          borderRadius: 999,
          blurSigma: 16,
          tint: widget.menuOpen
              ? const Color(0x40FFFFFF)
              : const Color(0x1FFFFFFF),
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
  );

  void _focusAfterTopBar() {
    if (_seekable) {
      _progressFocus.requestFocus();
    } else if (_playVisible) {
      _playPauseFocus.requestFocus();
    } else if (_controls.any) {
      _volumeFocus.requestFocus();
    }
  }

  String? _protocolLabel(String? protocol) {
    final value = protocol?.trim();
    if (value == null || value.isEmpty) return null;
    final normalized = value.toLowerCase();
    if (normalized.contains('miplay') ||
        normalized.contains('xiaomi') ||
        value == '小米妙播') {
      return '小米妙播';
    }
    if (normalized.contains('airplay')) return 'AirPlay';
    if (normalized.contains('dlna')) return 'DLNA';
    return value;
  }

  // ------------------------------------------------------------------
  // Main content
  // ------------------------------------------------------------------

  Widget _landscapeBody() => Padding(
    padding: const EdgeInsets.fromLTRB(30, 4, 24, 10),
    child: LayoutBuilder(
      builder: (context, content) {
        final leftWidth = (content.maxWidth * .36)
            .clamp(210.0, 380.0)
            .toDouble();
        final dense = content.maxHeight < 260;
        final artwork = math
            .min(leftWidth - 8, content.maxHeight * (dense ? .50 : .62))
            .clamp(dense ? 40.0 : 96.0, 360.0)
            .toDouble();
        final rightWidth = content.maxWidth - leftWidth - 28;
        final activeFont = (rightWidth * .042).clamp(22.0, 34.0).toDouble();
        return Row(
          key: const ValueKey('player-layout-horizontal'),
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            SizedBox(
              width: leftWidth,
              child: _leftColumn(
                artwork: artwork,
                compact: content.maxHeight < 420,
                center: false,
                dense: dense,
              ),
            ),
            const SizedBox(width: 28),
            Expanded(
              child: _lyricsColumn(
                activeFontSize: activeFont,
                inactiveFontSize: activeFont * .68,
              ),
            ),
          ],
        );
      },
    ),
  );

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
    final title = playback.title?.trim().isNotEmpty == true
        ? playback.title!.trim()
        : '暂无音乐';
    final artist = playback.artist?.trim();
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

  Widget _artworkCard(double size) => DecoratedBox(
    decoration: BoxDecoration(
      borderRadius: BorderRadius.circular(26),
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
      borderRadius: BorderRadius.circular(26),
      child: ArtworkCard(
        source: widget.artworkSource,
        size: size,
        hasPlayback: _hasPlayback,
        identity: widget.artworkIdentity,
        flipDirection: artworkFlipDirection,
      ),
    ),
  );

  Widget _breathingArtwork({required double size, required Widget child}) {
    final enabled = widget.state.playback.isPlaying && !_reduceMotion;
    return _ArtworkBreath(enabled: enabled, child: child);
  }

  Widget _lyricsColumn({
    required double activeFontSize,
    required double inactiveFontSize,
    bool alignCenter = false,
  }) {
    final playback = widget.state.playback;
    final reduceMotion = _reduceMotion;
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 280),
      curve: Curves.easeOutCubic,
      builder: (context, reveal, child) => Opacity(
        opacity: reveal,
        child: Transform.translate(
          offset: Offset(0, 10 * (1 - reveal)),
          child: child,
        ),
      ),
      child: LyricsView(
        key: const ValueKey('player-lyrics-view'),
        lyrics: playback.lyrics,
        loading: playback.lyricsLoading,
        isPlaying: playback.isPlaying,
        positionMs: playback.positionMs,
        durationMs: playback.durationMs,
        trackKey:
            playback.trackIdentity ??
            playback.sourceEpoch ??
            playback.mediaUrl ??
            playback.protocol ??
            'fusionplay-track',
        foreground: Colors.white,
        muted: Colors.white.withValues(alpha: .50),
        activeColor: Colors.white,
        activeFontSize: activeFontSize,
        inactiveFontSize: inactiveFontSize,
        lineGap: (activeFontSize * .30).clamp(7.0, 13.0),
        textShadow: const [Shadow(blurRadius: 18, color: Color(0x66000000))],
        semanticLabel: '歌词',
      ),
    );
  }

  String? _lyricsStatusLabel(PlaybackSnapshot playback) {
    final lyrics = playback.lyrics;
    if (lyrics != null && lyrics.hasLyrics) {
      if (lyrics.hasWordTiming) return '逐字歌词';
      if (lyrics.isSynced) return '同步歌词';
      return '静态歌词';
    }
    return null;
  }

  // ------------------------------------------------------------------
  // Bottom dock
  // ------------------------------------------------------------------

  Widget _dock() {
    final playback = widget.state.playback;
    final duration = _durationMs;
    final position = seekPositionMs?.round() ?? playback.positionMs;
    final clampedPosition = position.clamp(
      0,
      duration == 0 ? playback.positionMs : duration,
    );
    final progress = duration > 0 ? clampedPosition / duration : 0.0;
    final reduceMotion = _reduceMotion;
    return Padding(
      padding: const EdgeInsets.fromLTRB(22, 0, 22, 18),
      child: LiquidGlass(
        borderRadius: 28,
        blurSigma: 18,
        tint: const Color(0x59101318),
        forceFallback: widget.forceGlassFallback,
        padding: const EdgeInsets.fromLTRB(20, 8, 20, 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              children: [
                _timeLabel(
                  _hasPlayback ? _formatDuration(clampedPosition) : '0:00',
                ),
                const SizedBox(width: 10),
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
                const SizedBox(width: 10),
                _timeLabel(_hasPlayback ? _formatDuration(duration) : '0:00'),
              ],
            ),
            const SizedBox(height: 4),
            LayoutBuilder(
              builder: (context, dock) {
                final wide = dock.maxWidth >= 460;
                final transport = _transport();
                final volume = _volumeControls();
                if (wide) {
                  return Row(
                    children: [
                      Expanded(
                        child: Align(
                          alignment: Alignment.centerLeft,
                          child: _dockStatus(),
                        ),
                      ),
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

  Widget _timeLabel(String text) => SizedBox(
    width: 46,
    child: Text(
      text,
      textAlign: TextAlign.center,
      style: TextStyle(
        fontSize: 12,
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

  Widget _transport() {
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
          ),
        ),
      );
    }
    if (_playVisible) {
      if (children.isNotEmpty) children.add(const SizedBox(width: 16));
      children.add(
        _ControlEntrance(
          index: 1,
          reduceMotion: reduceMotion,
          child: _playPauseButton(reduceMotion),
        ),
      );
    }
    if (_nextVisible) {
      if (children.isNotEmpty) children.add(const SizedBox(width: 16));
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
          ),
        ),
      );
    }
    if (children.isEmpty) return const SizedBox(height: 56);
    return Row(mainAxisSize: MainAxisSize.min, children: children);
  }

  Widget _playPauseButton(bool reduceMotion) {
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
        shape: G2ContinuousBorder(radius: 29),
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
                  _seekable
                      ? _progressFocus
                      : (_menuVisible ? _menuFocus : null),
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
          dimension: 58,
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
                  size: 28,
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
  }) {
    final reduceMotion = _reduceMotion;
    return AnimatedPressScale(
      key: key,
      externalPressed: remotePressed,
      child: RemoteFocusFrame(
        focusNode: focusNode,
        onActivate: onPressed,
        onPressChange: (pressed) => remotePressed.value = pressed,
        borderColor: Colors.white,
        shape: G2ContinuousBorder(radius: 23),
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
                  _seekable
                      ? _progressFocus
                      : (_menuVisible ? _menuFocus : null),
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
          dimension: 46,
          child: LiquidGlass(
            borderRadius: 999,
            blurSigma: 14,
            tint: const Color(0x26FFFFFF),
            animate: !reduceMotion,
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
                    size: 22,
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

  Widget _volumeControls() {
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
              radius: 22,
              child: SizedBox.square(
                dimension: 40,
                child: Center(
                  child: Icon(
                    icon,
                    size: 20,
                    color: Colors.white.withValues(alpha: .82),
                  ),
                ),
              ),
            ),
          ),
        ),
        const SizedBox(width: 6),
        SizedBox(
          width: 96,
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
            onChanged: (value) {
              setState(() => _volumeDrag = value);
              _scheduleVolumeCommit((value * 100).round());
            },
            onChangeEnd: (value) => _commitVolume((value * 100).round()),
          ),
        ),
        SizedBox(
          width: 34,
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

  /// Throttled live volume updates while dragging (120-150ms).
  void _scheduleVolumeCommit(int percent) {
    _volumeDebounceTimer?.cancel();
    _volumeDebounceTimer = Timer(const Duration(milliseconds: 130), () {
      if (!mounted) return;
      _applyVolume(percent);
    });
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

  void _commitVolume(int percent) {
    _volumeDebounceTimer?.cancel();
    _volumeDebounceTimer = null;
    _applyVolume(percent);
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
