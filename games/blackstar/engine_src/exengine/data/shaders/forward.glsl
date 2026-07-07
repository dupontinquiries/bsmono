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

uniform sampler2D u_texture;
uniform sampler2D u_spec;
uniform sampler2D u_norm;

uniform mat4 u_projection;
uniform mat4 u_view;
uniform mat4 u_inverse_view;

uniform bool u_ambient_pass;
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

void main()
{
  // guard: an unset tint uniform reads as 0 -> treat near-zero as white so a
  // model never renders black just because tint wasn't provided.
  vec3 tint = (dot(u_tint, u_tint) < 0.001) ? vec3(1.0) : u_tint;

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
    vec3 Nvs = normalize(TBN * normalize(texture(u_norm, uv).rgb * 2.0 - 1.0));
    vec3 sun_vs = normalize(mat3(u_view) * normalize(vec3(0.35, 0.9, 0.35)));
    float ndl = max(dot(Nvs, sun_vs), 0.0);
    diffuse += base_diff * (0.12 + 0.62 * ndl);   // ambient floor + directional shaping
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
  // distance fog: frag is in view space, so its length is the camera distance.
  float fog = 1.0 - exp(-u_fog_density * length(frag));
  c = mix(c, u_fog_color, clamp(fog, 0.0, 1.0));
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