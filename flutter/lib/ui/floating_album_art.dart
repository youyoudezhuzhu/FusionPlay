import 'package:flutter/material.dart';

/// A floating cover with a diffused ambient shadow, not an offset solid card.
/// Shadows live outside the artwork clip; the image stays sharp.
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
          // The radial diffusion has no rectangular fill or hard lower edge.
          Positioned.fill(
            child: IgnorePointer(
              child: CustomPaint(
                painter: _AlbumAmbientShadow(),
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
                  color: Colors.white.withValues(alpha: .09),
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

class _AlbumAmbientShadow extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width * .5, size.height * .54);
    final radius = size.width * .72;
    final paint = Paint()
      ..shader = RadialGradient(
        colors: [
          Colors.black.withValues(alpha: .23),
          Colors.black.withValues(alpha: .10),
          Colors.black.withValues(alpha: 0),
        ],
        stops: const [0, .52, 1],
      ).createShader(Rect.fromCircle(center: center, radius: radius));
    canvas.drawCircle(center, radius, paint);
  }

  @override
  bool shouldRepaint(covariant _AlbumAmbientShadow oldDelegate) => false;
}
