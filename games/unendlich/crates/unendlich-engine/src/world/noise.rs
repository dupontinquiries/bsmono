//! Noise functions matching the GLSL compute shader.
//!
//! Important: these implementations intentionally mirror the GPU shader
//! (`assets/shaders/chunk_generation.comp`) so CPU-generated chunks match
//! GPU-generated ones bit-for-bit (modulo float ordering). Don't "improve"
//! one without porting the change to the other.

#[inline(always)]
fn hash_u32(mut x: u32) -> u32 {
    x = ((x >> 16) ^ x).wrapping_mul(0x45d9f3b);
    x = ((x >> 16) ^ x).wrapping_mul(0x45d9f3b);
    (x >> 16) ^ x
}

#[inline(always)]
pub fn hash_2d(x: i32, y: i32, seed: u32) -> u32 {
    hash_u32(
        (x as u32).wrapping_mul(374761393)
            .wrapping_add((y as u32).wrapping_mul(668265263))
            .wrapping_add(seed)
    )
}

#[inline(always)]
pub fn hash_3d(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    hash_u32(
        (x as u32).wrapping_mul(374761393)
            .wrapping_add((y as u32).wrapping_mul(668265263))
            .wrapping_add((z as u32).wrapping_mul(1274126177))
            .wrapping_add(seed)
    )
}

#[inline(always)]
fn hash_to_float(h: u32) -> f64 {
    h as f64 / 4_294_967_295.0
}

#[inline(always)]
fn smoothstep(t: f64) -> f64 {
    // Quintic fade 6t^5 - 15t^4 + 10t^3 (Ken Perlin's "improved noise" curve).
    // The old cubic t^2(3-2t) is only C1: its 2nd derivative is non-zero at the
    // cell boundaries (t=0,1), which shows up as faint grid-aligned creases in
    // the terrain/caves. The quintic is C2 (1st AND 2nd derivatives vanish at
    // the endpoints), so adjacent noise cells blend without the crease.
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn noise_2d(x: f64, y: f64, seed: u32) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - x.floor();
    let yf = y - y.floor();

    let u = smoothstep(xf);
    let v = smoothstep(yf);

    let a = hash_to_float(hash_2d(xi, yi, seed));
    let b = hash_to_float(hash_2d(xi + 1, yi, seed));
    let c = hash_to_float(hash_2d(xi, yi + 1, seed));
    let d = hash_to_float(hash_2d(xi + 1, yi + 1, seed));

    lerp(lerp(a, b, u), lerp(c, d, u), v)
}

pub fn noise_3d(x: f64, y: f64, z: f64, seed: u32) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let zi = z.floor() as i32;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let zf = z - z.floor();

    let u = smoothstep(xf);
    let v = smoothstep(yf);
    let w = smoothstep(zf);

    let n000 = hash_to_float(hash_3d(xi, yi, zi, seed));
    let n100 = hash_to_float(hash_3d(xi + 1, yi, zi, seed));
    let n010 = hash_to_float(hash_3d(xi, yi + 1, zi, seed));
    let n110 = hash_to_float(hash_3d(xi + 1, yi + 1, zi, seed));
    let n001 = hash_to_float(hash_3d(xi, yi, zi + 1, seed));
    let n101 = hash_to_float(hash_3d(xi + 1, yi, zi + 1, seed));
    let n011 = hash_to_float(hash_3d(xi, yi + 1, zi + 1, seed));
    let n111 = hash_to_float(hash_3d(xi + 1, yi + 1, zi + 1, seed));

    lerp(
        lerp(lerp(n000, n100, u), lerp(n010, n110, u), v),
        lerp(lerp(n001, n101, u), lerp(n011, n111, u), v),
        w,
    )
}

/// Multi-octave 2D noise (Fractal Brownian Motion).
pub fn fbm_2d(x: f64, y: f64, seed: u32, octaves: u32, frequency: f64, amplitude: f64) -> f64 {
    let mut value = 0.0;
    let mut total_amplitude = 0.0;
    let mut freq = frequency;
    let mut amp = amplitude;

    for i in 0..octaves {
        value += noise_2d(x * freq, y * freq, seed.wrapping_add(i)) * amp;
        total_amplitude += amp;
        freq *= 2.0;
        amp *= 0.5;
    }

    if total_amplitude > 0.0 {
        value / total_amplitude
    } else {
        0.0
    }
}

/// Multi-octave 3D noise (Fractal Brownian Motion).
pub fn fbm_3d(x: f64, y: f64, z: f64, seed: u32, octaves: u32, frequency: f64, amplitude: f64) -> f64 {
    let mut value = 0.0;
    let mut total_amplitude = 0.0;
    let mut freq = frequency;
    let mut amp = amplitude;

    for i in 0..octaves {
        value += noise_3d(x * freq, y * freq, z * freq, seed.wrapping_add(i)) * amp;
        total_amplitude += amp;
        freq *= 2.0;
        amp *= 0.5;
    }

    if total_amplitude > 0.0 {
        value / total_amplitude
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = noise_2d(123.456, 789.012, 42);
        let b = noise_2d(123.456, 789.012, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn output_in_range() {
        for i in 0..100 {
            let v = noise_2d(i as f64 * 0.7, i as f64 * 1.3, 12345);
            assert!(v >= 0.0 && v <= 1.0, "noise out of [0,1]: {}", v);
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = noise_2d(50.0, 50.0, 1);
        let b = noise_2d(50.0, 50.0, 2);
        assert_ne!(a, b);
    }
}
