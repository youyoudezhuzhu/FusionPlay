import 'package:flutter/material.dart';

/// A sharp artwork face with a fine bevel and a soft, downward cast shadow.
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
    final corners = BorderRadius.circular(radius ?? size * .075);
    // Cap elevation on large displays; a TV cover should still feel close
    // to the background. Negative spread keeps the shadow off the top edge.
    final lift = (size * .048).clamp(5.0, 16.0).toDouble();
    return SizedBox.square(
      dimension: size,
      child: DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: corners,
          boxShadow: [
            BoxShadow(
              color: const Color(0x59070812),
              offset: Offset(0, lift),
              blurRadius: lift * 1.65,
              spreadRadius: -lift * .35,
            ),
            BoxShadow(
              color: const Color(0x40040810),
              offset: Offset(0, lift * .38),
              blurRadius: lift * .5,
              spreadRadius: -lift * .12,
            ),
          ],
        ),
        child: ClipRRect(
          borderRadius: corners,
          child: CustomPaint(
            foregroundPainter: _CoverBevel(corners),
            child: child,
          ),
        ),
      ),
    );
  }
}

class _CoverBevel extends CustomPainter {
  const _CoverBevel(this.corners);

  final BorderRadius corners;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    final edge = corners.toRRect(rect).deflate(.6);
    canvas.drawRRect(
      edge,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.2
        ..shader = const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0x44FFFFFF), Color(0x09FFFFFF), Color(0x38060A16)],
          stops: [0, .42, 1],
        ).createShader(rect),
    );
  }

  @override
  bool shouldRepaint(_CoverBevel oldDelegate) => oldDelegate.corners != corners;
}
