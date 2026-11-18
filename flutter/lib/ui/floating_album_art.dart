import 'package:flutter/material.dart';

/// Floating album art: a soft ambient shadow, the unobscured square artwork,
/// and a hairline rim light. The shadow is deliberately painted outside the
/// artwork's clip so it never becomes a black plinth under the cover.
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
    return SizedBox(
      width: size,
      height: size,
      child: Stack(
        alignment: Alignment.center,
        clipBehavior: Clip.none,
        children: [
          // A soft, blurred ellipse-like spread. No solid black offset card.
          // Wide, low-opacity diffusion: no hard edge and no solid plinth.
          Container(
            width: size * .92,
            height: size * .92,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(size * .10),
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withValues(alpha: .16),
                  blurRadius: size * .16,
                  spreadRadius: 0,
                  offset: Offset(0, size * .05),
                ),
                BoxShadow(
                  color: Colors.white.withValues(alpha: .05),
                  blurRadius: size * .08,
                ),
              ],
            ),
          ),
          ClipRRect(
            borderRadius: BorderRadius.circular(r),
            child: SizedBox.square(dimension: size, child: child),
          ),
          IgnorePointer(
            child: Container(
              width: size,
              height: size,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(r),
                border: Border.all(
                  color: Colors.white.withValues(alpha: .10),
                  width: .6,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
