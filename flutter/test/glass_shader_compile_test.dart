import 'package:flutter_test/flutter_test.dart';
import 'dart:ui' as ui;

void main() {
  testWidgets('glass shaders compile', (tester) async {
    for (final asset in ['shaders/liquid_glass_refraction.frag']) {
      try {
        final program = await ui.FragmentProgram.fromAsset(asset);
        expect(program.fragmentShader(), isNotNull);
        expect(program.fragmentShader(), isNotNull);
      } catch (e) {
        fail('shader failed: $asset: $e');
      }
    }
  });
}
