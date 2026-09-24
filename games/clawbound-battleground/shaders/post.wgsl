// Post-processing: single-pass bloom + lightning flash + ACES tonemap +
// saturation + vignette + film grain + rain streaks.

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct PostParams {
    a: vec4<f32>, // exposure, vignette, time, lightning
    b: vec4<f32>, // bloom strength, night, rain, heat
    c: vec4<f32>, // sun screen x, sun screen y (uv), ray strength, contrast
    rings: array<vec4<f32>, 8>, // blast fronts on screen: u, v, radius (v-units), strength
};
@group(0) @binding(2) var<uniform> pp: PostParams;

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
    // --- the air moving. A blast front is denser air: it bends what is seen
    // through it, outward, in a thin band at the ring. Heat does the same
    // thing gently and everywhere at once, rising. Both are one UV offset
    // applied before anything is sampled, so bloom and shafts bend with it.
    var bend = vec2(0.0);
    let aspect = f32(textureDimensions(hdr).x) / f32(textureDimensions(hdr).y);
    for (var i = 0; i < 8; i = i + 1) {
        let r = pp.rings[i];
        if (r.w <= 0.002) {
            continue;
        }
        let d = (in.uv - r.xy) * vec2(aspect, 1.0);
        let dist = length(d);
        let x = (dist - r.z) / (0.018 + r.z * 0.10);
        if (abs(x) < 3.0) {
            // push pixels outward ahead of the front, pull behind it
            bend = bend - (d / max(dist, 0.001)) * vec2(1.0 / aspect, 1.0) * (-x * exp(-x * x)) * r.w * 0.030;
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
        for (var i: i32 = 0; i < 22; i = i + 1) {
            uvr = uvr + step;
            let s = textureSample(hdr, samp, clamp(uvr, vec2(0.0), vec2(1.0))).rgb;
            acc = acc + bright(s) * wgt;
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
