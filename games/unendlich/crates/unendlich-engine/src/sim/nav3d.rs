//! `sim::nav3d` — the reactive 3D move-direction pick for swimmers and fliers.
//!
//! Ground creatures path in 2.5D (the existing nav). Fish and dragons need the
//! third axis, but the full picture is expensive — so this core does the cheap
//! local part: given a 3×3×3 occupancy around the creature and a desired
//! direction, choose which of the 26 neighbouring cells to move toward.
//!
//! The occupancy is a flat `[bool; 27]` indexed `z*9 + y*3 + x` with each axis
//! in `{0,1,2}` (the creature sits at the centre, index 13). A cell that is
//! `true` is blocked. [`step3d`] biases toward `goal_dir`, refuses blocked
//! cells, and returns a unit-ish direction — or all-zero when it is hemmed in
//! or has no aligned opening.
//!
//! Medium matters for the vertical axis: an [`Medium::Air`] flier corrects
//! toward its target altitude (`altitude_err > 0` means "climb"), while a
//! [`Medium::Water`] swimmer is buoyancy-neutral and ignores altitude entirely,
//! moving vertically only as the goal asks. Pure: no clock, no allocation; ties
//! resolve to the lowest cell index, so the same situation always steps the
//! same way.

const EPS: f32 = 1e-6;

/// The medium a creature moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    /// Buoyancy-neutral; ignores altitude correction.
    Water,
    /// Corrects toward a target altitude.
    Air,
}

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

fn norm3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Choose a local move direction.
///
/// Returns a unit-length vector pointing at the best unblocked neighbour
/// (the one whose direction is most aligned with the desired heading), or
/// `[0,0,0]` when every neighbour is blocked or none is aligned with the goal.
pub fn step3d(
    blocked: &[bool; 27],
    goal_dir: [f32; 3],
    medium: Medium,
    altitude_err: f32,
) -> [f32; 3] {
    // Build the desired heading: goal, plus an altitude pull for fliers.
    let alt = match medium {
        Medium::Air => clampf(altitude_err, -1.0, 1.0),
        Medium::Water => 0.0,
    };
    let desired = [goal_dir[0], goal_dir[1] + alt, goal_dir[2]];
    let dn = norm3(desired);
    let dhat = if dn > EPS {
        [desired[0] / dn, desired[1] / dn, desired[2] / dn]
    } else {
        [0.0, 0.0, 0.0]
    };

    let mut best_score = 0.0f32; // require strictly-positive alignment to move
    let mut best_dir = [0.0f32, 0.0, 0.0];
    let mut found = false;

    let mut dz = -1i32;
    while dz <= 1 {
        let mut dy = -1i32;
        while dy <= 1 {
            let mut dx = -1i32;
            while dx <= 1 {
                if !(dx == 0 && dy == 0 && dz == 0) {
                    let idx = ((dz + 1) * 9 + (dy + 1) * 3 + (dx + 1)) as usize;
                    if !blocked[idx] {
                        let off = [dx as f32, dy as f32, dz as f32];
                        let on = norm3(off);
                        let ohat = [off[0] / on, off[1] / on, off[2] / on];
                        let score =
                            dhat[0] * ohat[0] + dhat[1] * ohat[1] + dhat[2] * ohat[2];
                        if !found || score > best_score {
                            best_score = score;
                            best_dir = ohat;
                            found = true;
                        }
                    }
                }
                dx += 1;
            }
            dy += 1;
        }
        dz += 1;
    }

    // Move only toward a positively-aligned opening; otherwise hold position.
    if found && best_score > EPS {
        best_dir
    } else {
        [0.0, 0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit_signed(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 100_000.0 - 1.0
    }

    fn open() -> [bool; 27] {
        [false; 27]
    }
    fn idx_of(dx: i32, dy: i32, dz: i32) -> usize {
        ((dz + 1) * 9 + (dy + 1) * 3 + (dx + 1)) as usize
    }
    fn snap(v: f32) -> i32 {
        if v > 0.5 {
            1
        } else if v < -0.5 {
            -1
        } else {
            0
        }
    }

    #[test]
    fn moves_toward_goal_in_open_space() {
        let r = step3d(&open(), [1.0, 0.0, 0.0], Medium::Water, 0.0);
        // points along +x
        assert!(r[0] > 0.0, "expected +x, got {:?}", r);
        let dot = r[0] * 1.0 + r[1] * 0.0 + r[2] * 0.0;
        assert!(dot > 0.0);
    }

    #[test]
    fn avoids_blocked_cells() {
        // block the straight +x cell; goal is +x → must pick a different open cell
        let mut b = open();
        b[idx_of(1, 0, 0)] = true;
        let r = step3d(&b, [1.0, 0.0, 0.0], Medium::Water, 0.0);
        assert_ne!(r, [0.0, 0.0, 0.0]);
        // the cell we actually head into must be open
        let cell = idx_of(snap(r[0]), snap(r[1]), snap(r[2]));
        assert!(!b[cell], "stepped into a blocked cell");
    }

    #[test]
    fn air_corrects_altitude_but_water_does_not() {
        let goal = [1.0, 0.0, 0.0];
        // big positive altitude error = "climb"
        let air = step3d(&open(), goal, Medium::Air, 1.0);
        assert!(air[1] > 0.0, "air should climb, got {:?}", air);
        let water = step3d(&open(), goal, Medium::Water, 1.0);
        assert_eq!(water[1], 0.0, "water should ignore altitude, got {:?}", water);
    }

    #[test]
    fn all_blocked_yields_zero() {
        let mut b = open();
        for i in 0..27 {
            b[i] = true;
        }
        let r = step3d(&b, [1.0, 1.0, 0.0], Medium::Air, 0.5);
        assert_eq!(r, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn prop_output_bounded_and_open() {
        let mut seed = 0x3D00_u32;
        for _ in 0..4000 {
            let mut b = open();
            for i in 0..27 {
                b[i] = lcg(&mut seed) & 3 == 0;
            }
            b[13] = false; // centre is the creature, never "blocked"
            let goal = [
                unit_signed(&mut seed),
                unit_signed(&mut seed),
                unit_signed(&mut seed),
            ];
            let medium = if lcg(&mut seed) & 1 == 0 {
                Medium::Air
            } else {
                Medium::Water
            };
            let alt = unit_signed(&mut seed) * 2.0;
            let r = step3d(&b, goal, medium, alt);
            let n = norm3(r);
            assert!(n <= 1.0 + 1e-4, "not unit-ish: {}", n);
            if n > EPS {
                let cell = idx_of(snap(r[0]), snap(r[1]), snap(r[2]));
                assert!(!b[cell], "headed into a blocked cell");
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x9F9F_u32;
        for _ in 0..3000 {
            let mut b = open();
            for i in 0..27 {
                b[i] = lcg(&mut seed) & 1 == 0;
            }
            let goal = [
                unit_signed(&mut seed),
                unit_signed(&mut seed),
                unit_signed(&mut seed),
            ];
            let medium = if lcg(&mut seed) & 1 == 0 {
                Medium::Air
            } else {
                Medium::Water
            };
            let alt = unit_signed(&mut seed);
            assert_eq!(
                step3d(&b, goal, medium, alt),
                step3d(&b, goal, medium, alt)
            );
        }
    }

    #[test]
    fn no_goal_no_move() {
        // zero goal, water (no altitude pull) → nothing to head toward
        let r = step3d(&open(), [0.0, 0.0, 0.0], Medium::Water, 0.0);
        assert_eq!(r, [0.0, 0.0, 0.0]);
    }
}
