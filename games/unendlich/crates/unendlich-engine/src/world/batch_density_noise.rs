//! `batch_density_noise` — batched 3D value-noise / fBm sampling (perf candidate).
//!
//! Round 62, perf thread (r62-02 worldgen). The genuinely-3D part of worldgen —
//! density, caves, ore detail — is a tight per-voxel scalar loop over
//! [`super::noise::noise_3d`] / [`super::noise::fbm_3d`]. This module is the
//! batched, **structure-of-arrays** form of those calls: it processes 8 voxels
//! per step (one contiguous run along an axis), hoisting the octave/seed setup
//! and walking a cache-friendly slice, exactly as the R17 [`super::batch_ridge_noise`]
//! does for the 2D ridge field.
//!
//! ## Bit-identity (determinism gate)
//! Each lane runs the **exact** scalar op sequence of `noise.rs` — same f64
//! operations, same order, no reassociation, no FMA contraction, no fast-math —
//! so every lane is byte-for-byte equal to the scalar `noise_3d`/`fbm_3d`. That
//! keeps the seed→world mapping identical (`world-bench` "all match: true" is
//! preserved). The SoA layout is what the autovectorizer can pack 4-/8-wide; the
//! *result* does not depend on whether it vectorizes, so a batched fast path can
//! never drift from the scalar fallback. The `#[cfg(test)]` block asserts this
//! byte-identity against `super::noise` so the harness re-verifies it on merge.
//!
//! Status: this is the **bit-exact primitive**; wiring it into
//! `cpu_generator`'s density/cave loop and the device measurement are the
//! operator's step (see the round return for the plan + perf-probe commands).
//! No clock/RNG/unwrap; pure and deterministic.
#![allow(dead_code)]

/// Lane width. 8 keeps the SoA arrays in registers for AVX2 (4×f64) / NEON and
/// matches a half/whole CHUNK_SIZE row cleanly.
pub const LANES: usize = 8;

// ---- per-lane scalar mirror of world::noise (f64, identical op order) ----

#[inline(always)]
fn hash_u32_x(mut x: [u32; LANES]) -> [u32; LANES] {
    let mut i = 0;
    while i < LANES {
        x[i] = ((x[i] >> 16) ^ x[i]).wrapping_mul(0x45d9f3b);
        i += 1;
    }
    i = 0;
    while i < LANES {
        x[i] = ((x[i] >> 16) ^ x[i]).wrapping_mul(0x45d9f3b);
        i += 1;
    }
    i = 0;
    while i < LANES {
        x[i] = (x[i] >> 16) ^ x[i];
        i += 1;
    }
    x
}

#[inline(always)]
fn hash_3d_x(x: [i32; LANES], y: [i32; LANES], z: [i32; LANES], seed: u32) -> [u32; LANES] {
    let mut a = [0u32; LANES];
    let mut i = 0;
    while i < LANES {
        a[i] = (x[i] as u32)
            .wrapping_mul(374761393)
            .wrapping_add((y[i] as u32).wrapping_mul(668265263))
            .wrapping_add((z[i] as u32).wrapping_mul(1274126177))
            .wrapping_add(seed);
        i += 1;
    }
    hash_u32_x(a)
}

#[inline(always)]
fn hash_to_float_x(h: [u32; LANES]) -> [f64; LANES] {
    let mut o = [0.0f64; LANES];
    let mut i = 0;
    while i < LANES {
        o[i] = h[i] as f64 / 4_294_967_295.0;
        i += 1;
    }
    o
}

#[inline(always)]
fn smoothstep_x(t: [f64; LANES]) -> [f64; LANES] {
    let mut o = [0.0f64; LANES];
    let mut i = 0;
    while i < LANES {
        o[i] = t[i] * t[i] * t[i] * (t[i] * (t[i] * 6.0 - 15.0) + 10.0);
        i += 1;
    }
    o
}

#[inline(always)]
fn lerp_x(a: [f64; LANES], b: [f64; LANES], t: [f64; LANES]) -> [f64; LANES] {
    let mut o = [0.0f64; LANES];
    let mut i = 0;
    while i < LANES {
        o[i] = a[i] + (b[i] - a[i]) * t[i];
        i += 1;
    }
    o
}

/// 8-wide `noise_3d`: each lane is byte-identical to `super::noise::noise_3d`.
pub fn noise_3d_x8(x: [f64; LANES], y: [f64; LANES], z: [f64; LANES], seed: u32) -> [f64; LANES] {
    let mut xi = [0i32; LANES];
    let mut yi = [0i32; LANES];
    let mut zi = [0i32; LANES];
    let mut xf = [0.0f64; LANES];
    let mut yf = [0.0f64; LANES];
    let mut zf = [0.0f64; LANES];
    let mut i = 0;
    while i < LANES {
        xi[i] = x[i].floor() as i32;
        yi[i] = y[i].floor() as i32;
        zi[i] = z[i].floor() as i32;
        xf[i] = x[i] - x[i].floor();
        yf[i] = y[i] - y[i].floor();
        zf[i] = z[i] - z[i].floor();
        i += 1;
    }
    let u = smoothstep_x(xf);
    let v = smoothstep_x(yf);
    let w = smoothstep_x(zf);

    let mut xi1 = xi;
    let mut yi1 = yi;
    let mut zi1 = zi;
    i = 0;
    while i < LANES {
        xi1[i] = xi[i] + 1;
        yi1[i] = yi[i] + 1;
        zi1[i] = zi[i] + 1;
        i += 1;
    }

    let n000 = hash_to_float_x(hash_3d_x(xi, yi, zi, seed));
    let n100 = hash_to_float_x(hash_3d_x(xi1, yi, zi, seed));
    let n010 = hash_to_float_x(hash_3d_x(xi, yi1, zi, seed));
    let n110 = hash_to_float_x(hash_3d_x(xi1, yi1, zi, seed));
    let n001 = hash_to_float_x(hash_3d_x(xi, yi, zi1, seed));
    let n101 = hash_to_float_x(hash_3d_x(xi1, yi, zi1, seed));
    let n011 = hash_to_float_x(hash_3d_x(xi, yi1, zi1, seed));
    let n111 = hash_to_float_x(hash_3d_x(xi1, yi1, zi1, seed));

    lerp_x(
        lerp_x(lerp_x(n000, n100, u), lerp_x(n010, n110, u), v),
        lerp_x(lerp_x(n001, n101, u), lerp_x(n011, n111, u), v),
        w,
    )
}

/// 8-wide `fbm_3d`: each lane is byte-identical to `super::noise::fbm_3d`.
pub fn fbm_3d_x8(
    x: [f64; LANES],
    y: [f64; LANES],
    z: [f64; LANES],
    seed: u32,
    octaves: u32,
    frequency: f64,
    amplitude: f64,
) -> [f64; LANES] {
    let mut value = [0.0f64; LANES];
    let mut total_amplitude = 0.0f64;
    let mut freq = frequency;
    let mut amp = amplitude;

    let mut o = 0;
    while o < octaves {
        let mut xx = [0.0f64; LANES];
        let mut yy = [0.0f64; LANES];
        let mut zz = [0.0f64; LANES];
        let mut l = 0;
        while l < LANES {
            xx[l] = x[l] * freq;
            yy[l] = y[l] * freq;
            zz[l] = z[l] * freq;
            l += 1;
        }
        let nb = noise_3d_x8(xx, yy, zz, seed.wrapping_add(o));
        l = 0;
        while l < LANES {
            value[l] += nb[l] * amp;
            l += 1;
        }
        total_amplitude += amp;
        freq *= 2.0;
        amp *= 0.5;
        o += 1;
    }

    let mut out = [0.0f64; LANES];
    if total_amplitude > 0.0 {
        let mut l = 0;
        while l < LANES {
            out[l] = value[l] / total_amplitude;
            l += 1;
        }
    }
    out
}

/// Fill `out` with `fbm_3d` sampled along a contiguous **x-row**: voxel `i` is at
/// world `(x0 + i, y, z)`. Processes [`LANES`] at a time; the `<LANES` tail falls
/// back to the scalar reference. Byte-identical to calling
/// `super::noise::fbm_3d` per voxel.
pub fn fbm_3d_row_x(
    out: &mut [f64],
    x0: f64,
    y: f64,
    z: f64,
    seed: u32,
    octaves: u32,
    frequency: f64,
    amplitude: f64,
) {
    let n = out.len();
    let mut base = 0usize;
    while base + LANES <= n {
        let mut xs = [0.0f64; LANES];
        let ys = [y; LANES];
        let zs = [z; LANES];
        let mut i = 0;
        while i < LANES {
            xs[i] = x0 + (base + i) as f64;
            i += 1;
        }
        let b = fbm_3d_x8(xs, ys, zs, seed, octaves, frequency, amplitude);
        out[base..base + LANES].copy_from_slice(&b);
        base += LANES;
    }
    // scalar tail
    while base < n {
        out[base] = super::noise::fbm_3d(x0 + base as f64, y, z, seed, octaves, frequency, amplitude);
        base += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::noise;

    fn lcg(s: &mut u64) -> u64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *s >> 11
    }

    #[test]
    fn noise_and_fbm_x8_are_byte_identical_to_scalar() {
        let mut s = 0xABCD_1234_5678_9F01u64;
        for _ in 0..50_000 {
            let bx = (lcg(&mut s) % 100_000) as f64 * 0.013 - 600.0;
            let y = (lcg(&mut s) % 4000) as f64 * 0.03 - 60.0;
            let z = (lcg(&mut s) % 100_000) as f64 * 0.013 - 600.0;
            let seed = (lcg(&mut s) % 100_000) as u32;
            let oct = 1 + (lcg(&mut s) % 6) as u32;
            let freq = 0.001 + (lcg(&mut s) % 1000) as f64 * 0.01;

            let mut xs = [0.0; LANES];
            let ys = [y; LANES];
            let zs = [z; LANES];
            for i in 0..LANES {
                xs[i] = bx + i as f64;
            }
            let nb = noise_3d_x8(xs, ys, zs, seed);
            let fb = fbm_3d_x8(xs, ys, zs, seed, oct, freq, 1.0);
            for i in 0..LANES {
                assert_eq!(
                    nb[i].to_bits(),
                    noise::noise_3d(xs[i], y, z, seed).to_bits(),
                    "noise_3d lane {} drifted",
                    i
                );
                assert_eq!(
                    fb[i].to_bits(),
                    noise::fbm_3d(xs[i], y, z, seed, oct, freq, 1.0).to_bits(),
                    "fbm_3d lane {} drifted",
                    i
                );
            }
        }
    }

    #[test]
    fn fbm_row_matches_scalar_including_tail() {
        let mut s = 0x0F0F_1234u64;
        for len in [0usize, 1, 7, 8, 9, 16, 31, 32, 33] {
            let bx = (lcg(&mut s) % 1000) as f64;
            let y = (lcg(&mut s) % 1000) as f64 * 0.1;
            let z = (lcg(&mut s) % 1000) as f64 * 0.1;
            let seed = 4242u32;
            let mut out = vec![0.0f64; len];
            fbm_3d_row_x(&mut out, bx, y, z, seed, 4, 0.0173, 1.0);
            for i in 0..len {
                assert_eq!(
                    out[i].to_bits(),
                    noise::fbm_3d(bx + i as f64, y, z, seed, 4, 0.0173, 1.0).to_bits(),
                    "row len {} idx {} drifted",
                    len,
                    i
                );
            }
        }
    }
}
