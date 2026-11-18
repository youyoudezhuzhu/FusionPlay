import 'dart:math' as math;

import 'package:flutter/material.dart';

/// Artwork that appears gently lifted from the ambient background.
///
/// The image remains sharp. The shadow and ambient bounce light are painted
/// outside its bounds rather than as a dark, offset backing card.
class FloatingAlbumArt extends StatelessWidget {
  const FloatingAlbumArt({
    super.key,
    required this.size,
    required this.child,
    this.radius,
  });

  final double size;
  final double? radius;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final r = radius ?? size * .075;
    return SizedBox.square(
      dimension: size,
      child: Stack(
        alignment: Alignment.center,
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: IgnorePointer(
              child: CustomPaint(
                painter: _FloatingCoverLight(radius: r),
              ),
            ),
          ),
          ClipRRect(
            borderRadius: BorderRadius.circular(r),
            child: SizedBox.square(dimension: size, child: child),
          ),
          IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(r),
                border: Border.all(
                  color: Colors.white.withValues(alpha: .10),
                  width: .65,
                ),
              ),
              child: SizedBox.square(dimension: size),
            ),
          ),
        ],
      ),
    );
  }
}

class _FloatingCoverLight extends CustomPainter {
  const _FloatingCoverLight({required this.radius});

  final double radius;

  @override
  void paint(Canvas canvas, Size size) {
    final side = math.min(size.width, size.height);
    final bounds = Offset.zero & size;
    final rounded = RRect.fromRectAndRadius(
      bounds.deflate(side * .015),
      Radius.circular(radius),
    );

    // Wide, soft penumbra: blurred shadow has no hard rectangular silhouette.
    canvas.drawRRect(
      rounded.shift(Offset(0, side * .022)),
      Paint()
        ..color = Colors.black.withValues(alpha: .25)
        ..maskFilter = MaskFilter.blur(BlurStyle.normal, side * .105),
    );

    // Subtle near-contact occlusion makes the cover feel lifted, not pasted.
    canvas.drawRRect(
      rounded.shift(Offset(0, side * .008)),
      Paint()
        ..color = Colors.black.withValues(alpha: .11)
        ..maskFilter = MaskFilter.blur(BlurStyle.normal, side * .028),
    );

    // Soft environmental bounce below the artwork. Not a visible halo.
    final glowCenter = Offset(side * .5, side * .64);
    final glowRadius = side * .76;
    canvas.drawCircle(
      glowCenter,
      glowRadius,
      Paint()
        ..shader = RadialGradient(
          colors: [
            const Color(0xFFB58B9D).withValues(alpha: .10),
            const Color(0xFFB58B9D).withValues(alpha: .035),
            const Color(0xFFB58B9D).withValues(alpha: 0),
          ],
          stops: const [0, .52, 1],
        ).createShader(Rect.fromCircle(center: glowCenter, radius: glowRadius)),
    );
  }

  @override
  bool shouldRepaint(covariant _FloatingCoverLight oldDelegate) =>
      oldDelegate.radius != radius;
}
