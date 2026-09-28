// Caustics: sunlight focused by a moving water surface into bright,
// wandering filaments on whatever lies under it. The iteration folds a
// grid of sines through itself (the well-known tileable-caustic trick,
// after Dave Hoskins); two layers at different scales and speeds, and the
// red and blue a hair apart, the way a real caustic splits.

fn caustic_layer(p: vec2<f32>, t: f32) -> f32 {
    let tau = 6.28318;
    let q = (fract(p / tau) * tau) - vec2(250.0);
    var i = q;
    var c = 1.0;
    let inten = 0.005;
    for (var n = 0; n < 4; n = n + 1) {
        let tt = t * (1.0 - (3.5 / f32(n + 1)));
        i = q + vec2(cos(tt - i.x) + sin(tt + i.y), sin(tt - i.y) + cos(tt + i.x));
        c = c + 1.0 / length(vec2(q.x / (sin(i.x + tt) / inten), q.y / (cos(i.y + tt) / inten)));
    }
    c = c / 4.0;
    c = 1.17 - pow(c, 1.4);
    return pow(abs(c), 8.0);
}

// Caustic light at world `xz` (0..~2, mostly near 0 with bright lines),
// per colour.
fn caustic(xz: vec2<f32>, t: f32) -> vec3<f32> {
    let p = xz * 0.55;
    let a = caustic_layer(p, t * 0.55);
    let b = caustic_layer(p * 1.7 + vec2(3.1, 1.3), t * 0.8 + 11.0);
    let r = caustic_layer(p + vec2(0.012, 0.0), t * 0.55);
    let bl = caustic_layer(p - vec2(0.012, 0.0), t * 0.55);
    // squared and lifted: thin bright lines on near-black, not a soft haze
    let g = a * 0.7 + b * 0.45;
    let c = vec3(r * 0.7 + b * 0.45, g, bl * 0.7 + b * 0.45);
    return clamp(c * c * 3.2, vec3(0.0), vec3(3.0));
}
