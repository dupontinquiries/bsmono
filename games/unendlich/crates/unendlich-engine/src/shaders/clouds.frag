#version 450
//
// clouds.frag — procedural volumetric-ish cloud layer for the sky dome.
// r336-weather-bonus (paired with `world::weather::GlobalWeather` +
// `world::cloud_coverage::coverage`).
//
// Drawn as a fullscreen triangle (present.vert / sky.vert style) AFTER sky.frag and
// BEFORE opaque geometry, blended over the sky by alpha. Reconstructs a world-space
// view ray from NDC and the push-constant inverse view-proj, intersects it with a
// flat cloud plane at `cloud_height`, and evaluates animated FBM value-noise there.
// The CPU feeds `coverage` from `cloud_coverage::coverage(humidity, temp, altitude)`
// and the current `GlobalWeather` intensity; the shader maps coverage → cloud
// amount/opacity so overcast/storm skies fill in and clear skies stay empty.
//
// NO new descriptor set/binding: all inputs ride a single push-constant block
// (<= 128 bytes). Pure procedural noise — no texture samplers — so it validates and
// runs without extra resources.

layout(location = 0) in  vec2 v_uv;       // 0..1 fullscreen UV
layout(location = 0) out vec4 out_color;  // premultiplied cloud color + coverage alpha

layout(push_constant) uniform Pc {
    mat4  inv_view_proj; // NDC -> world (to build the view ray)            64B
    vec4  cam_pos;       // xyz = camera world pos                          16B
    vec4  sun_dir;       // xyz = normalized direction TO the sun           16B
    vec4  params;        // x=coverage[0,1] y=time(s) z=wind(cells/s) w=cloud_height  16B
} pc;                    // total 112B

// --- hash-based value noise (deterministic, GPU-cheap) ----------------------
float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 345.45));
    p += dot(p, p + 34.345);
    return fract(p.x * p.y);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f); // smoothstep interpolation
    float a = hash21(i + vec2(0.0, 0.0));
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 5; ++i) {
        sum += amp * vnoise(p);
        p = p * 2.02 + vec2(7.1, 3.7);
        amp *= 0.5;
    }
    return sum;
}

void main() {
    float coverage = clamp(pc.params.x, 0.0, 1.0);
    float time     = pc.params.y;
    float wind     = pc.params.z;
    float height   = pc.params.w;

    // Build a world-space view ray from this pixel's NDC.
    vec2 ndc = v_uv * 2.0 - 1.0;
    vec4 near = pc.inv_view_proj * vec4(ndc, 0.0, 1.0);
    vec4 far  = pc.inv_view_proj * vec4(ndc, 1.0, 1.0);
    vec3 ro = near.xyz / near.w;
    vec3 rd = normalize(far.xyz / far.w - ro);

    // Clouds only on the upper hemisphere; fade out toward/below the horizon.
    float up = rd.y;
    if (up <= 0.02) {
        out_color = vec4(0.0);
        return;
    }

    // Intersect the ray with the flat cloud plane y = cam_pos.y + height.
    float plane_y = pc.cam_pos.y + max(height, 1.0);
    float t = (plane_y - ro.y) / max(rd.y, 1e-3);
    vec2 world = ro.xz + rd.xz * t;

    // Animated FBM at the hit point; wind scrolls the field over time.
    vec2 uv = world * 0.0009 + vec2(time * wind * 0.01, time * wind * 0.006);
    float n = fbm(uv);

    // Coverage raises the cloud field and the density floor: clear skies (low
    // coverage) only show the tallest puffs; overcast fills in.
    float thresh = mix(0.78, 0.12, coverage);
    float density = clamp((n - thresh) / max(1.0 - thresh, 1e-3), 0.0, 1.0);

    // Horizon fade (thinner clouds toward the horizon) + a soft edge.
    float horizon = smoothstep(0.02, 0.30, up);
    float alpha = density * horizon * mix(0.55, 1.0, coverage);

    // Lighting: brighten toward the sun, shade the underside.
    float sun = clamp(dot(rd, normalize(pc.sun_dir.xyz)) * 0.5 + 0.5, 0.0, 1.0);
    vec3 lit = mix(vec3(0.55, 0.58, 0.65), vec3(1.0, 0.98, 0.94), sun * density);
    // storms (high coverage) darken the clouds.
    lit *= mix(1.0, 0.7, smoothstep(0.6, 1.0, coverage));

    out_color = vec4(lit * alpha, alpha); // premultiplied for over-blend on the sky
}
