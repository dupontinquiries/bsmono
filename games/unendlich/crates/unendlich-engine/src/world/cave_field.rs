//! `world::cave_field` — WG1 (r102): a real multi-scale, depth-stratified cave
//! carve for worldgen.
//!
//! Pure, deterministic, panic-free. Given a world seed and a voxel coordinate
//! plus the local surface height, decides whether the voxel is carved to air.
//!
//! ## Depth expansion (r102 follow-up, guided by `docs/specs/cave_biomes.docx`)
//! The cave guide specifies caves as an *ocean-like layered system* that keeps
//! going far below the surface: smooth limestone/river tunnels near the top, then
//! **abyssal chasms 200–500 blocks deep** (angular, vertical), then the
//! **geothermal deep world** at extreme depth (huge chambers, P 5–50 atm ≈
//! hundreds of blocks down). The earlier version stopped dead at y=4; this
//! version carves all the way down to a deep `WORLD_FLOOR_Y`, with morphology
//! that changes with depth and a smooth taper at both ends (no guillotine).
//!
//! Morphology bands (smoothly blended, no hard seams):
//!   * **SHALLOW** (≈ surface … −64): thin worm tunnels + sparse small chambers
//!     — the limestone/underground-river analog.
//!   * **CHASM**  (≈ −64 … −512): fatter tunnels, more/larger chambers, plus
//!     near-vertical **fault fissures** (ridged 2-D field) — the abyssal analog.
//!   * **DEEP**   (≈ −512 … floor): widest tunnels and the biggest, most frequent
//!     chambers — the geothermal-deep-world cavern scale.
//!
//! Worm tunnels are still the intersection of two ridged-noise iso-surfaces (a
//! connected 1-D tube curve), so caves stay connected, not isolated specks.
//!
//! Boundedness & surface integrity (unchanged invariants):
//!   - Nothing carved at/above `surface_y - SURFACE_BUFFER` (solid crust kept).
//!   - Nothing carved below `WORLD_FLOOR_Y` (a deep bedrock floor), and carve
//!     strength tapers to 0 over `FLOOR_TAPER` blocks above it.
//!   - Carve strength ramps in below the surface so it can't breach the crust.
//!
//! Off by default: the generator only consults this when the cave flag is on.

use super::noise::{noise_2d, noise_3d};

// ── tunables (documented) ────────────────────────────────────────────────────

/// Solid crust preserved below the surface — no carving in `[surface_y - BUF, ∞)`.
pub const SURFACE_BUFFER: f64 = 6.0;
/// Deep world floor: no caves below this y. Caves now span hundreds of blocks
/// (was a hard y=4 floor). Per the guide's chasm (200–500) + geothermal
/// (extreme) depths, the carvable column reaches ~1000 blocks.
pub const WORLD_FLOOR_Y: f64 = -1024.0;
/// Caves taper out over this many blocks above the floor (no hard cut-off).
pub const FLOOR_TAPER: f64 = 48.0;
/// Depth (below the buffered surface) over which carve strength ramps in.
const DEPTH_RAMP: f64 = 18.0;

// Worm-tunnel field frequency (world units → noise units).
const WORM_FREQ: f64 = 0.045;

// Depth anchors for morphology blending (absolute world-y).
const Y_SHALLOW: f64 = 40.0;    // worm/limestone zone reference
const Y_CHASM: f64 = -288.0;    // abyssal chasm zone reference
const Y_DEEP: f64 = -800.0;     // geothermal deep-world reference

// Per-band morphology (tube radius, chamber freq, chamber threshold).
// Deeper → fatter tunnels, lower-freq (bigger) chambers, lower threshold (more).
const TUBE_R_SHALLOW: f64 = 0.07;
const TUBE_R_CHASM: f64 = 0.095;
const TUBE_R_DEEP: f64 = 0.13;
const CH_FREQ_SHALLOW: f64 = 0.013;
const CH_FREQ_CHASM: f64 = 0.010;
const CH_FREQ_DEEP: f64 = 0.0075;
const CH_THRESH_SHALLOW: f64 = 0.82;
const CH_THRESH_CHASM: f64 = 0.74;
const CH_THRESH_DEEP: f64 = 0.66;

// Distinct seed salts so each field is independent.
const SALT_WORM_A: u32 = 0xCA0E_1A01;
const SALT_WORM_B: u32 = 0xCA0E_1B02;
const SALT_CHAMBER: u32 = 0xCA0E_2C03;
const SALT_CHASM: u32 = 0xCA0E_3D04;

/// Ridged 3-D noise: folds [0,1] noise so its ridge is a thin iso-surface.
#[inline]
fn ridged_3d(x: f64, y: f64, z: f64, seed: u32) -> f64 {
    1.0 - (2.0 * noise_3d(x, y, z, seed) - 1.0).abs()
}

/// Ridged 2-D noise (for vertical chasm fissures).
#[inline]
fn ridged_2d(x: f64, z: f64, seed: u32) -> f64 {
    1.0 - (2.0 * noise_2d(x, z, seed) - 1.0).abs()
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Inverse-lerp for a downward axis: 0 at `y >= hi`, 1 at `y <= lo` (`hi > lo`).
#[inline]
fn inv_down(y: f64, hi: f64, lo: f64) -> f64 {
    if hi <= lo { return 0.0; }
    ((hi - y) / (hi - lo)).clamp(0.0, 1.0)
}

/// Depth-based carve gate in `[0,1]`: 0 in the preserved crust, ramping to 1 by
/// `DEPTH_RAMP` below it, held at 1 through the deep column, then tapering back
/// to 0 over `FLOOR_TAPER` above `WORLD_FLOOR_Y`. This keeps the carve bounded at
/// BOTH ends — no breach of the crust, no guillotine at the floor.
#[inline]
fn depth_gate(y: f64, surface_y: f64) -> f64 {
    let ceiling = surface_y - SURFACE_BUFFER;
    if y >= ceiling || y <= WORLD_FLOOR_Y {
        return 0.0;
    }
    let top_ramp = ((ceiling - y) / DEPTH_RAMP).clamp(0.0, 1.0);
    let floor_ramp = ((y - WORLD_FLOOR_Y) / FLOOR_TAPER).clamp(0.0, 1.0);
    top_ramp.min(floor_ramp)
}

/// Morphology at depth `y`: (tube_radius, chamber_freq, chamber_threshold,
/// chasm_weight). Smoothly blended shallow→chasm→deep; `chasm_weight` peaks in
/// the mid (abyssal) band and fades at both ends.
#[inline]
fn morph(y: f64) -> (f64, f64, f64, f64) {
    let a = inv_down(y, Y_SHALLOW, Y_CHASM); // 0 shallow → 1 by chasm depth
    let b = inv_down(y, Y_CHASM, Y_DEEP);    // 0 at chasm → 1 by deep depth
    let tube_r = lerp(lerp(TUBE_R_SHALLOW, TUBE_R_CHASM, a), TUBE_R_DEEP, b);
    let ch_freq = lerp(lerp(CH_FREQ_SHALLOW, CH_FREQ_CHASM, a), CH_FREQ_DEEP, b);
    let ch_thr = lerp(lerp(CH_THRESH_SHALLOW, CH_THRESH_CHASM, a), CH_THRESH_DEEP, b);
    let chasm_weight = a * (1.0 - b); // peaks mid-depth (the abyssal band)
    (tube_r, ch_freq, ch_thr, chasm_weight)
}

/// Is the voxel at `(x,y,z)` carved to air? Pure & deterministic in
/// `(seed, x,y,z,surface_y)`. False everywhere in the preserved crust and below
/// the world floor, so surface integrity holds by construction.
pub fn cave_at(seed: u32, x: f64, y: f64, z: f64, surface_y: f64) -> bool {
    let gate = depth_gate(y, surface_y);
    if gate <= 0.0 {
        return false;
    }

    let (tube_r, ch_freq, ch_thr, chasm_w) = morph(y);

    // WORM TUNNELS — intersection of two ridged fields ⇒ connected tubes.
    let (wx, wy, wz) = (x * WORM_FREQ, y * WORM_FREQ, z * WORM_FREQ);
    let ra = ridged_3d(wx, wy, wz, seed.wrapping_add(SALT_WORM_A));
    let rb = ridged_3d(wx, wy, wz, seed.wrapping_add(SALT_WORM_B));
    // Tube widens a little with the gate (deep tunnels slightly fatter).
    let tube_cut = 1.0 - tube_r * (0.6 + 0.4 * gate);
    if ra >= tube_cut && rb >= tube_cut {
        return true;
    }

    // CHAMBERS — low-freq blobs; bigger and more frequent with depth. Only count
    // where the gate is near full (keeps them off the crust and the floor taper).
    if gate > 0.5 {
        let chamber = noise_3d(x * ch_freq, y * ch_freq, z * ch_freq,
                               seed.wrapping_add(SALT_CHAMBER));
        if chamber > ch_thr {
            return true;
        }
    }

    // ABYSSAL CHASM FISSURES — near-vertical fault voids in the mid band. A
    // low-freq ridged 2-D field over (x,z): near its ridge a thin vertical
    // fissure runs through the chasm band (verticality is the abyssal signature).
    if chasm_w > 0.05 && gate > 0.5 {
        let cf = ridged_2d(x * 0.02, z * 0.02, seed.wrapping_add(SALT_CHASM));
        let cut = 1.0 - 0.04 * chasm_w; // thin fissure, widens a touch mid-band
        if cf >= cut {
            return true;
        }
    }

    false
}

/// Generator convenience: solid-after-caves iff solid before AND not carved.
#[inline]
pub fn solid_after_caves(seed: u32, x: f64, y: f64, z: f64, surface_y: f64, solid_before: bool) -> bool {
    solid_before && !cave_at(seed, x, y, z, surface_y)
}

/// Sample a vertical slice's void fraction. Counts only legally-carvable voxels
/// (below the crust, above the floor). Returns `(carved, considered)`. Pure.
pub fn slice_void_counts(
    seed: u32, surface_of: impl Fn(f64) -> f64,
    x_lo: i32, x_hi: i32, y_lo: i32, y_hi: i32, z: f64,
) -> (u64, u64) {
    let mut carved = 0u64;
    let mut considered = 0u64;
    for xi in x_lo..x_hi {
        let x = xi as f64;
        let sy = surface_of(x);
        for yi in y_lo..y_hi {
            let y = yi as f64;
            if y > WORLD_FLOOR_Y && y < sy - SURFACE_BUFFER {
                considered += 1;
                if cave_at(seed, x, y, z, sy) {
                    carved += 1;
                }
            }
        }
    }
    (carved, considered)
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn surf(x: f64) -> f64 {
        72.0 + 6.0 * (x * 0.05).sin()
    }

    #[test]
    fn no_caves_below_world_floor() {
        for yi in (WORLD_FLOOR_Y as i32 - 20)..(WORLD_FLOOR_Y as i32 + 1) {
            assert!(!cave_at(7, 10.0, yi as f64, 10.0, surf(10.0)),
                "carved at/below world floor y={yi}");
        }
    }

    #[test]
    fn no_caves_in_surface_crust() {
        let seed = 12345;
        for xi in 0..200 {
            let x = xi as f64;
            let sy = surf(x);
            let ceiling = sy - SURFACE_BUFFER;
            let mut yy = ceiling;
            while yy <= sy + 4.0 {
                assert!(!cave_at(seed, x, yy, 3.0, sy),
                    "carved into crust at x={x} y={yy} (surface {sy})");
                yy += 0.5;
            }
        }
    }

    #[test]
    fn surface_integrity_never_carves_above_ceiling() {
        let seed = 99;
        let mut s = 0x1234_5678u64;
        let mut nxt = || { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); (s >> 33) as f64 };
        for _ in 0..30000 {
            let x = (nxt() % 4000.0) - 2000.0;
            let z = (nxt() % 4000.0) - 2000.0;
            let y = (nxt() % 1300.0) - 1100.0;
            let sy = surf(x);
            if cave_at(seed, x, y, z, sy) {
                assert!(y < sy - SURFACE_BUFFER, "carve above the crust ceiling (y={y}, surf={sy})");
                assert!(y > WORLD_FLOOR_Y, "carve below the world floor (y={y})");
            }
        }
    }

    #[test]
    fn caves_now_extend_deep() {
        // The whole point of the expansion: real caves hundreds of blocks down,
        // far below the old y=4 floor. Scan several deep bands; each must carve.
        let seed = 2024;
        for &(y_lo, y_hi, label) in &[
            (-120, -60, "shallow/chasm transition"),
            (-360, -240, "abyssal chasm band"),
            (-760, -640, "geothermal deep band"),
        ] {
            let (carved, considered) =
                slice_void_counts(seed, surf, -400, 400, y_lo, y_hi, 11.0);
            assert!(considered > 0, "{label}: nothing considered");
            let frac = carved as f64 / considered as f64;
            assert!(frac > 0.003, "{label}: no caves at depth (frac={frac})");
            assert!(frac < 0.40, "{label}: runaway carving (frac={frac})");
        }
    }

    #[test]
    fn deepest_cave_reaches_well_below_old_floor() {
        // Find the deepest carved voxel over a wide scan — must be far below the
        // old hard floor of y=4 (we expect it down near WORLD_FLOOR + taper).
        let seed = 555;
        let mut deepest = i32::MAX;
        for xi in -300..300 {
            let x = xi as f64;
            let sy = surf(x);
            for yi in (WORLD_FLOOR_Y as i32 + 1)..0 {
                let y = yi as f64;
                if cave_at(seed, x, y, 5.0, sy) && yi < deepest {
                    deepest = yi;
                }
            }
        }
        assert!(deepest < -400, "deepest cave only reached y={deepest}; expected far deeper");
    }

    #[test]
    fn taper_thins_caves_near_floor() {
        // Void fraction in the bottom taper band should be LESS than in the
        // fully-open deep band just above it (smooth fade, not a guillotine).
        let seed = 4242;
        let floor = WORLD_FLOOR_Y as i32;
        let (c_taper, n_taper) =
            slice_void_counts(seed, surf, -300, 300, floor + 1, floor + 1 + FLOOR_TAPER as i32, 9.0);
        let (c_open, n_open) =
            slice_void_counts(seed, surf, -300, 300, floor + 120, floor + 220, 9.0);
        let f_taper = c_taper as f64 / n_taper.max(1) as f64;
        let f_open = c_open as f64 / n_open.max(1) as f64;
        assert!(f_taper < f_open + 1e-9, "taper band ({f_taper}) not thinner than open band ({f_open})");
    }

    #[test]
    fn deterministic() {
        let seed = 555;
        for _ in 0..3 {
            assert_eq!(cave_at(seed, 12.0, -300.0, -7.0, 64.0),
                       cave_at(seed, 12.0, -300.0, -7.0, 64.0));
        }
    }

    #[test]
    fn void_fraction_bounded_over_full_column() {
        let seed = 2024;
        let (carved, considered) =
            slice_void_counts(seed, surf, -300, 300, WORLD_FLOOR_Y as i32 + 1, 80, 11.0);
        assert!(considered > 0);
        let frac = carved as f64 / considered as f64;
        assert!(frac > 0.005, "no caves (frac={frac})");
        assert!(frac < 0.35, "runaway carving over full column (frac={frac})");
    }

    #[test]
    fn seed_varying() {
        let mut diff = 0u64;
        for xi in -150..150 {
            let x = xi as f64;
            let sy = surf(x);
            for yi in -500..0 {
                let y = yi as f64;
                if cave_at(1, x, y, 5.0, sy) != cave_at(2, x, y, 5.0, sy) {
                    diff += 1;
                }
            }
        }
        assert!(diff > 0, "seeds produced identical caves");
    }

    #[test]
    fn solid_after_caves_only_removes() {
        let seed = 42;
        for yi in -700..0 {
            let y = yi as f64;
            let sy = 64.0;
            assert!(!solid_after_caves(seed, 3.0, y, 3.0, sy, false));
            let s = solid_after_caves(seed, 3.0, y, 3.0, sy, true);
            assert_eq!(s, !cave_at(seed, 3.0, y, 3.0, sy));
        }
    }

    #[test]
    fn tunnels_are_connected_not_isolated_specks() {
        let seed = 7;
        let sy = 80.0;
        let z = 0.0;
        let mut carved = 0u64;
        let mut connected = 0u64;
        for xi in -150..150 {
            for yi in -400..70 {
                let (x, y) = (xi as f64, yi as f64);
                if y >= sy - SURFACE_BUFFER { continue; }
                if cave_at(seed, x, y, z, sy) {
                    carved += 1;
                    let nb = cave_at(seed, x+1.0, y, z, sy)
                          || cave_at(seed, x-1.0, y, z, sy)
                          || cave_at(seed, x, y+1.0, z, sy)
                          || cave_at(seed, x, y-1.0, z, sy)
                          || cave_at(seed, x, y, z+1.0, sy)
                          || cave_at(seed, x, y, z-1.0, sy);
                    if nb { connected += 1; }
                }
            }
        }
        assert!(carved > 100, "too few carved voxels to assess connectivity ({carved})");
        let conn_frac = connected as f64 / carved as f64;
        assert!(conn_frac > 0.6,
            "caves look like isolated specks, not tunnels (connected {conn_frac:.2})");
    }
}
