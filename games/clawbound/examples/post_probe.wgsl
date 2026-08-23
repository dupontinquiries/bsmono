// GENERATED from src/main.rs POST_WGSL by maintenance — do not edit.
// The tap expression is replaced with __TAP__ so examples/post_probe.rs can
// render the OLD (source-texel) and NEW (output-pixel) variants from one
// source and they cannot drift apart.

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
struct FxUniform { misc: vec4<f32>, screen: vec4<f32> };  // misc=(nightmare,night,time,danger), screen=(texelX,texelY, outputWpx,outputHpx)
@group(0) @binding(2) var<uniform> fx: FxUniform;
struct VO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> VO {
  var p = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0));
  var o: VO; let q = p[vi]; o.pos = vec4<f32>(q, 0.0, 1.0);
  o.uv = vec2<f32>((q.x + 1.0) * 0.5, 1.0 - (q.y + 1.0) * 0.5); return o;
}
fn h21(p: vec2<f32>) -> f32 {
  var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
  p3 = p3 + dot(p3, vec3<f32>(p3.y, p3.z, p3.x) + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
// ordered 4x4 Bayer threshold (0..1) at a pixel, for dithering
fn bayer4(c: vec2<f32>) -> f32 {
  var m = array<f32,16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
  let x = u32(c.x) & 3u;
  let y = u32(c.y) & 3u;
  return (m[y * 4u + x] + 0.5) / 16.0;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  let uv = i.uv;
  let nightmare = fx.misc.x; let night = fx.misc.y; let t = fx.misc.z;
  // EDGE TAPS ARE ONE OUTPUT PIXEL, NOT ONE SOURCE TEXEL.
  //
  // `fx.screen.xy` is 1/scene_w,1/scene_h — the SUPERSAMPLED texel. In High
  // (AAA) the scene target is 1.85x the window, so that step is 0.54 of an
  // output pixel and this outline filter runs at sub-pixel spacing. Its result
  // then changes between adjacent output pixels for reasons invisible at
  // output resolution, the `edge * 0.8` ink mix follows it, and the 26-level
  // Bayer quantisation at the bottom turns that into a hard dot lattice —
  // the reported "High graphics has rendering issues", strongest over water
  // and sky where the depth gradient is smooth and the taps disagree most.
  //
  // Everything else in this shader already works in OUTPUT pixels (`px = uv *
  // fx.screen.zw` for paper, grain and stars). The edge detector was the only
  // thing in source space, which is why Standard — where the two spaces are
  // identical — looks correct and AAA does not.
  //
  // One output pixel in UV is 1/outputW, and `fx.screen.zw` IS the output size.
  let texel = __TAP__;
  let c = textureSample(tex, samp, uv);
  let dC = c.a;
  let dR = textureSample(tex, samp, uv + vec2<f32>(texel.x, 0.0)).a;
  let dD = textureSample(tex, samp, uv + vec2<f32>(0.0, texel.y)).a;
  let dL = textureSample(tex, samp, uv - vec2<f32>(texel.x, 0.0)).a;
  let dU = textureSample(tex, samp, uv - vec2<f32>(0.0, texel.y)).a;
  let edge = clamp((abs(dC-dR)+abs(dC-dD)+abs(dC-dL)+abs(dC-dU)) * 350.0, 0.0, 1.0);
  let px = uv * fx.screen.zw;
  let paper = 0.94 + 0.06 * h21(floor(px * 0.5));
  let grain = 0.965 + 0.035 * h21(px);
  var col = c.rgb * paper * grain;

  // ── night sky: stars in the far background (dC ~ 1 = nothing drawn), high up ──
  let is_sky = step(0.985, dC) * (1.0 - smoothstep(0.4, 0.78, uv.y));
  let cell = floor(px / 3.0);
  let hs = h21(cell);
  let star = step(0.984, hs) * (0.55 + 0.45 * h21(cell + 5.0) + 0.15 * sin(t * 2.0 + hs * 30.0));
  // the otherworld blots the stars out and swallows the sky
  let star_vis = is_sky * star * (1.0 - nightmare) * max(night, 0.2);
  col = col + vec3<f32>(0.9, 0.92, 1.0) * star_vis;

  // ── otherworld: darken, drain color, long creeping shadow from below ──
  if (nightmare > 0.001) {
    let g = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = mix(col, vec3<f32>(g * 0.72, g * 0.66, g * 0.9), nightmare * 0.7);    // desaturate to a sickly grey-violet
    col = col * (1.0 - nightmare * 0.34);                                        // sky goes dark, not pitch black
    let creep = smoothstep(0.2, 0.98, uv.y) * (0.28 + 0.22 * h21(vec2<f32>(cell.x, floor(t * 1.3))));
    col = col * (1.0 - creep * nightmare * 0.42);                               // long shadows crawl up from the ground
    col = col + vec3<f32>(0.06, 0.03, 0.09) * nightmare;                        // faint violet underglow so shapes read
  }

  // ── suspense: a heartbeat that pounds faster and harder as danger (fx.misc.w) rises,
  //    closing the vignette in on each beat and bleeding the edges red near a catch ──
  let danger = fx.misc.w;
  let hr = 3.0 + danger * 6.5;
  let beat = pow(max(sin(t * hr), 0.0), 6.0);
  let pulse = danger * (0.35 + 0.65 * beat);
  let d = distance(uv, vec2<f32>(0.5, 0.5));
  let vig = 0.55 + nightmare * 0.35 + pulse * 0.55;
  col = col * (1.0 - smoothstep(0.42 - pulse * 0.12, 0.98, d) * vig);
  let edgeglow = smoothstep(0.44, 0.95, d) * pulse;
  col = mix(col, vec3<f32>(0.52, 0.06, 0.05), edgeglow * 0.55);
  let ink = vec3<f32>(0.15, 0.11, 0.09);
  col = mix(col, ink, edge * 0.8);
  // ordered (Bayer) dithering: quantise the colour along a 4x4 threshold pattern,
  // which breaks up smooth-gradient banding and lays down a printed-paper texture.
  let bay = bayer4(px);
  let levels = 26.0;
  col = floor(col * levels + (bay - 0.5)) / levels;
  return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
