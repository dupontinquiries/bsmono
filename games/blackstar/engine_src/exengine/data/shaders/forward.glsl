#START VS
#version 330 core

layout (location = 0) in vec3 in_position;
layout (location = 1) in vec2 in_uv;
layout (location = 2) in vec3 in_normals;
layout (location = 3) in vec4 in_tangents;
layout (location = 4) in vec4 in_color;
layout (location = 5) in vec4 in_boneindex;
layout (location = 6) in vec4 in_boneweights;
layout (location = 7) in mat4 in_instancematrix;

out vec3 frag;
out vec3 normal;
out vec2 uv;
out mat3 TBN;

uniform mat4 u_view;
uniform mat4 u_projection;
uniform mat4 u_bone_matrix[200];
uniform bool u_has_skeleton;

void main()
{
  mat4 transform = in_instancematrix;
  if (u_has_skeleton == true) {
    vec4 boneindex = in_boneindex*255.0;
    vec4 boneweights = in_boneweights*255.0;
    mat4 skeleton = u_bone_matrix[int(boneindex.x)] * boneweights.x +
                    u_bone_matrix[int(boneindex.y)] * boneweights.y +
                    u_bone_matrix[int(boneindex.z)] * boneweights.z +
                    u_bone_matrix[int(boneindex.w)] * boneweights.w;

    transform = transform * skeleton;
  }

  vec4 v = u_projection * u_view * transform * vec4(in_position, 1.0);

  gl_Position    = v;
  normal         = mat3(transpose(inverse(u_view * transform))) * in_normals;
  frag           = vec3(u_view * in_instancematrix * vec4(in_position, 1.0f));
  uv             = in_uv;

  // calculate tbn matrix for normal mapping
  vec3 T = normalize(vec3(u_view * transform * vec4(in_tangents.xyz, 0.0)));
  vec3 B = normalize(vec3(u_view * transform * in_tangents));
  vec3 N = normalize(vec3(u_view * transform * vec4(in_normals, 0.0)));
  TBN    = mat3(T, B, N);
}
#END VS


#START FS
#version 330 core

out vec4 color;

in vec3 frag;
in vec3 normal;
in vec2 uv;
in mat3 TBN;
uniform sampler2D u_emission;
uniform float u_emission_strength;
uniform float u_mat_boost;   // per-model: amplifies normal detail, spec, reflection (1=default)

uniform sampler2D u_texture;
uniform sampler2D u_spec;
uniform sampler2D u_norm;

uniform mat4 u_projection;
uniform mat4 u_view;
uniform mat4 u_inverse_view;

uniform bool u_ambient_pass;
uniform float u_gfx_quality;   // 0 mobile .. 3 ultra
uniform float u_time;          // seconds, for animated procedural ground
uniform vec3  u_sun_dir;       // planetary sun direction (world), arcs across the day
uniform float u_sun_strength;  // 0 night .. ~0.62 noon (per-world, event-dimmable)
uniform vec3  u_sun_color;     // dawn amber .. noon white .. dying ember
uniform float u_wetness;       // rain/immersion: surfaces go glossier
uniform float u_cloud_amt;     // drifting cloud-shadow strength (0 indoors/space)
uniform sampler2D u_sun_shadow;   // the sun's orthographic depth map (high+)
uniform mat4  u_sun_light_mat;    // world -> sun clip
uniform int   u_sun_shadow_on;
uniform bool u_is_lit;
uniform vec3 u_tint;
uniform bool u_emissive_solid;
uniform vec3 u_fog_color;
uniform float u_fog_density;

uniform samplerCube u_reflection;

/* spot lights */
const int MAX_SL = 32;
struct spot_light {
  vec3 position;
  vec3 direction;
  vec3 color;
  float inner;
  float outer;
  bool is_shadow;
  float far;
};
// for dynamic spotlights
uniform spot_light u_spot_light;
uniform sampler2D  u_spot_depth;
// for static spotlights
uniform spot_light u_spot_lights[MAX_SL];
uniform int        u_spot_count;
uniform bool       u_spot_active;
/* ------------ */

/* dir light */
struct dir_light {
  vec3 position;
  vec3 target;
  vec3 color;
  float far;
};
uniform dir_light u_dir_light;
uniform sampler2D u_dir_depth;
uniform mat4      u_dir_transform;
uniform bool      u_dir_active;
/* ------------ */

/* point light */
const int MAX_PL = 64;
struct point_light {
  vec3 position;
  vec3 color;
  bool is_shadow;
  float far;
};
// for dynamic lights
uniform point_light u_point_light;
uniform samplerCube u_point_depth;
// for static ones done in a single render pass
uniform point_light u_point_lights[MAX_PL];
uniform int         u_point_count;
uniform bool        u_point_active;
/* ------------ */

vec3 pcf_offset[20] = vec3[]
(
  vec3( 1,  1,  1), vec3( 1, -1,  1), vec3(-1, -1,  1), vec3(-1,  1,  1), 
  vec3( 1,  1, -1), vec3( 1, -1, -1), vec3(-1, -1, -1), vec3(-1,  1, -1),
  vec3( 1,  1,  0), vec3( 1, -1,  0), vec3(-1, -1,  0), vec3(-1,  1,  0),
  vec3( 1,  0,  1), vec3(-1,  0,  1), vec3( 1,  0, -1), vec3(-1,  0, -1),
  vec3( 0,  1,  1), vec3( 0, -1,  1), vec3( 0, -1, -1), vec3( 0,  1, -1)
);

vec3 calc_point_light(point_light light)
{
  point_light l = light;
  l.position = vec3(u_view * vec4(l.position, 1.0));

  vec3 fragpos = frag;
  vec3 normals = texture(u_norm, uv).rgb;
  normals = normalize(normals * 2.0 - 1.0);
  normals = normalize(TBN * normals);
  
  vec3 diff    = texture(u_texture, uv).rgb;
  vec3 spec   = texture(u_spec, uv).rgb;

  vec3 view_dir  = normalize(-fragpos);
  vec3 light_dir = l.position - fragpos;
  float dist = length(light_dir);
  light_dir = normalize(light_dir);
  
  // diffuse
  vec3 diffuse   = max(dot(light_dir, normals), 0.0) * diff * l.color;

  // specular -- metallic baseline so surfaces catch light even without a spec
  // map, plus a tighter, brighter highlight for a shiny metal read.
  vec3 halfwayd  = normalize(light_dir + view_dir);
  float specs    = pow(max(dot(normals, halfwayd), 0.0), 48.0f);
  vec3 metal     = spec * 0.6 + 0.18;           // lower metallic floor (was 0.45)
  vec3 specular  = l.color * specs * metal * 0.9;
  // fresnel rim: brighter glint at grazing angles (edge sheen on metal)
  float fres     = pow(1.0 - max(dot(normals, view_dir), 0.0), 3.0) * 0.5;
  specular      += l.color * fres * metal * 0.5;

  // attenuation
  float attenuation = 1.0f / (1.0f + 0.14f * dist + 0.07f * (dist * dist));
  diffuse  *= attenuation;
  specular *= attenuation;

  // shadows
  float costheta = clamp(dot(normals, light_dir), 0.0, 1.0);
  float bias     = 0.2*tan(acos(costheta));
  bias           = clamp(bias, 0.1, 0.2);
  float shadow = 0.0f;
  if (l.is_shadow) {
    vec3 frag_to_light  = mat3(u_inverse_view) * (fragpos - l.position);
    float current_depth = length(frag_to_light);
    float view_dist     = length(-fragpos);

    // PCF smoothing
    float radius = (1.0 + (view_dist / l.far)) / l.far;
    float offset = 0.1;
    int   samples = 20;
    float closest_depth = 0.0f;
    for (int i=0; i<samples; ++i) {
      closest_depth  = texture(u_point_depth, frag_to_light + pcf_offset[i] * radius).r;
      closest_depth *= l.far;
      if (current_depth - bias > closest_depth)
        shadow += 1.0;
    }
    shadow /= float(samples);
  }

  return vec3((1.0 - shadow) * (diffuse + specular));
}

// ---- procedural noise for the dynamic ground (value noise + fBm, cheap + seamless) ----
float ex_hash(vec2 p){ p = fract(p*vec2(123.34, 456.21)); p += dot(p, p+45.32); return fract(p.x*p.y); }
float ex_vnoise(vec2 p){
  vec2 i = floor(p), f = fract(p); f = f*f*(3.0-2.0*f);
  float a = ex_hash(i), b = ex_hash(i+vec2(1,0)), c = ex_hash(i+vec2(0,1)), d = ex_hash(i+vec2(1,1));
  return mix(mix(a,b,f.x), mix(c,d,f.x), f.y);
}
float ex_fbm2(vec2 p){ float s=0.0, a=0.5; for(int i=0;i<4;i++){ s+=a*ex_vnoise(p); p*=2.0; a*=0.5; } return s; }
float ex_fbm2c(vec2 p){ return ex_vnoise(p)*0.65 + ex_vnoise(p*2.3)*0.35; }   // 2-tap: cheap cloud octaves
// cellular / Voronoi: nearest-feature distance + a per-cell random id, for cracked plates
vec2 ex_cell(vec2 p){
  vec2 ip = floor(p), fp = fract(p);
  float f1 = 8.0; vec2 cid = vec2(0.0);
  for (int j=-1;j<=1;j++){
    for (int i=-1;i<=1;i++){
      vec2 g = vec2(float(i), float(j));
      vec2 o = vec2(ex_hash(ip+g), ex_hash(ip+g+17.1));
      vec2 r = g + o - fp;
      float d = dot(r,r);
      if (d < f1){ f1 = d; cid = ip+g; }
    }
  }
  return vec2(sqrt(f1), ex_hash(cid));
}

void main()
{
  // guard: an unset tint uniform reads as 0 -> treat near-zero as white so a
  // model never renders black just because tint wasn't provided.
  vec3 tint = (dot(u_tint, u_tint) < 0.001) ? vec3(1.0) : u_tint;
  // planetary sun state, hoisted to function scope so the lit block, the procedural
  // ground, AND the atmosphere (god rays / height fog) can all read it.
  float sunS = (u_sun_strength <= 0.0001) ? 0.62 : u_sun_strength;
  vec3  sunCg = (dot(u_sun_color,u_sun_color) < 0.001) ? vec3(1.0) : u_sun_color;
  vec3  sdirg = (dot(u_sun_dir,u_sun_dir) < 0.001) ? vec3(0.35, 0.9, 0.35) : u_sun_dir;

  // Emissive/unlit path: effects glow in their PURE tint colour (no texture
  // sample -- the muddy textures were washing bolts toward grey/white).
  if (!u_is_lit) {
    // emissive/unlit. Skyboxes want their TEXTURE (their tint is white); bolts
    // and effects want PURE tint colour (their texture is plain white). We tell
    // them apart with u_emissive_solid: solid effects ignore the texture.
    vec4 base = u_emissive_solid ? vec4(tint * 0.85, 1.0)
                                 : texture(u_texture, uv) * vec4(tint, 1.0);
    color = u_ambient_pass ? base : vec4(0.0, 0.0, 0.0, 1.0);
    return;
  }

  vec3 diffuse = vec3(0.0f);

  if (u_ambient_pass) {
    // low, moody ambient + a DIRECTIONAL sun so characters and geometry are
    // shaped consistently everywhere (point lights attenuate to nothing far from
    // the arena centre, which left distant characters lit only by the specular
    // floor -> washed white). The sun has no attenuation, so form reads anywhere.
    vec3 base_diff = texture(u_texture, uv).rgb;
    vec3 nrm_ts = texture(u_norm, uv).rgb * 2.0 - 1.0;
    nrm_ts.xy *= u_mat_boost;                     // amp bump strength (more relief on tires/face)
    vec3 Nvs = normalize(TBN * normalize(nrm_ts));
    // DISTANCE NORMAL FADE: far surfaces relax toward the smooth vertex normal so
    // normal-map detail can't SHIMMER at distance -- a clean, stable far image.
    {
      float nf = clamp((length(frag) - 45.0) / 130.0, 0.0, 0.85);
      Nvs = normalize(mix(Nvs, normalize(TBN * vec3(0.0, 0.0, 1.0)), nf));
    }
    vec3 sun_vs = normalize(mat3(u_view) * normalize(sdirg));
    float ndl = max(dot(Nvs, sun_vs), 0.0);
    // SUN SHADOW (high+): project this fragment into the sun's depth map; 3x3 PCF with
    // a slope-scaled bias. Everything the sun contributes -- direct, glint, in stack --
    // is multiplied by the result: REAL cast shadows from the planetary star.
    float sunSh = 1.0;
    if (u_sun_shadow_on == 1) {
      vec3 wps = (u_inverse_view * vec4(frag, 1.0)).xyz;
      vec4 lp = u_sun_light_mat * vec4(wps, 1.0);
      vec3 lpn = lp.xyz / lp.w * 0.5 + 0.5;
      if (lpn.z <= 1.0) {
        float bias = max(0.0022 * (1.0 - ndl), 0.0007);
        float acc = 0.0;
        vec2 stex = 1.0 / vec2(textureSize(u_sun_shadow, 0));
        for (int sy = -1; sy <= 1; sy++)
          for (int sx = -1; sx <= 1; sx++) {
            float d2 = texture(u_sun_shadow, lpn.xy + vec2(sx, sy) * stex).r;
            acc += (lpn.z - bias > d2) ? 0.0 : 1.0;
          }
        sunSh = mix(0.28, 1.0, acc / 9.0);   /* shadows stay readable, never pure black */
      }
    }
    // CLOUD SHADOWS (balanced+): broad soft light-patches drift across the world --
    // the sun through a moving cloud deck. World-space fbm, slow diagonal drift,
    // never darker than 0.55 so it reads as weather, not failure. Off indoors/space.
    if (u_cloud_amt > 0.001 && u_gfx_quality >= 0.5) {
      vec3 wpc = (u_inverse_view * vec4(frag, 1.0)).xyz;
      float cl = ex_fbm2c(wpc.xz * 0.0045 + vec2(u_time * 0.016, u_time * 0.011));
      float cs2 = mix(1.0, 0.55 + 0.45 * smoothstep(0.35, 0.65, cl), u_cloud_amt);
      sunSh *= cs2;
    }
    vec3  sunC = sunCg;
    // ambient floor: flat on mobile; on balanced+ a HEMISPHERE fill (cool sky from
    // above graduating to a warm-dark ground bounce below) for far more form.
    vec3 up_vs = normalize(mat3(u_view) * vec3(0.0, 1.0, 0.0));
    float hemi_t = dot(Nvs, up_vs) * 0.5 + 0.5;
    vec3 hemi = mix(vec3(0.08, 0.07, 0.06), vec3(0.16, 0.19, 0.24), hemi_t);
    vec3 amb_floor = mix(vec3(0.12), hemi, step(0.5, u_gfx_quality));
    amb_floor *= 0.55 + 0.45 * clamp(sunS / 0.62, 0.0, 1.0);   // the sky's light dies with the sun
    diffuse += base_diff * (amb_floor + sunS * ndl * sunC * sunSh);   // the PLANETARY sun shapes the world (shadowed)
    // ENVIRONMENT REFLECTION (fake IBL, balanced+): the reflection vector samples the same
    // sky/ground hemisphere, weighted by FRESNEL -- armour catches the sky at grazing
    // angles and a warm bounce underneath, scaled by daylight so night stays matte-dark.
    if (u_gfx_quality >= 0.5) {
      vec3 vdir = normalize(-frag);
      vec3 R = reflect(-vdir, Nvs);
      float rt = dot(R, up_vs) * 0.5 + 0.5;
      vec3 env = mix(vec3(0.10, 0.08, 0.06), mix(vec3(0.18,0.22,0.30), sunCg, 0.35), rt);
      float fresE = pow(1.0 - max(dot(vdir, Nvs), 0.0), 3.0);
      diffuse += env * fresE * (0.30 + 0.35*clamp(sunS/0.62, 0.0, 1.0)) * (1.0 + u_wetness*0.9) * u_mat_boost;
      // sun GLINT: a tight specular from the planetary sun itself (the key reflection)
      vec3 hgl = normalize(sun_vs + vdir);
      diffuse += sunC * pow(max(dot(Nvs, hgl), 0.0), 64.0) * sunS * (0.8 + u_wetness*0.7) * sunSh * sqrt(u_mat_boost);
      // BACK-LIGHT rim (the film kicker): a cool edge light from opposite the sun lifts
      // silhouettes off dark backgrounds -- the third point of three-point lighting.
      float rimK = pow(1.0 - max(dot(vdir, Nvs), 0.0), 2.5) * max(dot(Nvs, -sun_vs), 0.0);
      diffuse += vec3(0.55, 0.72, 1.0) * rimK * sunS * 0.55;
    }
    diffuse += texture(u_emission, uv).rgb * 2.2 * u_emission_strength;  // texture-based emission
  } else {
    if (u_point_active && u_point_count <= 0)
      diffuse += calc_point_light(u_point_light);
  }

  if (u_point_count > 0)
    for (int i=0; i<u_point_count; i++)
      diffuse += calc_point_light(u_point_lights[i]);

  // subtle element tint, then a filmic (ACES-approx) tonemap so bright lights
  // roll off into a moody range instead of blowing out flat.
  vec3 c = diffuse * mix(vec3(1.0), tint, 0.5);
  // SILHOUETTE RIM: on a dark, low-contrast war palette bots otherwise vanish
  // into the murk. This is a FAINT, cold reflected-void sheen catching an edge --
  // just enough to separate the silhouette, never a bright neon outline that
  // would fight the grim tone. Kept dim + desaturated toward deep steel-blue.
  vec3 rim_n = normalize(normal);        // view-space surface normal (in scope here)
  vec3 rim_v = normalize(-frag);         // view-space view direction
  float rim = pow(1.0 - max(dot(rim_n, rim_v), 0.0), 4.0);
  c += rim * (0.085 * mix(vec3(0.32,0.38,0.52), tint, 0.35));
  c = (c * (2.51*c + 0.03)) / (c * (2.43*c + 0.59) + 0.14);
  // ---- PROCEDURAL DYNAMIC GROUND: a full seamless material on up-facing ground --
  //      cracked energy-PLATES (Voronoi cells with glowing pulsing seams), flowing energon
  //      VEINS threading across them, fine mottling, and a slow breathing sheen. All in
  //      world-space (via inverse-view) so it's connected map-wide with no UV seams, all
  //      animated off u_time, and biased toward the map's element tint. Mobile skips it. ----
  if (u_gfx_quality >= 1.0) {
    vec3 wpos  = (u_inverse_view * vec4(frag, 1.0)).xyz;
    vec3 wnorm = normalize(mat3(u_inverse_view) * normal);
    if (wnorm.y > 0.6) {
      vec2  gp   = wpos.xz;
      float flow = u_time * 0.05;
      vec2  warp = vec2(ex_fbm2(gp*0.03 + flow), ex_fbm2(gp*0.03 - flow + 5.2));   // domain warp
      vec3  vcol = mix(vec3(0.20, 0.70, 1.0), tint, 0.5);                          // energon, element-biased
      // 1) cracked energy PLATES: warped Voronoi cells are plates, the seams glow + pulse
      vec2  cell = ex_cell(gp*0.09 + warp*0.5);
      float seam = smoothstep(0.07, 0.0, cell.x);
      float plate = 0.88 + 0.22*ex_hash(vec2(cell.y, 1.7));                        // plate-to-plate shade
      float seamPulse = 0.5 + 0.5*sin(u_time*1.5 + cell.y*30.0);                   // each plate breathes its own seam
      // 2) flowing VEINS threading across the plates
      float v     = ex_fbm2(gp*0.06 + warp*1.5);
      float veins = smoothstep(0.045, 0.0, abs(v - 0.5));
      float vpulse= 0.5 + 0.5*sin(u_time*2.0 - gp.x*0.05 - gp.y*0.05 + v*12.0);
      // 3) fine mottling + a slow breathing sheen
      float grain = ex_fbm2(gp*0.25)*0.12;
      float breath= 0.5 + 0.5*sin(u_time*0.7 + ex_fbm2(gp*0.02)*6.2831);
      c *= plate * (0.94 + grain);
      // NIGHT HUNT-LIGHT: as the planetary sun dies, the energon burns BRIGHTER --
      // at night the seams + veins become the light you navigate by.
      float nightGlow = 1.0 + 0.9 * (1.0 - clamp(sunS / 0.62, 0.0, 1.0));
      c += vcol * seam  * (0.22 + 0.40*seamPulse) * nightGlow;
      c += vcol * veins * (0.28 + 0.50*vpulse)   * nightGlow;
      c += vcol * 0.02  * breath * nightGlow;
    }
  }
  // ---- ATMOSPHERE: distance fog + HEIGHT FOG (ground mist) + GOD RAYS (sun in-scatter).
  float fdist = length(frag);
  float hfog = 0.0;
  if (u_gfx_quality >= 0.5) {
    // ground mist: fog thickens toward low world height, breathing slowly -- valleys pool.
    vec3 wpf = (u_inverse_view * vec4(frag, 1.0)).xyz;
    float mistband = exp(-max(wpf.y, 0.0) * 0.030);
    hfog = mistband * (0.85 + 0.15 * sin(u_time * 0.23 + wpf.x * 0.01)) * 1.1;
  }
  float fog = 1.0 - exp(-u_fog_density * fdist * (1.0 + hfog));
  // god rays: looking TOWARD the sun, the fog itself catches light -- a broad in-scatter
  // halo plus a tighter core, strongest at dawn/dusk when the sun sits low (long shafts).
  vec3 fogC = u_fog_color;
  {
    vec3 vd = normalize(frag);
    vec3 sun_vs3 = normalize(mat3(u_view) * normalize(sdirg));
    float toSun = max(dot(vd, sun_vs3), 0.0);
    float lowSun = 1.0 - clamp(sdirg.y * 2.2, 0.0, 1.0);          // shafts live at the horizons
    float shaft = pow(toSun, 24.0) * 1.6 + pow(toSun, 6.0) * 0.35;
    fogC += sunCg * sunS * shaft * (0.8 + 1.6 * lowSun);
  }
  c = mix(c, fogC, clamp(fog, 0.0, 1.0));
  // atmospheric haze for SCALE: distant geometry desaturates and cools slightly,
  // so a large map reads as VAST distance rather than a blurred miniature. Ramps
  // in from ~250u out to ~1350u and tops out gently, so nearby detail is untouched.
  float hz = clamp((length(frag) - 250.0) / 1100.0, 0.0, 1.0);
  hz = hz * hz * 0.40;
  float hlum = dot(c, vec3(0.299, 0.587, 0.114));
  vec3 haze = mix(vec3(hlum * 0.6), vec3(0.10, 0.13, 0.19), 0.5);   // darker + cool, never white
  c = mix(c, haze, hz);
  color = vec4(c, 1.0);
}
#END FS