#version 460 core

// Real screen-space refraction for LiquidGlass.
//
// The renderer captures a *background-only* RepaintBoundary to a texture, so
// this shader samples actual backdrop pixels instead of the framebuffer that
// contains the glass itself. That avoids the Flutter/Impeller feedback loop
// that made ImageFilter.shader flash white when several glass surfaces repaint.
#include <flutter/runtime_effect.glsl>

uniform vec2 uSize;         // glass size in logical pixels
uniform vec2 uOrigin;       // glass top-left in global logical pixels
uniform vec2 uImageSize;    // captured texture size in physical pixels
uniform float uDpr;
uniform float uRadius;
uniform float uRefraction;
uniform float uThickness;
uniform float uSpecular;
uniform float uAbsorption;
uniform vec4 uTint;
uniform vec2 uAxisX;
uniform vec2 uAxisY;

uniform sampler2D uBackdrop;

out vec4 fragColor;

float roundedRectSdf(vec2 p, vec2 halfSize, float radius) {
  vec2 q = abs(p) - (halfSize - vec2(radius));
  return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float sdfAt(vec2 p, vec2 halfSize, float radius) {
  return roundedRectSdf(p, halfSize, radius);
}

void main() {
  vec2 fragPx = FlutterFragCoord().xy;
  float dpr = max(uDpr, 0.001);
  vec2 sizePx = max(uSize, vec2(1.0));
  vec2 local = fragPx; // Canvas-local logical coordinates.
  vec2 halfSize = sizePx * 0.5;
  vec2 p = local - halfSize;

  float radiusPx = clamp(uRadius, 2.0, min(halfSize.x, halfSize.y));
  float sdf = sdfAt(p, halfSize, radiusPx);

  // Analytic-ish normal from the SDF gradient, valid for circles, capsules and
  // rounded rectangles. This is what makes edge refraction follow the shape.
  float eps = 0.75;
  vec2 gradient = vec2(
    sdfAt(p + vec2(eps, 0.0), halfSize, radiusPx) -
        sdfAt(p - vec2(eps, 0.0), halfSize, radiusPx),
    sdfAt(p + vec2(0.0, eps), halfSize, radiusPx) -
        sdfAt(p - vec2(0.0, eps), halfSize, radiusPx)
  );
  vec2 normal = normalize(gradient + vec2(1e-5));

  float thickness = clamp(uThickness, 1.0, radiusPx * 1.2);
  float edge = 1.0 - clamp(-sdf / thickness, 0.0, 1.0);
  float bevel = edge * edge * (3.0 - 2.0 * edge);

  // Displace the actual captured backdrop toward the rim. The further inside
  // the bevel, the larger the physical path length through the glass.
  float bendPx = thickness * 0.85 * uRefraction * bevel;
  vec2 displacedLocal = local + normal * bendPx;
  vec2 globalLogical = uOrigin + uAxisX * displacedLocal.x + uAxisY * displacedLocal.y;

  // Convert global logical pixels to captured texture UV.
  vec2 imagePx = uImageSize;
  vec2 uv = (globalLogical * dpr) / max(imagePx, vec2(1.0));
  uv = clamp(uv, vec2(0.001), vec2(0.999));

  // Chromatic dispersion: red/blue sample slightly farther from the normal.
  vec3 color = texture(uBackdrop, uv).rgb;
  if (bevel > 0.0 && uRefraction > 0.0) {
    vec2 caOffset = normal * (bevel * 1.2 * uRefraction * dpr) /
        max(imagePx, vec2(1.0));
    color.r = texture(uBackdrop, clamp(uv + caOffset, vec2(0.001), vec2(0.999))).r;
    color.b = texture(uBackdrop, clamp(uv - caOffset, vec2(0.001), vec2(0.999))).b;
  }

  // Beer-Lambert rim absorption and a subtle body tint. Keep the backdrop
  // readable: the glass is a lens, not a painted card.
  color *= 1.0 - uAbsorption * bevel * bevel * 0.65;
  color = mix(color, uTint.rgb, clamp(uTint.a, 0.0, 1.0) * 0.55);

  // Directional specular and grazing-angle Fresnel along the real normal.
  vec2 light = normalize(vec2(-0.55, -0.8));
  float facing = clamp(dot(normal, light), -1.0, 1.0);
  float specular = pow(max(facing, 0.0), 2.2) * pow(bevel, 2.6);
  float fresnel = pow(bevel, 4.0) * 0.32;
  // Lift the transmitted hue only inside the bevel; keep the face clear.
  float glint = clamp((specular * 0.38 + fresnel * 0.65) * uSpecular, 0.0, 1.0);
  color = mix(color, min(vec3(1.0), color * 1.15 + vec3(0.32)), glint);

  fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
