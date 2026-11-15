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
