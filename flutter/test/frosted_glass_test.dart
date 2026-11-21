import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fusionplay_android_flutter/ui/frosted_glass.dart';

void main() {
  Widget host(Widget child) => MaterialApp(
    home: Scaffold(
      backgroundColor: const Color(0xFF1B2330),
      body: Center(child: SizedBox(width: 220, height: 120, child: child)),
    ),
  );

  testWidgets('clips and blurs the live background when enabled', (
    tester,
  ) async {
    await tester.pumpWidget(
      host(const FrostedGlass(child: Center(child: Text('surface')))),
    );
    expect(find.byType(ClipRRect), findsOneWidget);
    expect(find.byType(BackdropFilter), findsOneWidget);
    expect(find.text('surface'), findsOneWidget);
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
    final dialog = find.byType(GlassDialog);
    expect(
      find.descendant(of: dialog, matching: find.byType(BackdropFilter)),
      findsOneWidget,
    );
    final route = ModalRoute.of(tester.element(dialog))!;
    expect(route.barrierColor!.a, lessThan(.2));
    expect(find.text('glass dialog content'), findsOneWidget);
    expect(tester.binding.transientCallbackCount, 0);
  });
}
