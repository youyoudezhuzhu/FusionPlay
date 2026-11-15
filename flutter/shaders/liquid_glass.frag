#version 460 core

// Liquid-glass backdrop filter (Impeller ImageFilter.shader).
//
// Used as the "outer" filter of ImageFilter.compose(inner: blur, outer: this),
// so the texture we sample is already blurred. The pass models a thick bevelled
// slab of glass:
//  * rounded-rect SDF in physical pixels (uRadius stays in logical pixels),
//  * bevel refraction that bends the backdrop toward the rim,
//  * chromatic dispersion that grows with the bevel slope,
//  * Beer-Lambert rim absorption (dark glass edge),
//  * directional specular + grazing-angle Fresnel rim light,
//  * a slow travelling sheen so the surface feels liquid rather than static.
//
// Uniform order is fixed: the engine binds the first vec2 to the input texture
// size and the first sampler to the input itself.
#include <flutter/runtime_effect.glsl>

uniform vec2 uSize;
uniform float uDpr;
uniform float uRadius;
uniform float uRefraction;
uniform float uSpecular;
uniform float uTime;
uniform vec2 uLight;
uniform float uThickness;
uniform float uAbsorption;
uniform sampler2D uTexture;

out vec4 fragColor;

float roundedRectSdf(vec2 p, vec2 halfSize, float radius) {
  vec2 q = abs(p) - (halfSize - vec2(radius));
  return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
  vec2 sizePx = max(uSize, vec2(1.0));
  vec2 fragPx = FlutterFragCoord().xy;
  float dpr = max(uDpr, 1.0);
  vec2 halfPx = sizePx * 0.5;

  float radiusPx = clamp(uRadius * dpr, 2.0, min(halfPx.x, halfPx.y));
  vec2 centered = fragPx - halfPx;
  float sdf = roundedRectSdf(centered, halfPx, radiusPx);

  // Distance into the bevel band, normalised to 0 (body) .. 1 (rim).
  float bevelPx = clamp(uThickness * dpr, 2.0, radiusPx * 0.95);
  float edge = 1.0 - clamp(-sdf / bevelPx, 0.0, 1.0);
  float bevel = edge * edge * (3.0 - 2.0 * edge); // smoothstep-ish profile

  // Outward direction of the rounded rect; stable analytic approximation.
  vec2 normal = normalize(centered / max(halfPx, vec2(1.0)) + vec2(1e-4));

  vec2 uv = fragPx / sizePx;
  // Refraction: the further into the bevel, the further the sample is bent
  // toward the rim, mimicking a thick lens.
  float bendPx = bevelPx * 0.62 * uRefraction * bevel;
  vec2 shift = (normal * bendPx) / sizePx;

  // Chromatic dispersion grows with the bevel slope.
  float caPx = (0.35 + 2.0 * bevel) * dpr * uRefraction;
  vec2 caShift = (normal * caPx) / sizePx;

  vec2 baseUv = clamp(uv + shift, vec2(0.001), vec2(0.999));
  vec3 color = vec3(
    texture(uTexture, clamp(uv + shift + caShift, vec2(0.001), vec2(0.999))).r,
    texture(uTexture, baseUv).g,
    texture(uTexture, clamp(uv + shift - caShift, vec2(0.001), vec2(0.999))).b
  );
  float alpha = texture(uTexture, baseUv).a;

  // Beer-Lambert absorption: glass edges read darker than the body.
  color *= 1.0 - uAbsorption * bevel * bevel * 0.85;

  // Directional specular + grazing-angle Fresnel.
  vec2 light = normalize(uLight + vec2(1e-4));
  float facing = clamp(dot(normal, light), -1.0, 1.0);
  float specular = pow(max(facing, 0.0), 2.4) * pow(bevel, 3.0);
  float fresnel = pow(bevel, 5.0) * 0.42;
  color += vec3(1.0) * (specular * 0.75 + fresnel) * uSpecular;

  // Slow travelling sheen across the body.
  float wave = sin((uv.x + uv.y * 0.32) * 6.5 - uTime * 0.55);
  color += vec3(smoothstep(0.88, 1.0, wave) * 0.05 * uSpecular);

  // Very small lift so the material reads as glass rather than a plain blur.
  color = mix(color, color * 1.05 + 0.012, 0.55);

  fragColor = vec4(color, alpha);
}
