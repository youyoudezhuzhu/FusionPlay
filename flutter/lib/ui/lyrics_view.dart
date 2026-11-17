import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

import '../models/lyrics.dart';
import '../theme/fusion_theme.dart';
import 'lyrics_playback_clock.dart';

/// Karaoke-capable lyrics viewport used by the player.
///
/// The parent constrains its height. Synced documents follow playback and fill
/// word-by-word; unsynced documents are a plain scrollable list. All motion
/// degrades to instant when `MediaQuery.disableAnimations` is set.
class LyricsView extends StatefulWidget {
  const LyricsView({
    super.key,
    required this.lyrics,
    required this.loading,
    required this.isPlaying,
    required this.positionMs,
    this.durationMs,
    this.trackKey,
    this.foreground = Colors.white,
    this.muted,
    this.activeColor,
    this.clock,
    this.compact = false,
    this.semanticLabel = '歌词',
    this.activeFontSize = 19,
    this.inactiveFontSize = 16,
    this.lineGap = 5,
    this.textShadow,
    this.emptyTitle = '暂无歌词',
    this.emptyDescription = '歌词由发送端提供',
  });

  final LyricsDocument? lyrics;
  final bool loading;
  final bool isPlaying;
  final int positionMs;
  final int? durationMs;

  /// Stable identity of the current track (trackIdentity/sourceEpoch/...).
  final Object? trackKey;

  final Color foreground;
  final Color? muted;
  final Color? activeColor;

  /// Injectable clock for tests; a local ticking clock is created otherwise.
  final LyricsPlaybackClock? clock;

  final bool compact;
  final String semanticLabel;

  /// Typography overrides for the large YAQMC-style right column.
  final double activeFontSize;
  final double inactiveFontSize;
  final double lineGap;
  final List<Shadow>? textShadow;

  /// Channel-specific copy for the "no lyrics" empty state.
  final String emptyTitle;
  final String emptyDescription;

  static const autoScrollDuration = Duration(milliseconds: 420);
  static const autoScrollPause = Duration(seconds: 4);
  static const currentLineAlignment = .40;

  @override
  State<LyricsView> createState() => _LyricsViewState();
}

class _LyricsViewState extends State<LyricsView>
    with SingleTickerProviderStateMixin {
  late final LyricsPlaybackClock _clock;
  late final bool _ownsClock;
  final ScrollController _scroll = ScrollController();
  List<GlobalKey> _lineKeys = const <GlobalKey>[];
  int _activeIndex = -1;
  bool _userPaused = false;
  Timer? _resumeTimer;
  bool _clockHidden = false;

  bool get _reduceMotion =>
      MediaQuery.maybeOf(context)?.disableAnimations ?? false;

  LyricsDocument? get _lyrics => widget.lyrics;

  bool get _synced => _lyrics?.isSynced ?? false;

  Color get _activeColor => widget.activeColor ?? widget.foreground;

  Color get _inactiveColor =>
      widget.muted ?? widget.foreground.withValues(alpha: .52);

  @override
  void initState() {
    super.initState();
    final injected = widget.clock;
    _ownsClock = injected == null;
    _clock = injected ?? LyricsPlaybackClock();
    if (_ownsClock) {
      _clock.attachTicker(this);
      _clock.setVisible(true);
    }
    _rebuildLineKeys();
    _clock.update(
      positionMs: widget.positionMs,
      isPlaying: widget.isPlaying,
      durationMs: widget.durationMs,
      trackKey: widget.trackKey,
    );
    _activeIndex = _computeActiveIndex();
    if (!_synced) {
      _clock.setVisible(false);
      _clockHidden = true;
    }
    _clock.addListener(_handleClockTick);
    if (_synced) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _scheduleCenter(animate: false);
      });
    }
  }

  @override
  void didUpdateWidget(covariant LyricsView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.clock != widget.clock) {
      // Clocks are owned per viewport; swapping them mid-life is unsupported.
    }
    final trackChanged = widget.trackKey != oldWidget.trackKey;
    final documentChanged = widget.lyrics != oldWidget.lyrics;
    if (trackChanged || documentChanged) {
      _rebuildLineKeys();
      _resetUserScrollPause();
      _activeIndex = -1;
    }
    _clock.update(
      positionMs: widget.positionMs,
      isPlaying: widget.isPlaying,
      durationMs: widget.durationMs,
      trackKey: widget.trackKey,
    );
    _handleClockTick();
    final shouldHide = !_synced || _lyrics == null;
    if (shouldHide && !_clockHidden) {
      _clock.setVisible(false);
      _clockHidden = true;
    } else if (!shouldHide && _clockHidden) {
      _clock.setVisible(true);
      _clockHidden = false;
    }
    if ((trackChanged || documentChanged) && _synced) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _scheduleCenter(animate: false);
      });
    }
  }

  @override
  void dispose() {
    _resumeTimer?.cancel();
    _clock.removeListener(_handleClockTick);
    if (_ownsClock) _clock.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _rebuildLineKeys() {
    final count = _lyrics?.lines.length ?? 0;
    _lineKeys = List<GlobalKey>.generate(
      count,
      (index) => GlobalKey(debugLabel: 'lyric-line-$index'),
    );
  }

  int _computeActiveIndex() {
    final lyrics = _lyrics;
    if (lyrics == null || !lyrics.isSynced) return -1;
    return lyrics.activeLineIndex(_clock.positionMs);
  }

  void _handleClockTick() {
    if (!mounted) return;
    final next = _computeActiveIndex();
    if (next == _activeIndex) return;
    setState(() => _activeIndex = next);
    if (!_userPaused && next >= 0) _scheduleCenter(animate: true);
  }

  void _scheduleCenter({required bool animate}) {
    if (_activeIndex < 0 || _activeIndex >= _lineKeys.length) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _userPaused) return;
      final lineContext = _lineKeys[_activeIndex].currentContext;
      if (lineContext == null) return;
      Scrollable.ensureVisible(
        lineContext,
        alignment: LyricsView.currentLineAlignment,
        duration: animate && !_reduceMotion
            ? LyricsView.autoScrollDuration
            : Duration.zero,
        curve: Curves.easeOutCubic,
      );
    });
  }

  void _pauseAutoScroll() {
    _resumeTimer?.cancel();
    _resumeTimer = Timer(LyricsView.autoScrollPause, _resumeAutoScroll);
    if (!_userPaused) setState(() => _userPaused = true);
  }

  void _resumeAutoScroll() {
    _resumeTimer?.cancel();
    if (!_userPaused) {
      _scheduleCenter(animate: true);
      return;
    }
    setState(() => _userPaused = false);
    _scheduleCenter(animate: true);
  }

  void _resetUserScrollPause() {
    _resumeTimer?.cancel();
    _userPaused = false;
  }

  bool _onScrollNotification(ScrollNotification notification) {
    if (notification is UserScrollNotification &&
        notification.direction != ScrollDirection.idle) {
      _pauseAutoScroll();
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final reduceMotion = _reduceMotion;
    final child = AnimatedSwitcher(
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 240),
      switchInCurve: Curves.easeOutCubic,
      switchOutCurve: Curves.easeOut,
      transitionBuilder: (child, animation) {
        if (reduceMotion) return child;
        final curved = CurvedAnimation(
          parent: animation,
          curve: Curves.easeOutCubic,
        );
        return FadeTransition(
          opacity: curved,
          child: SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, .025),
              end: Offset.zero,
            ).animate(curved),
            child: ScaleTransition(
              scale: Tween<double>(begin: .985, end: 1).animate(curved),
              child: child,
            ),
          ),
        );
      },
      child: _buildState(),
    );

    return Semantics(
      label: widget.semanticLabel,
      child: Stack(
        fit: StackFit.expand,
        children: [
          ShaderMask(
            blendMode: BlendMode.dstIn,
            shaderCallback: (rect) => const LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: [
                Color(0x00000000),
                Color(0xFF000000),
                Color(0xFF000000),
                Color(0x00000000),
              ],
              stops: [0, .12, .84, 1],
            ).createShader(rect),
            child: child,
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: 4,
            child: IgnorePointer(
              ignoring: !_userPaused,
              child: Center(
                child: AnimatedOpacity(
                  opacity: _userPaused ? 1 : 0,
                  duration: reduceMotion
                      ? Duration.zero
                      : const Duration(milliseconds: 160),
                  child: AnimatedScale(
                    scale: _userPaused ? 1 : .92,
                    duration: reduceMotion
                        ? Duration.zero
                        : const Duration(milliseconds: 160),
                    curve: Curves.easeOutCubic,
                    child: _ReturnToCurrentChip(
                      foreground: widget.foreground,
                      muted: _inactiveColor,
                      onTap: _resumeAutoScroll,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildState() {
    final lyrics = widget.lyrics;
    if (widget.loading && (lyrics == null || lyrics.lines.isEmpty)) {
      return _LyricsShimmer(
        key: const ValueKey('lyrics-loading'),
        reduceMotion: _reduceMotion,
        baseColor: _inactiveColor.withValues(alpha: .16),
        highlightColor: _activeColor.withValues(alpha: .30),
      );
    }
    if (lyrics == null || lyrics.lines.isEmpty) {
      return _LyricsEmpty(
        key: const ValueKey('lyrics-empty'),
        foreground: widget.foreground,
        muted: _inactiveColor,
        title: widget.emptyTitle,
        description: widget.emptyDescription,
      );
    }
    return _LyricsListBody(
      key: ValueKey(
        'lyrics-list:${widget.trackKey ?? 'track'}:'
        '${lyrics.synced}:${lyrics.lines.length}',
      ),
      lyrics: lyrics,
      synced: _synced,
      activeIndex: _synced ? _activeIndex : -1,
      lineKeys: _lineKeys,
      reduceMotion: _reduceMotion,
      compact: widget.compact,
      activeColor: _activeColor,
      inactiveColor: _inactiveColor,
      clock: _clock,
      durationMs: widget.durationMs,
      controller: _scroll,
      onScrollNotification: _onScrollNotification,
      activeFontSize: widget.activeFontSize,
      inactiveFontSize: widget.inactiveFontSize,
      lineGap: widget.lineGap,
      textShadow: widget.textShadow,
    );
  }
}

class _LyricsListBody extends StatelessWidget {
  const _LyricsListBody({
    super.key,
    required this.lyrics,
    required this.synced,
    required this.activeIndex,
    required this.lineKeys,
    required this.reduceMotion,
    required this.compact,
    required this.activeColor,
    required this.inactiveColor,
    required this.clock,
    required this.durationMs,
    required this.controller,
    required this.onScrollNotification,
    required this.activeFontSize,
    required this.inactiveFontSize,
    required this.lineGap,
    required this.textShadow,
  });

  final LyricsDocument lyrics;
  final bool synced;
  final int activeIndex;
  final List<GlobalKey> lineKeys;
  final bool reduceMotion;
  final bool compact;
  final Color activeColor;
  final Color inactiveColor;
  final LyricsPlaybackClock clock;
  final int? durationMs;
  final ScrollController controller;
  final bool Function(ScrollNotification notification) onScrollNotification;
  final double activeFontSize;
  final double inactiveFontSize;
  final double lineGap;
  final List<Shadow>? textShadow;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final height = constraints.maxHeight.isFinite && constraints.maxHeight > 0
          ? constraints.maxHeight
          : 200.0;
      return NotificationListener<ScrollNotification>(
        onNotification: onScrollNotification,
        child: SingleChildScrollView(
          controller: controller,
          physics: const BouncingScrollPhysics(),
          padding: EdgeInsets.symmetric(vertical: height * .38),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var index = 0; index < lyrics.lines.length; index++)
                _buildLine(index),
            ],
          ),
        ),
      );
    },
  );

  Widget _buildLine(int index) {
    final line = lyrics.lines[index];
    final active = synced && index == activeIndex;
    final blank = line.isBlank;
    final fontSize =
        (active ? activeFontSize : inactiveFontSize) * (compact ? .96 : 1.0);
    final color = active
        ? activeColor
        : blank
        ? inactiveColor.withValues(alpha: .30)
        : inactiveColor;
    final style = TextStyle(
      fontFamily: FusionTypography.fontFamily,
      fontSize: fontSize,
      height: 1.36,
      color: color,
      fontWeight: active
          ? FusionTypography.semiBoldFontWeight
          : FusionTypography.regularFontWeight,
      fontVariations: active
          ? FusionTypography.semiBold
          : FusionTypography.regular,
    );
    final content = active && line.hasWords
        ? _WordKaraokeLine(
            clock: clock,
            lyrics: lyrics,
            lineIndex: index,
            line: line,
            durationMs: durationMs,
            activeColor: activeColor,
            inactiveColor: inactiveColor,
          )
        : Text(blank ? '♪' : line.text, textAlign: TextAlign.start);
    final translation = line.translation?.trim();
    final translated = translation == null || translation.isEmpty
        ? null
        : Padding(
            padding: EdgeInsets.only(top: active ? 3 : 2),
            child: Text(
              translation,
              textAlign: TextAlign.start,
              style: TextStyle(
                fontFamily: FusionTypography.fontFamily,
                fontSize: fontSize * .62,
                height: 1.3,
                color: active
                    ? activeColor.withValues(alpha: .92)
                    : inactiveColor.withValues(alpha: .72),
                fontWeight: FusionTypography.regularFontWeight,
                fontVariations: FusionTypography.regular,
              ),
            ),
          );
    return Padding(
      key: lineKeys.length > index ? lineKeys[index] : null,
      padding: EdgeInsets.symmetric(horizontal: 2, vertical: active ? 7 : 5),
      child: AnimatedScale(
        scale: active && !reduceMotion ? 1.04 : 1,
        alignment: Alignment.centerLeft,
        duration: reduceMotion
            ? Duration.zero
            : const Duration(milliseconds: 200),
        curve: Curves.easeOutCubic,
        child: AnimatedDefaultTextStyle(
          duration: reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 200),
          curve: Curves.easeOutCubic,
          style: style,
          child: Stack(
            alignment: Alignment.topLeft,
            children: [
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [content, ?translated],
              ),
              if (active)
                const SizedBox.shrink(key: ValueKey('active-lyric-line')),
            ],
          ),
        ),
      ),
    );
  }
}

class _WordKaraokeLine extends StatelessWidget {
  const _WordKaraokeLine({
    required this.clock,
    required this.lyrics,
    required this.lineIndex,
    required this.line,
    required this.durationMs,
    required this.activeColor,
    required this.inactiveColor,
  });

  final LyricsPlaybackClock clock;
  final LyricsDocument lyrics;
  final int lineIndex;
  final LyricLine line;
  final int? durationMs;
  final Color activeColor;
  final Color inactiveColor;

  @override
  Widget build(BuildContext context) {
    final baseStyle = DefaultTextStyle.of(context).style;
    return AnimatedBuilder(
      animation: clock,
      builder: (context, _) {
        final position = clock.positionMs;
        return Wrap(
          spacing: 2,
          runSpacing: 2,
          children: [
            for (var index = 0; index < line.words.length; index++)
              LyricsWordFill(
                key: ValueKey('karaoke-word-$lineIndex-$index'),
                text: line.words[index].text,
                progress: lyrics.wordProgressAt(
                  lineIndex,
                  index,
                  position,
                  durationMs: durationMs,
                ),
                baseStyle: baseStyle,
                activeColor: activeColor,
                inactiveColor: inactiveColor,
                wordIndex: index,
              ),
          ],
        );
      },
    );
  }
}

/// One karaoke word: dark base glyphs with a bright copy clipped by [progress].
@visibleForTesting
class LyricsWordFill extends StatelessWidget {
  const LyricsWordFill({
    super.key,
    required this.text,
    required this.progress,
    required this.baseStyle,
    required this.activeColor,
    required this.inactiveColor,
    this.wordIndex = 0,
  });

  final String text;

  /// 0 = not started, 1 = fully sung.
  final double progress;
  final TextStyle baseStyle;
  final Color activeColor;
  final Color inactiveColor;
  final int wordIndex;

  @override
  Widget build(BuildContext context) {
    if (text.isEmpty) return const SizedBox.shrink();
    final clipped = progress.clamp(0.0, 1.0);
    return Stack(
      children: [
        Text(text, style: baseStyle.copyWith(color: inactiveColor)),
        if (clipped > 0)
          ClipRect(
            clipper: _WordProgressClipper(clipped),
            child: Text(text, style: baseStyle.copyWith(color: activeColor)),
          ),
      ],
    );
  }
}

class _WordProgressClipper extends CustomClipper<Rect> {
  const _WordProgressClipper(this.progress);

  final double progress;

  @override
  Rect getClip(Size size) =>
      Rect.fromLTWH(0, 0, size.width * progress, size.height);

  @override
  bool shouldReclip(_WordProgressClipper oldClipper) =>
      oldClipper.progress != progress;
}

class _ReturnToCurrentChip extends StatelessWidget {
  const _ReturnToCurrentChip({
    required this.foreground,
    required this.muted,
    required this.onTap,
  });

  final Color foreground;
  final Color muted;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Material(
    color: Colors.transparent,
    shape: const StadiumBorder(),
    clipBehavior: Clip.antiAlias,
    child: InkWell(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
        decoration: ShapeDecoration(
          color: foreground.withValues(alpha: .12),
          shape: StadiumBorder(
            side: BorderSide(color: foreground.withValues(alpha: .18)),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.my_location_rounded, size: 14, color: foreground),
            const SizedBox(width: 6),
            Text(
              '回到当前',
              style: TextStyle(
                fontSize: 12,
                color: foreground,
                fontWeight: FusionTypography.mediumFontWeight,
                fontVariations: FusionTypography.medium,
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

class _LyricsEmpty extends StatelessWidget {
  const _LyricsEmpty({
    super.key,
    required this.foreground,
    required this.muted,
    required this.title,
    required this.description,
  });

  final Color foreground;
  final Color muted;
  final String title;
  final String description;

  @override
  Widget build(BuildContext context) => Center(
    child: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(Icons.music_off_rounded, size: 30, color: muted),
        const SizedBox(height: 10),
        Text(
          title,
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: 15,
            color: foreground.withValues(alpha: .88),
            fontWeight: FusionTypography.semiBoldFontWeight,
            fontVariations: FusionTypography.semiBold,
          ),
        ),
        const SizedBox(height: 4),
        Text(
          description,
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 12, color: muted),
        ),
      ],
    ),
  );
}

class _LyricsShimmer extends StatefulWidget {
  const _LyricsShimmer({
    super.key,
    required this.reduceMotion,
    required this.baseColor,
    required this.highlightColor,
  });

  final bool reduceMotion;
  final Color baseColor;
  final Color highlightColor;

  @override
  State<_LyricsShimmer> createState() => _LyricsShimmerState();
}

class _LyricsShimmerState extends State<_LyricsShimmer>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1400),
  );

  static const _widthFactors = <double>[.88, .66, .92, .58];

  @override
  void initState() {
    super.initState();
    if (widget.reduceMotion) {
      _controller.value = .5;
    } else {
      _controller.repeat();
    }
  }

  @override
  void didUpdateWidget(covariant _LyricsShimmer oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.reduceMotion && _controller.isAnimating) {
      _controller.stop();
      _controller.value = .5;
    } else if (!widget.reduceMotion && !_controller.isAnimating) {
      _controller.repeat();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Center(
    child: AnimatedBuilder(
      animation: _controller,
      builder: (context, _) {
        final progress = _controller.value;
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var index = 0; index < _widthFactors.length; index++)
              Padding(
                padding: EdgeInsets.only(
                  bottom: index == _widthFactors.length - 1 ? 0 : 14,
                ),
                child: FractionallySizedBox(
                  alignment: Alignment.centerLeft,
                  widthFactor: _widthFactors[index],
                  child: Container(
                    height: 13,
                    decoration: ShapeDecoration(
                      gradient: LinearGradient(
                        colors: [
                          widget.baseColor,
                          widget.highlightColor,
                          widget.baseColor,
                        ],
                        stops: [
                          math.max(0, progress - .3),
                          math.max(.001, progress),
                          math.min(1, progress + .3),
                        ],
                      ),
                      shape: const StadiumBorder(),
                    ),
                  ),
                ),
              ),
          ],
        );
      },
    ),
  );
}
