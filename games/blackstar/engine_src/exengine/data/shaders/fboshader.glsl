#START VS
#version 330 core

layout (location = 0) in vec2 in_position;
layout (location = 1) in vec2 in_uv;

out vec2 uv;

void main() 
{
  gl_Position = vec4(in_position.x, in_position.y, 0.0f, 1.0f);
  uv = in_uv;
}
#END VS


#START FS
#version 330 core

in vec2 uv;

out vec4 color;

uniform sampler2D u_texture;
uniform float u_gfx_quality;   // 0 mobile .. 3 ultra
uniform vec2  u_sun_uv;        // the planetary sun's screen position
uniform float u_sun_vis;       // 0 hidden .. 1 full lens flare
uniform vec3  u_grade_warm;    // per-world highlight tint (split-tone cinematography)
uniform vec3  u_grade_cool;    // per-world shadow tint
uniform float u_time;          // for the animated film grain
uniform float u_exposure;      // eye adaptation (1 day .. ~1.5 night, lagging)
uniform float u_upscale;       // 1/render_scale: >1 = reconstructing a sub-native frame
uniform sampler2D u_depth;     // the canvas depth (SSAO + depth-aware post)

// camera planes (must match camera.c's mat4x4_perspective: near 0.05, far 4000)
float lin_depth(float d) {
  float n = 0.05, f = 4000.0;
  return 2.0 * n * f / (f + n - (2.0*d - 1.0) * (f - n));
}

vec3 aces_tonemap(vec3 x) {
  float a = 2.51;
  float b = 0.03;
  float c = 2.43;
  float d = 0.59;
  float e = 0.14;
  return clamp((x*(a*x+b))/(x*(c*x+d)+e), 0.0, 1.0);
}

const vec3 u_white_point = vec3(1.6, 1.6, 1.6);

float luma(vec3 c) { return dot(c, vec3(0.299, 0.587, 0.114)); }

void main()
{
  vec3 tex_color = textureLod(u_texture, uv, 0.0).rgb;
  // --- chromatic aberration (high+): channels split radially toward the frame edges,
  // the classic anamorphic-lens fringe. Center stays perfectly sharp. ---
  if (u_gfx_quality >= 1.5) {
    vec2 cc = uv - 0.5;
    float cd2 = dot(cc, cc);
    vec2 caoff = cc * cd2 * 0.030;
    tex_color.r = textureLod(u_texture, uv - caoff, 0.0).r;
    tex_color.b = textureLod(u_texture, uv + caoff, 0.0).b;
  }

  // --- FXAA: luma edge-detect + directional blend to soften geometry jaggies.
  // Skipped on mobile (quality 0) where the edge taps aren't worth the cost. ---
  if (u_gfx_quality >= 0.5) {
    vec2 texel = 1.0 / vec2(textureSize(u_texture, 0));
    float lM = luma(tex_color);
    float lN = luma(textureLod(u_texture, uv + vec2( 0.0,-1.0)*texel, 0.0).rgb);
    float lS = luma(textureLod(u_texture, uv + vec2( 0.0, 1.0)*texel, 0.0).rgb);
    float lE = luma(textureLod(u_texture, uv + vec2( 1.0, 0.0)*texel, 0.0).rgb);
    float lW = luma(textureLod(u_texture, uv + vec2(-1.0, 0.0)*texel, 0.0).rgb);
    float lmin = min(lM, min(min(lN,lS), min(lE,lW)));
    float lmax = max(lM, max(max(lN,lS), max(lE,lW)));
    if (lmax - lmin > 0.055) {                       // only on real edges
      vec2 dir = normalize(vec2(-(lN - lS), (lE - lW)) + 1e-5) * texel * 1.5;
      vec3 s1 = 0.5 * (textureLod(u_texture, uv - dir*0.17, 0.0).rgb
                     + textureLod(u_texture, uv + dir*0.17, 0.0).rgb);
      vec3 s2 = s1*0.5 + 0.25 * (textureLod(u_texture, uv - dir*0.5, 0.0).rgb
                               + textureLod(u_texture, uv + dir*0.5, 0.0).rgb);
      float l2 = luma(s2);
      tex_color = (l2 < lmin || l2 > lmax) ? s1 : s2;   // reject overshoot
    }
  }

  // --- UPSCALE RECONSTRUCTION (unendlich architecture, honest model): when the frame
  // was rendered sub-native (render scale < 1), a contrast-adaptive SHARPEN recovers
  // edge definition from the bilinear upsample -- copies/weights real pixels, never
  // hallucinates (the neural model's weights aren't shipped; this is the 1x/2x path). ---
  if (u_upscale > 1.05) {
    vec2 st = 1.0 / vec2(textureSize(u_texture, 0));
    vec3 cN = textureLod(u_texture, uv + vec2(0.0, -st.y), 0.0).rgb;
    vec3 cS = textureLod(u_texture, uv + vec2(0.0,  st.y), 0.0).rgb;
    vec3 cE = textureLod(u_texture, uv + vec2( st.x, 0.0), 0.0).rgb;
    vec3 cW = textureLod(u_texture, uv + vec2(-st.x, 0.0), 0.0).rgb;
    vec3 mn = min(min(cN, cS), min(cE, cW));
    vec3 mx = max(max(cN, cS), max(cE, cW));
    vec3 amp2 = clamp(min(mn, 1.0 - mx) / max(mx, vec3(0.001)), 0.0, 1.0);
    float w4 = -0.22 * clamp((u_upscale - 1.0), 0.0, 1.0);
    vec3 wamp = amp2 * w4;
    tex_color = clamp((tex_color + (cN + cS + cE + cW) * wamp) / (1.0 + 4.0 * wamp), 0.0, 1.0);
  }
  // --- CINEMATIC FAR-FIELD DOF (high+): beyond ~120u the frame softens gently (a
  // long-lens atmosphere blur), full by ~500u. Gameplay range stays tack-sharp; the
  // horizon breathes like a photographed one. Depth-weighted taps avoid silhouette
  // bleed from sharp foreground onto the blurred field. ---
  if (u_gfx_quality >= 1.5) {
    float zf = lin_depth(textureLod(u_depth, uv, 0.0).r);
    float coc = smoothstep(120.0, 500.0, zf);
    if (coc > 0.01) {
      vec2 texel3 = 1.0 / vec2(textureSize(u_texture, 0));
      vec3 acc3 = tex_color; float wsum3 = 1.0;
      float rad3 = coc * 3.2;
      for (int k3 = 0; k3 < 6; k3++) {
        float a3 = float(k3) * 1.047198 + 0.39;
        vec2 o3 = vec2(cos(a3), sin(a3)) * rad3 * texel3 * (1.0 + float(k3 & 1));
        float zs3 = lin_depth(textureLod(u_depth, uv + o3, 0.0).r);
        float w3 = smoothstep(100.0, 160.0, zs3);          // only the far field contributes
        acc3 += textureLod(u_texture, uv + o3, 0.0).rgb * w3;
        wsum3 += w3;
      }
      tex_color = mix(tex_color, acc3 / wsum3, coc);
    }
  }
  // --- SSAO (high+): depth-only ambient occlusion -- a golden spiral of taps around
  // each pixel; neighbours meaningfully NEARER the camera occlude it. Range-checked so
  // distant silhouettes don't halo, radius shrinking with distance so it stays a
  // crease/contact effect. Darkens the frame before exposure: corners, seams, and the
  // ground under everything gain natural weight. ---
  if (u_gfx_quality >= 1.5) {
    float zc = lin_depth(textureLod(u_depth, uv, 0.0).r);
    if (zc < 320.0) {                              // beyond this fog owns the frame anyway
      vec2 texel2 = 1.0 / vec2(textureSize(u_texture, 0));
      float radpx = clamp(620.0 / zc, 3.0, 42.0);
      float occ = 0.0;
      for (int k2 = 0; k2 < 8; k2++) {
        float ang = float(k2) * 2.399963;                 // golden angle spiral
        float rr2 = radpx * (0.25 + 0.75 * float(k2) / 7.0);
        vec2 off2 = vec2(cos(ang), sin(ang)) * rr2 * texel2;
        float zs = lin_depth(textureLod(u_depth, uv + off2, 0.0).r);
        float dz = zc - zs;                                // positive = neighbour nearer
        occ += smoothstep(0.10, 0.55, dz) * (1.0 - smoothstep(2.2, 4.5, dz));
      }
      float ao = 1.0 - 0.55 * (occ / 8.0);
      tex_color *= ao;
    }
  }
  // --- bloom: gather bright HDR energy from a 7x7 neighborhood and add it back as
  // glow, BEFORE tonemapping so it blooms naturally. Skipped on mobile (quality 0)
  // where the 49-tap gather is the single biggest post cost. ---
  if (u_gfx_quality >= 0.5) {
    vec2 texel = 1.0 / vec2(textureSize(u_texture, 0));
    vec3 bloom = vec3(0.0); float wsum = 0.0;
    for (int by = -3; by <= 3; by++) {
      for (int bx = -3; bx <= 3; bx++) {
        vec3 s = textureLod(u_texture, uv + vec2(bx, by) * texel * 2.5, 0.0).rgb;
        float b = max(max(s.r, s.g), s.b);
        float w = exp(-float(bx*bx + by*by) * 0.14);
        bloom += s * smoothstep(0.68, 1.6, b) * w;
        wsum  += w;
      }
    }
    tex_color += (bloom / wsum) * 0.85;   // richer energon bloom
  }

  // Exponential exposure. The NEGATIVE sign is essential: 1 - exp(-x) maps
  // [0,inf) -> [0,1). Without it, any value above ~0.1 goes negative and the
  // ACES pass below flips it past 1.0 -> pure white (the "everything is white"
  // bug that showed up once scene brightness rose).
  {
    float expo = (u_exposure < 0.05) ? 1.0 : u_exposure;   // unset-uniform guard
    tex_color = vec3(1.0) - exp(-tex_color * expo / u_white_point);
  }
  vec3 c = aces_tonemap(tex_color);

  // --- filmic grade: a gentle contrast lift + a touch of saturation ---
  c = pow(c, vec3(0.94));
  float lgy = luma(c);
  c = clamp(mix(vec3(lgy), c, 1.12), 0.0, 1.0);
  // --- per-world SPLIT-TONE: shadows lean into the world's cool tint, highlights into
  // its warm -- each map gets a coherent colour-grade derived from its own lighting. ---
  {
    vec3 gw = (dot(u_grade_warm,u_grade_warm) < 0.001) ? vec3(1.0) : u_grade_warm;
    vec3 gc2 = (dot(u_grade_cool,u_grade_cool) < 0.001) ? vec3(1.0) : u_grade_cool;
    vec3 tone = mix(gc2, gw, smoothstep(0.15, 0.85, lgy));
    c = clamp(c * mix(vec3(1.0), tone, 0.30), 0.0, 1.0);
  }
  // --- animated film GRAIN (balanced+): fine, luma-weighted (stronger in shadow) ---
  if (u_gfx_quality >= 0.5) {
    float gr = fract(sin(dot(gl_FragCoord.xy + vec2(u_time*61.7, u_time*41.3), vec2(12.9898, 78.233))) * 43758.5453);
    c += (gr - 0.5) * 0.028 * (1.0 - lgy*0.7);
  }

  // --- LENS FLARE (balanced+): a warm halo at the sun, ghost dots mirrored along the
  // line through screen centre, and a thin anamorphic streak -- all scaled by the
  // C-side visibility (screen edges, horizon, live sun strength: flares FLARE it). ---
  if (u_gfx_quality >= 0.5 && u_sun_vis > 0.001) {
    // OCCLUSION CHEAT (no depth buffer needed): sample the rendered frame in a small
    // cross around the sun's uv -- if those pixels are dark, geometry is in the way and
    // the flare dims. The visible sun spot is always the brightest thing on screen
    // (god-ray core + sky), so frame luma is an honest proxy.
    vec2 texel2 = 1.0 / vec2(textureSize(u_texture, 0));
    float socc = 0.0;
    socc += luma(textureLod(u_texture, clamp(u_sun_uv, 0.0, 1.0), 0.0).rgb);
    socc += luma(textureLod(u_texture, clamp(u_sun_uv + vec2( 6.0, 0.0)*texel2, 0.0, 1.0), 0.0).rgb);
    socc += luma(textureLod(u_texture, clamp(u_sun_uv + vec2(-6.0, 0.0)*texel2, 0.0, 1.0), 0.0).rgb);
    socc += luma(textureLod(u_texture, clamp(u_sun_uv + vec2(0.0,  6.0)*texel2, 0.0, 1.0), 0.0).rgb);
    socc += luma(textureLod(u_texture, clamp(u_sun_uv + vec2(0.0, -6.0)*texel2, 0.0, 1.0), 0.0).rgb);
    float occ = smoothstep(0.10, 0.42, socc * 0.2);
    vec2 asp = vec2(1.78, 1.0);                     // aspect-ish correction for round halos
    vec2 dsun = (uv - u_sun_uv) * asp;
    float dcore = length(dsun);
    vec3 fl = vec3(0.0);
    fl += vec3(1.0, 0.86, 0.62) * 0.55 * exp(-dcore*dcore*38.0);            // the halo
    fl += vec3(1.0, 0.72, 0.42) * 0.16 * exp(-dcore*dcore*5.0);             // wide bloom
    fl += vec3(1.0, 0.85, 0.60) * 0.20 * exp(-abs(dsun.y)*90.0) * exp(-abs(dsun.x)*3.4);  // anamorphic streak
    vec2 toc = (vec2(0.5) - u_sun_uv);              // ghosts march toward screen centre
    for (int gi = 1; gi <= 3; gi++) {
      vec2 gp = u_sun_uv + toc * (0.7 * float(gi));
      float gd = length((uv - gp) * asp);
      float gs = 0.05 / float(gi);
      fl += vec3(0.55, 0.75, 1.0) * gs * exp(-gd*gd*220.0*float(gi));
    }
    c += fl * u_sun_vis * occ;
  }
  // --- vignette: subtle cinematic edge darkening ---
  vec2 q = uv - 0.5;
  c *= 1.0 - dot(q, q) * 0.5;

  // --- ordered dither: break up gradient banding before 8-bit output ---
  float dith = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
  c += (dith - 0.5) / 255.0;

  color = vec4(c, 1.0);
}
#END FS