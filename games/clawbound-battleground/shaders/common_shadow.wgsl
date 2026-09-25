// --- shared: the sun's shadow (prepended to voxel, terrain and smooth by
// render.rs; `shadow_map` and `shadow_sampler` are declared by each).

// Fade the shadow out toward the edge of the shadow map's box.
fn shadow_edge(uv: vec2<f32>) -> f32 {
    let m = max(abs(uv.x - 0.5), abs(uv.y - 0.5)) * 2.0;
    return smoothstep(0.88, 0.99, m);
}

fn shadow_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(233.34, 851.73));
    q = q + dot(q, q + 23.45);
    return fract(q.x * q.y);
}

// Contact-hardening sun shadow. A blocker search finds how far above this
// surface the thing casting the shadow is; the filter's width grows with
// that gap -- crisp and detailed where a shadow meets what casts it (a
// cat's feet, a trunk's base), softening with distance (a canopy's shadow
// on the ground far below). The fixed 3x3 blur it replaces was the same
// width everywhere. The light's orthographic depth spans 319 units
// (groundfx::fit_shadow: near 1, far 320).
fn sun_shadow(shadow_pos: vec4<f32>, bias: f32, frag: vec2<f32>) -> f32 {
    let sp = shadow_pos.xyz / shadow_pos.w;
    let uv = sp.xy * vec2(0.5, -0.5) + vec2(0.5);
    if (!(uv.x > 0.002 && uv.x < 0.998 && uv.y > 0.002 && uv.y < 0.998 && sp.z <= 1.0)) {
        return 1.0;
    }
    let dims = vec2<f32>(textureDimensions(shadow_map));
    let texel = 1.0 / dims.x;
    // blocker search: the mean depth of what occludes, in a small window
    var found = 0.0;
    var mean = 0.0;
    for (var i: i32 = -2; i <= 2; i = i + 2) {
        for (var j: i32 = -2; j <= 2; j = j + 2) {
            let c = vec2<i32>(clamp((uv + vec2(f32(i), f32(j)) * texel * 2.0) * dims, vec2(0.0), dims - vec2(1.0)));
            let d = textureLoad(shadow_map, c, 0);
            if (d < sp.z - bias) {
                mean = mean + d;
                found = found + 1.0;
            }
        }
    }
    if (found < 0.5) {
        return 1.0;
    }
    mean = mean / found;
    // penumbra, in texels: ~1.3 per world unit of gap, from half a texel at
    // contact to six far away
    let gap = (sp.z - mean) * 319.0;
    let pen = clamp(gap * 1.3, 0.5, 6.0) * texel;
    // 12 rotated Poisson taps
    let ang = shadow_hash(frag) * 6.2831853;
    let rot = mat2x2<f32>(cos(ang), sin(ang), -sin(ang), cos(ang));
    var taps = array<vec2<f32>, 12>(
        vec2(-0.326, -0.406), vec2(-0.840, -0.074), vec2(-0.696, 0.457), vec2(-0.203, 0.621),
        vec2(0.962, -0.195), vec2(0.473, -0.480), vec2(0.519, 0.767), vec2(0.185, -0.893),
        vec2(0.507, 0.064), vec2(0.896, 0.412), vec2(-0.322, -0.933), vec2(-0.792, -0.598)
    );
    var s = 0.0;
    for (var k = 0; k < 12; k = k + 1) {
        s = s + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + rot * taps[k] * pen, sp.z - bias);
    }
    return mix(s / 12.0, 1.0, shadow_edge(uv));
}
