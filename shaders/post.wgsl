// Post-processing: single-pass bloom + lightning flash + ACES tonemap +
// saturation + vignette + film grain + rain streaks.

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct PostParams {
    a: vec4<f32>, // exposure, vignette, time, lightning
    b: vec4<f32>, // bloom strength, night, rain, unused
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
    var col = textureSample(hdr, samp, in.uv).rgb;

    // --- single-pass bloom: a 12-tap ring at two radii, weighted to a soft disc.
    // Cheap enough for a phone GPU; the MSAA resolve already softened the source.
    let texel = vec2(1.0) / vec2<f32>(textureDimensions(hdr));
    var glow = vec3(0.0);
    for (var i: i32 = 0; i < 12; i = i + 1) {
        let a = f32(i) * 0.5236;
        let dir = vec2(cos(a), sin(a));
        glow = glow + bright(textureSample(hdr, samp, in.uv + dir * texel * 3.0).rgb) * 0.6;
        glow = glow + bright(textureSample(hdr, samp, in.uv + dir * texel * 7.5).rgb) * 0.4;
    }
    glow = glow / 12.0;
    col = col + glow * pp.b.x;

    // --- lightning: a full-frame flash on top of the lit scene
    col = col + vec3(0.55, 0.58, 0.8) * params.w * 0.9;

    // exposure + ACES
    col = col * params.x;
    col = aces(col);

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
