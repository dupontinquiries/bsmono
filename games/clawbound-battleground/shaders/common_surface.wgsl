// --- shared: the ground's height field (prepended to terrain and post by
// render.rs). Materials blend by it and water fills its lows; the post
// pass's reflections read the same field, so they are exactly where the
// terrain shows water.

fn surf_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(127.1, 311.7));
    q = q + dot(q, q + 19.19);
    return fract(q.x * q.y);
}

fn surf_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(surf_hash(i), surf_hash(i + vec2(1.0, 0.0)), u.x), mix(surf_hash(i + vec2(0.0, 1.0)), surf_hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

// Height of the ground's surface detail, about 0..1: broad undulation,
// a middle scale, and small bumps.
fn surface_height(p: vec2<f32>) -> f32 {
    return surf_noise(p * 0.21) * 0.5 + surf_noise(p * 0.83 + vec2(7.3, 1.9)) * 0.32 + surf_noise(p * 3.1 + vec2(2.1, 9.4)) * 0.18;
}

// Standing water: a level that rises with the wetness fills the height
// field's lows -- light rain fills the cracks and hollows, a downpour
// floods more -- and only level ground holds it. 0 dry .. 1 a puddle.
fn water_fill(h: f32, wet: f32, up: f32) -> f32 {
    if (wet < 0.02) {
        return 0.0;
    }
    let level = mix(0.18, 0.56, wet);
    return (1.0 - smoothstep(level - 0.035, level + 0.035, h)) * smoothstep(0.86, 0.95, up) * smoothstep(0.02, 0.2, wet);
}
