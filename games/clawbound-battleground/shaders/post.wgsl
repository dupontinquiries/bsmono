// Post-processing: single-pass bloom + lightning flash + ACES tonemap +
// saturation + vignette + film grain + rain streaks.

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct PostParams {
    a: vec4<f32>, // exposure, vignette, time, lightning
    b: vec4<f32>, // bloom strength, night, rain, heat
    c: vec4<f32>, // sun screen x, sun screen y (uv), ray strength, contrast
    d: vec4<f32>, // x wetness, y ultra reflections (0/1), z ultra god rays (0/1)
    vp: mat4x4<f32>,
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,                 // xyz: the camera
    blast_a: array<vec4<f32>, 8>,   // blast fronts in the world: centre xyz, radius
    blast_b: array<vec4<f32>, 8>,   // strength, shell thickness
};
@group(0) @binding(2) var<uniform> pp: PostParams;
// The scene's depth (MSAA; sample 0 is plenty for a refraction mask).
@group(0) @binding(3) var scene_depth: texture_depth_multisampled_2d;

// How far along the ray one sphere lies, clipped to [0, t_max]
// (groundfx::sphere_span).
fn sphere_span(ro: vec3<f32>, rd: vec3<f32>, c: vec3<f32>, r: f32, t_max: f32) -> f32 {
    let oc = ro - c;
    let b = dot(oc, rd);
    let disc = b * b - (dot(oc, oc) - r * r);
    if (disc <= 0.0) {
        return 0.0;
    }
    let h = sqrt(disc);
    return max(min(-b + h, t_max) - max(-b - h, 0.0), 0.0);
}

// Blast-front air crossed by the ray before the scene surface
// (groundfx::shell_path, which the tests check).
fn shell_path(ro: vec3<f32>, rd: vec3<f32>, c: vec3<f32>, radius: f32, th: f32, t_scene: f32) -> f32 {
    let outer = sphere_span(ro, rd, c, radius + th * 0.5, t_scene);
    let inner = sphere_span(ro, rd, c, max(radius - th * 0.5, 0.0), t_scene);
    return max(outer - inner, 0.0);
}

// The world point behind a pixel, from the depth buffer.
fn world_at(uv: vec2<f32>, dims: vec2<f32>) -> vec4<f32> {
    let px = vec2<i32>(clamp(uv * dims, vec2(0.0), dims - vec2(1.0)));
    let d = textureLoad(scene_depth, px, 0);
    let w = pp.inv_vp * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    return vec4(w.xyz / w.w, d);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash21(i), hash21(i + vec2(1.0, 0.0)), u.x), mix(hash21(i + vec2(0.0, 1.0)), hash21(i + vec2(1.0, 1.0)), u.x), u.y);
}

// Wet ground as a mirror (ultra): screen-space reflection of a level wet
// surface. The pixel's world point and normal come from the depth buffer;
// the reflected ray is marched through it and what it meets is mirrored,
// by Fresnel (glancing views reflect most), in puddle-like patches that
// grow with the wetness. Returns the reflected colour and its weight.
fn wet_reflection(uv: vec2<f32>, dims: vec2<f32>) -> vec4<f32> {
    let wet = pp.d.x;
    let here = world_at(uv, dims);
    if (here.w >= 0.99999) {
        return vec4(0.0);
    }
    let P = here.xyz;
    let a = world_at(uv + vec2(1.0 / dims.x, 0.0), dims).xyz;
    let b = world_at(uv + vec2(0.0, 1.0 / dims.y), dims).xyz;
    var N = normalize(cross(b - P, a - P));
    if (N.y < 0.0) {
        N = -N;
    }
    if (N.y < 0.85) {
        return vec4(0.0); // only level ground holds water
    }
    let V = normalize(P - pp.cam.xyz);
    let R = reflect(V, N);
    let dist0 = length(P - pp.cam.xyz);
    var t = 0.25;
    var hit = vec3(0.0);
    var found = 0.0;
    for (var i = 0; i < 24; i = i + 1) {
        let q = P + R * t;
        let c = pp.vp * vec4(q, 1.0);
        if (c.w <= 0.05) {
            break;
        }
        let quv = vec2(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
        if (any(quv < vec2(0.0)) || any(quv > vec2(1.0))) {
            break;
        }
        let s = world_at(quv, dims);
        let dq = length(q - pp.cam.xyz);
        let ds = select(length(s.xyz - pp.cam.xyz), 1.0e6, s.w >= 0.99999);
        if (dq > ds && dq - ds < 0.6 + t * 0.15) {
            // explicit level: this runs after a data-dependent break
            // (non-uniform), where implicit-derivative sampling is not safe
            hit = textureSampleLevel(hdr, samp, quv, 0.0).rgb;
            // fade near the screen's edge, where the march runs out
            let e = min(min(quv.x, 1.0 - quv.x), min(quv.y, 1.0 - quv.y));
            found = smoothstep(0.0, 0.08, e);
            break;
        }
        t = t * 1.22 + 0.12;
    }
    let fres = 0.02 + 0.98 * pow(1.0 - max(dot(-V, N), 0.0), 5.0);
    let puddle = smoothstep(0.45 - wet * 0.35, 0.7 - wet * 0.3, vnoise(P.xz * 0.35) * 0.7 + vnoise(P.xz * 1.3) * 0.3);
    let far_fade = 1.0 - smoothstep(40.0, 90.0, dist0);
    return vec4(hit, found * fres * puddle * wet * far_fade * 0.95);
}

// The world-space direction of the view ray through `uv`.
fn function_ray(uv: vec2<f32>, ro: vec3<f32>) -> vec3<f32> {
    let w = pp.inv_vp * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    return normalize(w.xyz / w.w - ro);
}


struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    var p = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0)
    );
    var out: VSOut;
    out.clip = vec4(p[vi], 0.0, 1.0);
    out.uv = (p[vi] + vec2(1.0)) * 0.5;
    out.uv.y = 1.0 - out.uv.y;
    return out;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3(0.0), vec3(1.0));
}

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn bright(c: vec3<f32>) -> vec3<f32> {
    // soft knee around 1.0 in linear HDR — crystals, VFX, the sun disc, wet glints
    let l = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return c * smoothstep(0.85, 1.6, l);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let params = pp.a;
    // --- the air moving. A blast front is a SHELL of denser air in the
    // world: each pixel's view ray is tested against it, and only the air
    // the ray crosses BEFORE it hits the scene bends the view -- so what
    // stands in front of a blast is not warped, a ground blast is a dome
    // the ground cuts, and the shell is strongest at its silhouette, where
    // the ray grazes the most air. The bend is outward along the shell's
    // normal as the screen sees it. (It was a flat screen-space ring, bent
    // over everything; groundfx::shell_path is the same maths, tested.)
    // Heat shimmer does its own thing gently and everywhere. Both are one
    // UV offset before anything is sampled, so bloom and shafts bend too.
    var bend = vec2(0.0);
    let dims = vec2<f32>(textureDimensions(hdr));
    let aspect = dims.x / dims.y;
    var any_blast = false;
    for (var i = 0; i < 8; i = i + 1) {
        if (pp.blast_b[i].x > 0.002) {
            any_blast = true;
        }
    }
    if (any_blast) {
        let px = vec2<i32>(clamp(in.uv * dims, vec2(0.0), dims - vec2(1.0)));
        let d = textureLoad(scene_depth, px, 0);
        let ndc = vec4(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0, d, 1.0);
        let wp = pp.inv_vp * ndc;
        let world = wp.xyz / wp.w;
        let ro = pp.cam.xyz;
        let rd = normalize(world - ro);
        // the sky (depth at the far plane) is infinitely far
        let t_scene = select(length(world - ro), 1.0e6, d >= 0.99999);
        // neighbouring pixels' rays, for the gradient of the air crossed
        let ray_at = function_ray(in.uv + vec2(1.0 / dims.x, 0.0), ro);
        let ray_up = function_ray(in.uv + vec2(0.0, 1.0 / dims.y), ro);
        for (var i = 0; i < 8; i = i + 1) {
            let strength = pp.blast_b[i].x;
            if (strength <= 0.002) {
                continue;
            }
            let c = pp.blast_a[i].xyz;
            let radius = pp.blast_a[i].w;
            let th = pp.blast_b[i].y;
            let path = shell_path(ro, rd, c, radius, th, t_scene);
            let px_path = shell_path(ro, ray_at, c, radius, th, t_scene);
            let py_path = shell_path(ro, ray_up, c, radius, th, t_scene);
            if (path <= 0.0 && px_path <= 0.0 && py_path <= 0.0) {
                continue;
            }
            // Light bends where the THICKNESS of air changes across the view
            // -- steep at the shell's silhouette, near zero straight through
            // its middle (front and back walls cancel). The first version
            // bent along the shell's normal wherever it was crossed, and the
            // whole inside of the dome slid sideways. Bend along the
            // gradient, in pixels of path per pixel, capped.
            let g = vec2(px_path - path, py_path - path) / max(th, 0.05);
            let m = length(g);
            if (m > 1e-5) {
                let amt = min(m, 0.35) * strength * 0.06;
                bend = bend - (g / m) * vec2(1.0 / aspect, 1.0) * amt;
            }
        }
    }
    let heat = pp.b.w;
    if (heat > 0.002) {
        let t = pp.a.z;
        let wob = sin(in.uv.y * 95.0 - t * 6.5 + sin(in.uv.x * 40.0 + t * 2.0) * 1.7);
        bend = bend + vec2(wob * 0.0022, 0.0) * heat * smoothstep(0.0, 0.5, 1.0 - in.uv.y + 0.2);
    }
    let uv0 = clamp(in.uv + bend, vec2(0.001), vec2(0.999));
    var col = textureSample(hdr, samp, uv0).rgb;
    if (pp.d.y > 0.5 && pp.d.x > 0.02) {
        let r = wet_reflection(uv0, dims);
        col = mix(col, r.rgb, r.a);
    }

    // --- single-pass bloom: a 12-tap ring at two radii, weighted to a soft disc.
    // Cheap enough for a phone GPU; the MSAA resolve already softened the source.
    let texel = vec2(1.0) / vec2<f32>(textureDimensions(hdr));
    // LOW GRAPHICS SKIPS THE WHOLE RING. 12 iterations x 2 taps = 24 texture
    // samples per pixel, and at 1080p that is ~50M samples a frame before the
    // god rays add 22 more. Scaling the strength toward zero still pays for
    // every one of them; branching out does not.
    //
    // Zero bloom strength is the signal rather than a new uniform slot: `b`
    // is full, and "no bloom" and "bloom at zero intensity" are the same
    // picture -- so the low setting sets the strength it already has to 0 and
    // this reads it.
    var glow = vec3(0.0);
    if (pp.b.x > 0.001) {
    for (var i: i32 = 0; i < 12; i = i + 1) {
        let a = f32(i) * 0.5236;
        let dir = vec2(cos(a), sin(a));
        glow = glow + bright(textureSample(hdr, samp, uv0 + dir * texel * 3.0).rgb) * 0.6;
        glow = glow + bright(textureSample(hdr, samp, uv0 + dir * texel * 7.5).rgb) * 0.4;
    }
    glow = glow / 12.0;
    }
    col = col + glow * pp.b.x;

    // --- light shafts: radial samples of the bright pass toward the sun's
    // screen position; strongest when the sun is low and in frame
    let ray_s = pp.c.z;
    if (ray_s > 0.001) {
        let sun_uv = pp.c.xy;
        var acc = vec3(0.0);
        let step = (sun_uv - in.uv) / 22.0;
        var uvr = in.uv;
        var wgt = 1.0;
        let dither = hash21(in.uv * 640.0 + fract(pp.a.z) * 37.0) * 0.8;
        uvr = uvr + step * dither;
        let god = pp.d.z > 0.5;
        for (var i: i32 = 0; i < 22; i = i + 1) {
            uvr = uvr + step;
            let cuv = clamp(uvr, vec2(0.0), vec2(1.0));
            let s = textureSample(hdr, samp, cuv).rgb;
            if (god) {
                // Ultra: only open SKY lets sunlight through (depth at the
                // far plane); leaves, trunks and peaks block it -- rays
                // stream through the gaps. The bright-pixel version sprayed
                // rays off anything white: a cat, a bolt, a gem.
                let px = vec2<i32>(clamp(cuv * dims, vec2(0.0), dims - vec2(1.0)));
                let open = select(0.0, 1.0, textureLoad(scene_depth, px, 0) >= 0.99999);
                acc = acc + min(s, vec3(4.0)) * open * wgt * 0.4;
            } else {
                acc = acc + bright(s) * wgt;
            }
            wgt = wgt * 0.93;
        }
        let fade = smoothstep(1.4, 0.2, distance(in.uv, sun_uv));
        col = col + acc / 22.0 * ray_s * fade * vec3(1.0, 0.9, 0.75);
    }

    // --- lightning: a full-frame flash on top of the lit scene
    col = col + vec3(0.55, 0.58, 0.8) * params.w * 0.9;

    // exposure + ACES, then a gentle S-curve for punch (contrast in pp.c.w)
    col = col * params.x;
    col = aces(col);
    let k = pp.c.w;
    col = mix(col, col * col * (3.0 - 2.0 * col), k);

    // subtle saturation lift (the Long Night desaturates toward blue-steel)
    let luma = dot(col, vec3(0.2126, 0.7152, 0.0722));
    col = mix(vec3(luma), col, mix(1.24, 0.92, pp.b.y));
    col = mix(col, col * vec3(0.85, 0.9, 1.15), pp.b.y * 0.5);

    // --- rain streaks on the lens: thin falling lines, stronger at the edges
    let rain = pp.b.z;
    if (rain > 0.01) {
        let su = in.uv * vec2(140.0, 6.0) + vec2(0.0, params.z * 9.0);
        let cell = floor(su);
        let hh = hash21(cell);
        let fx = fract(su.x);
        let streak = smoothstep(0.08, 0.0, abs(fx - 0.5)) * step(0.88, hh) * smoothstep(0.3, 0.7, distance(in.uv, vec2(0.5)) + 0.3);
        col = col + vec3(0.12, 0.13, 0.16) * streak * rain;
    }

    // vignette
    let v = distance(in.uv, vec2(0.5, 0.5));
    col = col * (1.0 - smoothstep(0.55, 0.95, v) * params.y);

    // film grain
    let g = hash21(in.uv * 800.0 + fract(params.z) * 91.7) - 0.5;
    col = col + vec3(g * 0.028);

    return vec4(col, 1.0);
}
