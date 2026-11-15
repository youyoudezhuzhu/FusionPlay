import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/liquid_glass.dart';

void main() {
  Widget host(Widget child) => MaterialApp(
    home: Scaffold(
      backgroundColor: const Color(0xFF1B2330),
      body: Center(child: SizedBox(width: 220, height: 120, child: child)),
    ),
  );

  testWidgets('renders frosted fallback and its child', (tester) async {
    await tester.pumpWidget(
      host(
        const LiquidGlass(
          forceFallback: true,
          child: Center(child: Text('glass')),
        ),
      ),
    );
    expect(find.text('glass'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  test('glass animation is opt-in (static by default, avoids flicker)', () {
    expect(const LiquidGlass(child: SizedBox()).animate, isFalse);
  });

  test('unified glass levels expose distinct tokens', () {
    final card = GlassSpec.of(GlassLevel.card);
    final control = GlassSpec.of(GlassLevel.control);
    final panel = GlassSpec.of(GlassLevel.panel);
    expect(card.blur, lessThan(panel.blur));
    expect(card.tint, isNot(panel.tint));
    expect(control.rimAlpha, greaterThan(card.rimAlpha));
    expect(panel.shadowAlpha, greaterThan(control.shadowAlpha));
  });

  testWidgets('glass is static: no shader/ticker even when animate is set', (
    tester,
  ) async {
    await tester.pumpWidget(
      host(
        const LiquidGlass(
          animate: true,
          interactive: true,
          level: GlassLevel.control,
          child: Center(child: Text('static glass')),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('static glass'), findsOneWidget);
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('dynamic specular highlight follows pointer without ticking', (
    tester,
  ) async {
    await tester.pumpWidget(
      host(
        const LiquidGlass(
          interactive: true,
          level: GlassLevel.control,
          child: Center(child: Text('pointer glass')),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final center = tester.getCenter(find.byType(LiquidGlass));
    final gesture = await tester.createGesture();
    await gesture.down(center);
    await tester.pump();
    await gesture.moveBy(const Offset(24, -12));
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('GlassSurface renders token-based nested surface', (
    tester,
  ) async {
    await tester.pumpWidget(
      host(const GlassSurface(child: Center(child: Text('surface')))),
    );
    expect(find.text('surface'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('showGlassDialog uses the unified GlassDialog surface', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showGlassDialog<void>(
                context: context,
                dialogKey: const ValueKey('test-glass-dialog'),
                builder: (_) => const Text('glass dialog content'),
              ),
              child: const Text('open'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    expect(find.byType(GlassDialog), findsOneWidget);
    expect(find.byKey(const ValueKey('test-glass-dialog')), findsOneWidget);
    expect(find.text('glass dialog content'), findsOneWidget);
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('supports capsule radius and disable animations', (tester) async {
    await tester.pumpWidget(
      host(
        const LiquidGlass(
          forceFallback: true,
          borderRadius: 999,
          animate: false,
          child: Center(child: Icon(Icons.more_horiz)),
        ),
      ),
    );
    expect(find.byIcon(Icons.more_horiz), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
