import 'dart:io';
import 'dart:ui' as ui;

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/models/app_state.dart';
import 'package:fusionplay_android_flutter/ui/artwork.dart';
import 'package:fusionplay_android_flutter/ui/glass_backdrop.dart';
import 'package:fusionplay_android_flutter/ui/player_view.dart';

final String _coverData =
    'data:image/png;base64,${base64Encode(File('/tmp/fusionplay-cover.png').readAsBytesSync())}';

LyricsDocument _lyrics() => const LyricsDocument(
  origin: 'embedded',
  synced: true,
  lines: [
    LyricLine(
      startMs: 0,
      text: '雨后的城市 安静得像一封没寄出的信',
      words: [
        LyricsWord(startMs: 0, endMs: 600, text: '雨后的城市 '),
        LyricsWord(startMs: 600, endMs: 1600, text: '安静得像'),
      ],
    ),
    LyricLine(startMs: 4000, text: '我把想念折成一只纸船'),
    LyricLine(startMs: 8000, text: '顺着路灯漂流到你的窗前'),
    LyricLine(startMs: 12000, text: '如果晚风也恰好经过'),
    LyricLine(startMs: 16000, text: '请替我说一声晚安'),
  ],
);

AppState _state({required bool withLyrics, bool loading = false}) => AppState(
  initialized: true,
  playback: PlaybackSnapshot(
    title: 'FusionPlay Demo',
    artist: 'Demo Artist · 作曲',
    album: 'AirPlay Sessions',
    durationMs: 180000,
    positionMs: 5200,
    streamActive: true,
    isPlaying: true,
    protocol: 'airplay',
    qualityText: 'AirPlay 2 · ALAC',
    volumePercent: 68,
    lyrics: withLyrics ? _lyrics() : null,
    lyricsLoading: loading,
  ),
  controls: PlaybackControls.all,
);

Widget _host({
  required bool withLyrics,
  required GlobalKey boundaryKey,
  bool loading = false,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: ThemeData(fontFamily: 'MiSans', useMaterial3: true),
    home: Scaffold(
      backgroundColor: const Color(0xFF0B0D11),
      body: RepaintBoundary(
        key: boundaryKey,
        child: GlassBackdropScope(
          revision: 'preview',
          background: Stack(
            fit: StackFit.expand,
            children: [
              ArtworkBackground(source: _coverData, hasPlayback: true),
              const ColoredBox(color: Color(0x73000000)),
            ],
          ),
          child: PlayerView(
            state: _state(withLyrics: withLyrics, loading: loading),
            artworkSource: _coverData,
            artworkIdentity: 'preview-cover',
            onPlayback: (_) {},
            onSeek: (_) {},
            onVolume: (_) {},
            onOpenMenu: () {},
          ),
        ),
      ),
    ),
  );
}

Future<void> _capture(
  WidgetTester tester, {
  required String name,
  required Size size,
  required bool withLyrics,
  bool loading = false,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final key = GlobalKey();
  await tester.pumpWidget(
    _host(withLyrics: withLyrics, boundaryKey: key, loading: loading),
  );
  await tester.pump();
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 300)),
  );
  await tester.pump(const Duration(seconds: 1));
  final boundary =
      key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage(pixelRatio: 2));
  final data = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  final dir = Directory('/tmp/fusionplay-preview')..createSync(recursive: true);
  File('${dir.path}/$name.png').writeAsBytesSync(data!.buffer.asUint8List());
}

void main() {
  setUpAll(() async {
    final loader = FontLoader('MiSans')
      ..addFont(rootBundle.load('assets/fonts/MiSansUI.ttf'));
    await loader.load();
    final icons = FontLoader('MaterialIcons')
      ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
    await icons.load();
  });

  testWidgets('render Apple Music landscape previews', (tester) async {
    await _capture(
      tester,
      name: 'lyrics-960x540',
      size: const Size(960, 540),
      withLyrics: true,
    );
    await _capture(
      tester,
      name: 'lyrics-800x360',
      size: const Size(800, 360),
      withLyrics: true,
    );
    await _capture(
      tester,
      name: 'no-lyrics-960x540',
      size: const Size(960, 540),
      withLyrics: false,
    );
    await _capture(
      tester,
      name: 'no-lyrics-800x360',
      size: const Size(800, 360),
      withLyrics: false,
    );
    await _capture(
      tester,
      name: 'tablet-1280x800',
      size: const Size(1280, 800),
      withLyrics: true,
    );
    await _capture(
      tester,
      name: 'loading-960x540',
      size: const Size(960, 540),
      withLyrics: false,
      loading: true,
    );
  });
}
