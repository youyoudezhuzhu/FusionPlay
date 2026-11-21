import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/update_dialogs.dart';
import 'package:fusionplay_android_flutter/backend/update_service.dart';
import 'package:fusionplay_android_flutter/theme/fusion_theme.dart';
import 'package:fusionplay_android_flutter/ui/app_menu.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/glass_appearance.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';
import 'package:fusionplay_android_flutter/ui/settings_view.dart';

final Directory reviewDir = Platform.environment['FUSIONPLAY_AUDIT'] == '1'
    ? Directory('../fusionplay-ui-audit-codex')
    : Directory.systemTemp.createTempSync('fusionplay-ui-audit-');
final String coverData =
    'data:image/png;base64,${base64Encode(File('../fusionplay-ui-audit-codex/assets/cover.png').readAsBytesSync())}';

LyricsDocument _lyrics() => const LyricsDocument(
  origin: 'embedded',
  synced: true,
  lines: [
    LyricLine(startMs: 0, text: 'The night is calling'),
    LyricLine(startMs: 8000, text: 'And the stars are dancing'),
    LyricLine(startMs: 16000, text: 'With you by my side'),
    LyricLine(startMs: 24000, text: 'Everything feels right'),
    LyricLine(startMs: 32000, text: 'In this starlight'),
  ],
);

AppState _state({
  bool withLyrics = true,
  String title = 'Starlight',
  String artist = 'Muse · Drones',
  bool playing = true,
  int positionMs = 84000,
  int durationMs = 210000,
}) => AppState(
  initialized: true,
  playback: PlaybackSnapshot(
    title: title,
    artist: artist,
    album: 'Drones',
    durationMs: durationMs,
    positionMs: positionMs,
    streamActive: true,
    isPlaying: playing,
    protocol: 'airplay',
    qualityText: 'AirPlay 2 · Lossless',
    volumePercent: 68,
    lyrics: withLyrics ? _lyrics() : null,
  ),
  controls: PlaybackControls.all,
);

Widget _background() => Stack(
  fit: StackFit.expand,
  children: [
    ArtworkBackground(source: coverData, hasPlayback: true),
    const ColoredBox(color: Color(0x66000000)),
  ],
);

Widget _playerHost(AppState state, {String? artwork, Widget? backdrop}) =>
    RepaintBoundary(
      key: const ValueKey('review-boundary'),
      child: FrostedScene(
        background: backdrop ?? _background(),
        child: Scaffold(
          backgroundColor: Colors.transparent,
          body: Stack(
            fit: StackFit.expand,
            children: [
              SafeArea(
                child: PlayerView(
                  state: state,
                  artworkSource: artwork,
                  artworkIdentity: 'review-cover',
                  onPlayback: (_) {},
                  onSeek: (_) {},
                  onVolume: (_) {},
                  onOpenMenu: () {},
                ),
              ),
            ],
          ),
        ),
      ),
    );

Future<void> _pump(
  WidgetTester tester,
  Widget host,
  Size logical, {
  double dpr = 2,
  Duration elapsed = const Duration(milliseconds: 400),
}) async {
  tester.view.devicePixelRatio = dpr;
  tester.view.physicalSize = Size(logical.width * dpr, logical.height * dpr);
  await tester.pumpWidget(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: fusionTheme(),
      home: host,
    ),
  );
  await tester.pump();
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 350)),
  );
  await tester.pump(elapsed);
}

Future<void> _capture(
  WidgetTester tester,
  String path, {
  double dpr = 2,
}) async {
  final boundary = tester.firstRenderObject<RenderRepaintBoundary>(
    find.byKey(const ValueKey('review-boundary')),
  );
  final image = await tester.runAsync(() => boundary.toImage(pixelRatio: dpr));
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  if (path.endsWith('/P01.png')) {
    final pixels = await tester.runAsync(() => image!.toByteData());
    // The right-hand empty backdrop was saturated red across this band in
    // both historical P01 captures. Check multiple samples of the same fixture.
    for (final x in [900, 1300, 1500]) {
      final offset = (680 * image!.width + x) * 4;
      expect(
        pixels!.getUint8(offset) - pixels.getUint8(offset + 1),
        lessThan(100),
        reason: 'P01 backdrop contamination at x=$x',
      );
    }
  }
  image!.dispose();
  File(path).parent.createSync(recursive: true);
  File(path).writeAsBytesSync(data!.buffer.asUint8List());
}

void _metric(String id, String size, String name, Rect r) {
  debugPrint(
    'METRIC|$id|$size|$name|${r.left.toStringAsFixed(1)}|${r.top.toStringAsFixed(1)}|'
    '${r.right.toStringAsFixed(1)}|${r.bottom.toStringAsFixed(1)}|'
    '${r.width.toStringAsFixed(1)}|${r.height.toStringAsFixed(1)}',
  );
}

Future<void> _capturePlayer(
  WidgetTester tester, {
  required String id,
  required Size size,
  required AppState state,
  String? artwork,
  required String title,
  required String artist,
}) async {
  await _pump(tester, _playerHost(state, artwork: artwork), size);
  final error = tester.takeException();
  await _capture(tester, '${reviewDir.path}/screenshots/player/$id.png');
  final label = '${size.width.toInt()}x${size.height.toInt()}';
  debugPrint(
    'OVERFLOW|$id|$label|${error == null ? "none" : error.toString().replaceAll("\n", " ")}',
  );
  expect(error, isNull, reason: id);
  Rect? rect(Finder f) => f.evaluate().isEmpty ? null : tester.getRect(f.first);
  final cover = rect(find.byType(ArtworkCard));
  final titleRect = rect(find.text(title));
  final artistRect = rect(find.text(artist));
  final progress = rect(find.byKey(const ValueKey('player-progress-bar')));
  final time = rect(
    find
        .byWidgetPredicate(
          (w) =>
              w is Text &&
              w.data != null &&
              RegExp(r'^\d+:\d\d$').hasMatch(w.data!),
        )
        .first,
  );
  final remaining = rect(
    find
        .byWidgetPredicate(
          (w) =>
              w is Text &&
              w.data != null &&
              RegExp(r'^-\d+:\d\d$').hasMatch(w.data!),
        )
        .first,
  );
  final prev = rect(find.byKey(const ValueKey('playback-previous-button')));
  final play = rect(find.byKey(const ValueKey('playback-play-pause-button')));
  final next = rect(find.byKey(const ValueKey('playback-next-button')));
  final volume = rect(find.byKey(const ValueKey('player-volume-capsule')));
  if (progress != null && volume != null) {
    expect(progress.width, greaterThan(40), reason: id);
    final midpoint = (progress.center.dy + volume.center.dy) / 2;
    for (final button in [prev, play, next].nonNulls) {
      expect(button.center.dy, closeTo(midpoint, 1), reason: id);
    }
  }
  final controls = rect(find.byKey(const ValueKey('player-controls-column')));
  for (final e in {
    'cover': cover,
    'title': titleRect,
    'artist': artistRect,
    'progress': progress,
    'time': time,
    'remaining': remaining,
    'prev': prev,
    'play': play,
    'next': next,
    'volume': volume,
    'controls': controls,
  }.entries) {
    if (e.value != null) _metric(id, label, e.key, e.value!);
  }
}

Widget _overlayHost({
  required MorphStage stage,
  required Size size,
  Widget? settingsChild,
}) => RepaintBoundary(
  key: const ValueKey('review-boundary'),
  child: FrostedScene(
    background: _background(),
    child: MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: fusionTheme(),
      home: Scaffold(
        backgroundColor: Colors.transparent,
        body: Stack(
          fit: StackFit.expand,
          children: [
            FusionMorphOverlay(
              stage: stage,
              anchorRect: Rect.fromLTWH(size.width - 66, 10, 36, 36),
              menuItems: [
                FusionMenuItem(
                  icon: const Icon(Icons.settings_rounded),
                  label: '设置',
                  onSelected: () {},
                ),
                FusionMenuItem(
                  icon: const Icon(Icons.system_update_rounded),
                  label: '检查更新',
                  onSelected: () {},
                ),
                FusionMenuItem(
                  icon: const Icon(Icons.description_rounded),
                  label: '导出日志',
                  onSelected: () {},
                ),
                FusionMenuItem(
                  icon: const Icon(Icons.info_rounded),
                  label: '关于',
                  onSelected: () {},
                ),
              ],
              menuFocusNodes: List.generate(4, (_) => FocusNode()),
              settingsTitle: '设置',
              settingsChild: settingsChild ?? const SizedBox.shrink(),
              onDismiss: () {},
              onSettingsClose: () {},
              onClosed: () {},
              settingsMaxWidth: 520,
            ),
          ],
        ),
      ),
    ),
  ),
);

Widget _settings() => SettingsView(
  state: _state(),
  showHeader: false,
  scrollable: false,
  initialFocusNode: FocusNode(),
  onClose: () {},
  onReceiverName: (_) {},
  onStartup: (_) {},
  onAutoWake: (_) {},
  onAdvancedEffects: (_) {},
  onMiPlay: (_) {},
  onMiPlayIdentity: (_) {},
  onAirPlay: (_) {},
  onDlna: (_) {},
  onExportLogs: () async {},
  isAndroid: true,
);

void main() {
  tearDownAll(() {
    if (Platform.environment['FUSIONPLAY_AUDIT'] != '1') {
      reviewDir.deleteSync(recursive: true);
    }
  });
  setUpAll(() async {
    final loader = FontLoader('MiSans')
      ..addFont(rootBundle.load('assets/fonts/MiSansUI.ttf'));
    await loader.load();
    final icons = FontLoader('MaterialIcons')
      ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
    await icons.load();
  });

  testWidgets('photographic cover lighting study', (tester) async {
    final savedShadows = debugDisableShadows;
    debugDisableShadows = false;
    try {
      final photo = File(
        '../fusionplay-ui-audit-codex/cover-study/alpenglow.jpg',
      );
      final source =
          'data:image/jpeg;base64,${base64Encode(photo.readAsBytesSync())}';
      await tester.pumpWidget(const MaterialApp(home: SizedBox()));
      await tester.runAsync(
        () => precacheImage(
          artworkProvider(source)!,
          tester.element(find.byType(SizedBox).last),
        ),
      );
      for (final lyrics in [false, true]) {
        await _pump(
          tester,
          _playerHost(
            _state(withLyrics: lyrics),
            artwork: source,
            backdrop: PlayerBackdrop(source: source, hasPlayback: true),
          ),
          const Size(1280, 597),
        );
        await tester.pump(const Duration(milliseconds: 400));
        expect(tester.takeException(), isNull);
        await _capture(
          tester,
          '${reviewDir.path}/cover-study/${lyrics ? "lyrics" : "player"}.png',
        );
      }
    } finally {
      debugDisableShadows = savedShadows;
    }
  });

  testWidgets('round2 player captures P01-P20', (tester) async {
    final specs = <(String, Size, AppState, String?, String, String)>[
      (
        'P01',
        const Size(900, 400),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P02',
        const Size(900, 400),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P03',
        const Size(980, 420),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P04',
        const Size(980, 420),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P05',
        const Size(840, 360),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P06',
        const Size(840, 360),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P07',
        const Size(620, 320),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P08',
        const Size(620, 320),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P09',
        const Size(1280, 800),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P10',
        const Size(1280, 800),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P11',
        const Size(1920, 1080),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P12',
        const Size(1920, 1080),
        _state(withLyrics: false),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P13',
        const Size(899, 400),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P14',
        const Size(900, 349),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P15',
        const Size(900, 350),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P16',
        const Size(900, 460),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P17',
        const Size(900, 461),
        _state(),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P18',
        const Size(980, 420),
        _state(
          title: 'This Is An Extremely Long Song Title That Should Ellipsize',
          artist: 'An Extremely Long Artist Name · A Very Long Album Name',
        ),
        coverData,
        'This Is An Extremely Long Song Title That Should Ellipsize',
        'An Extremely Long Artist Name · A Very Long Album Name',
      ),
      (
        'P19',
        const Size(980, 420),
        _state(withLyrics: false),
        null,
        'Starlight',
        'Muse · Drones',
      ),
      (
        'P20',
        const Size(980, 420),
        _state(playing: false, positionMs: 205000, durationMs: 210000),
        coverData,
        'Starlight',
        'Muse · Drones',
      ),
    ];
    // Warm up artwork/glass once.
    await _capturePlayer(
      tester,
      id: '_warmup',
      size: const Size(980, 420),
      state: _state(),
      title: 'Starlight',
      artist: 'Muse · Drones',
    );
    Rect? previousCover;
    for (final (id, size, state, art, title, artist) in specs) {
      await _capturePlayer(
        tester,
        id: id,
        size: size,
        state: state,
        artwork: art,
        title: title,
        artist: artist,
      );
      final cover = tester.getRect(find.byType(ArtworkCard).first);
      if (id == 'P15' || id == 'P17') {
        expect((cover.center - previousCover!.center).distance, lessThan(3));
      }
      previousCover = cover;
    }
  });

  testWidgets('round2 glass overlays', (tester) async {
    await _pump(
      tester,
      _overlayHost(stage: MorphStage.menu, size: const Size(980, 420)),
      const Size(980, 420),
      elapsed: Duration.zero,
    );
    final output = Directory('${reviewDir.path}/screenshots/glass')
      ..createSync(recursive: true);
    final frames = Directory('/tmp/fusionplay-preview/menu-frames')
      ..createSync(recursive: true);
    for (var frame = 0; frame <= 16; frame++) {
      if (frame > 0) await tester.pump(const Duration(milliseconds: 17));
      final path =
          '${frames.path}/frame-${frame.toString().padLeft(2, '0')}.png';
      await _capture(tester, path);
      if (frame == 4 || frame == 8 || frame == 16) {
        final name = frame == 4
            ? 'G04'
            : frame == 8
            ? 'G05'
            : 'menu-open';
        File(path).copySync('${output.path}/$name.png');
      }
    }
    await _pump(
      tester,
      _overlayHost(
        stage: MorphStage.settings,
        size: const Size(980, 420),
        settingsChild: _settings(),
      ),
      const Size(980, 420),
    );
    await _capture(tester, '${reviewDir.path}/screenshots/glass/G06.png');
    await tester.drag(
      find.byKey(const ValueKey('settings-scroll')),
      const Offset(0, -220),
    );
    await tester.pump(const Duration(milliseconds: 300));
    await _capture(tester, '${reviewDir.path}/screenshots/glass/G07.png');

    await _pump(
      tester,
      RepaintBoundary(
        key: const ValueKey('review-boundary'),
        child: FrostedScene(
          background: _background(),
          child: MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: fusionTheme(),
            home: Scaffold(
              backgroundColor: Colors.transparent,
              body: SafeArea(
                child: Align(
                  alignment: Alignment.topCenter,
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: FrostedGlass(
                      level: GlassLevel.panel,

                      borderRadius: 22,
                      padding: const EdgeInsets.fromLTRB(18, 8, 6, 8),
                      child: const Text(
                        '同步妙播音量失败',
                        style: TextStyle(color: Colors.white, fontSize: 14),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
      const Size(980, 420),
    );
    await _capture(tester, '${reviewDir.path}/screenshots/glass/G09.png');

    await _pump(
      tester,
      RepaintBoundary(
        key: const ValueKey('review-boundary'),
        child: FrostedScene(
          background: _background(),
          child: MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: fusionTheme(),
            home: Scaffold(
              backgroundColor: Colors.transparent,
              body: Center(
                child: GlassDialog(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: const [
                      Text(
                        '关于 FusionPlay',
                        style: TextStyle(fontSize: 18, color: Colors.white),
                      ),
                      SizedBox(height: 12),
                      Text(
                        '版本 1.7.2\n设备名：FusionPlay',
                        style: TextStyle(
                          fontSize: 14,
                          color: Color(0xB8FFFFFF),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
      const Size(980, 420),
    );
    await _capture(tester, '${reviewDir.path}/screenshots/glass/G10.png');

    await _pump(
      tester,
      RepaintBoundary(
        key: const ValueKey('review-boundary'),
        child: FrostedScene(
          background: const _GridBackdrop(),
          child: Scaffold(
            backgroundColor: Colors.transparent,
            body: Stack(
              fit: StackFit.expand,
              children: [
                SafeArea(
                  child: PlayerView(
                    state: _state(),
                    artworkSource: coverData,
                    artworkIdentity: 'review-cover',
                    onPlayback: (_) {},
                    onSeek: (_) {},
                    onVolume: (_) {},
                    onOpenMenu: () {},
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
      const Size(980, 420),
    );
    await _capture(
      tester,
      '${reviewDir.path}/screenshots/glass/D02-grid-980x420.png',
    );
  });
  testWidgets('portrait lyrics retain controls and scrolling space', (
    tester,
  ) async {
    await _pump(
      tester,
      _playerHost(_state(), artwork: coverData),
      const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    await _capture(tester, '${reviewDir.path}/screenshots/player/portrait.png');
  });

  testWidgets('capture update routes through Navigator', (tester) async {
    late BuildContext home;
    final pending = Completer<int>();
    await _pump(
      tester,
      RepaintBoundary(
        key: const ValueKey('review-boundary'),
        child: FrostedScene(
          background: _background(),
          child: MaterialApp(
            theme: fusionTheme(),
            debugShowCheckedModeBanner: false,
            home: Builder(
              builder: (context) {
                home = context;
                return const Scaffold(backgroundColor: Colors.transparent);
              },
            ),
          ),
        ),
      ),
      const Size(900, 400),
    );
    Future<void> captureRoute(String name) async {
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      await _capture(tester, '${reviewDir.path}/screenshots/glass/$name.png');
      expect(tester.takeException(), isNull);
    }

    unawaited(
      showBusyDialog<int>(
        context: home,
        message: '正在检查更新…',
        task: () => pending.future,
      ),
    );
    await captureRoute('update-progress');
    pending.complete(1);
    await tester.pumpAndSettle();
    unawaited(
      showAvailableUpdateDialog(
        home,
        const UpdateCheckResult(
          status: UpdateCheckStatus.available,
          versionName: '2.0',
          notes: '更新说明',
        ),
        download: (_, _) async {},
        install: (_) async =>
            const UpdateInstallResult(status: UpdateInstallStatus.started),
      ),
    );
    await captureRoute('update-confirmation');
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    unawaited(showUpdateMessage(home, '检查更新失败', '请稍后重试'));
    await captureRoute('update-error');
  });

  testWidgets('player adapts to large type and different pixel ratios', (
    tester,
  ) async {
    for (final ratio in [1.0, 3.0]) {
      await _pump(
        tester,
        Builder(
          builder: (context) => MediaQuery(
            data: MediaQuery.of(
              context,
            ).copyWith(textScaler: const TextScaler.linear(1.5)),
            child: _playerHost(
              _state(title: '长标题 · A very long song name'),
              artwork: coverData,
            ),
          ),
        ),
        const Size(900, 400),
        dpr: ratio,
      );
      expect(tester.takeException(), isNull);
      expect(
        find.byKey(const ValueKey('playback-play-pause-button')),
        findsOneWidget,
      );
    }
  });
}

class _GridBackdrop extends StatelessWidget {
  const _GridBackdrop();
  @override
  Widget build(BuildContext context) => CustomPaint(painter: _GridPainter());
}

class _GridPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(
      Offset.zero & size,
      Paint()..color = const Color(0xFF101820),
    );
    final paint = Paint()
      ..color = const Color(0xFFFFFFFF)
      ..strokeWidth = 3;
    for (var x = 0.0; x < size.width; x += 40) {
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), paint);
    }
    for (var y = 0.0; y < size.height; y += 40) {
      canvas.drawLine(Offset(0, y), Offset(size.width, y), paint);
    }
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}
