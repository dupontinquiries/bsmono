//! Falling: how much a drop hurts, and whether a fleeing creature should take
//! it. This is the shared model a creature's *eyes* (which perceive the drop
//! ahead) and *brain* (which weighs the danger behind against the fall) consult
//! so animals stop sprinting off lethal cliffs to escape — a small hop is free,
//! a big drop is only worth it when truly cornered, and a fatal drop is never
//! taken. Pure and deterministic; the world is injected as a drop sampler so the
//! whole decision is testable without a live map.

/// Blocks a body can drop with no harm — a hop down a ledge or two costs nothing.
pub const SAFE_FALL_BLOCKS: i32 = 3;

/// Damage taken on landing after falling `fall_blocks`. Nothing up to
/// [`SAFE_FALL_BLOCKS`]; beyond that it grows faster than linear, so a long fall
/// is genuinely dangerous rather than merely a little worse than a short one
/// (1, 3, 6, 10, 15, … per extra block past the safe height).
pub fn fall_damage(fall_blocks: i32) -> i32 {
    let over = (fall_blocks - SAFE_FALL_BLOCKS).max(0);
    over * (over + 1) / 2
}

/// Whether a creature should accept a drop of `fall_blocks` while fleeing, given
/// its `current_hp` and `desperation` in `[0,1]` (0 = calm wander, 1 = predator
/// on its heels). A safe hop is always fine; a damaging drop is taken only when
/// the fraction of health it would cost is within what the creature's desperation
/// tolerates; and a drop that would kill (or knock out) the creature is *never*
/// taken, however panicked — animals don't leap to their death to escape.
pub fn drop_is_acceptable(fall_blocks: i32, current_hp: i32, desperation: f32) -> bool {
    if fall_blocks <= SAFE_FALL_BLOCKS {
        return true;
    }
    let dmg = fall_damage(fall_blocks);
    if dmg >= current_hp {
        return false;
    }
    let cost_fraction = dmg as f32 / current_hp.max(1) as f32;
    cost_fraction <= desperation.clamp(0.0, 1.0)
}

/// Pick a flee step that runs away from `threat_xz` but refuses to leap off a
/// cliff the fall model deems unacceptable. `drop_ahead(x, z)` is the *eyes*: it
/// returns how many blocks the ground falls from the creature's feet to the
/// surface at `(x, z)` (0 = level or uphill, positive = a drop). The *brain*
/// scans the straight-away heading first, then fans out to either side, taking
/// the first heading whose landing is an acceptable drop; if every escape
/// heading ends in a bad fall it holds at the edge (returns no movement) rather
/// than jumping. Returns the `(dx, dz)` step to apply.
pub fn flee_step_avoiding_cliffs(
    self_xz: (f32, f32),
    threat_xz: (f32, f32),
    step: f32,
    current_hp: i32,
    desperation: f32,
    drop_ahead: impl Fn(f32, f32) -> i32,
) -> (f32, f32) {
    let away = (self_xz.0 - threat_xz.0, self_xz.1 - threat_xz.1);
    let len = (away.0 * away.0 + away.1 * away.1).sqrt();
    if len < 1e-6 || step <= 0.0 {
        return (0.0, 0.0);
    }
    let base = away.1.atan2(away.0); // heading pointing directly away from the threat
    // Straight away first, then progressively wider deflections to both sides.
    const FAN: [f32; 7] = [0.0, 0.35, -0.35, 0.7, -0.7, 1.15, -1.15];
    for d in FAN {
        let h = base + d;
        let (dx, dz) = (h.cos() * step, h.sin() * step);
        let drop = drop_ahead(self_xz.0 + dx, self_xz.1 + dz);
        if drop_is_acceptable(drop, current_hp, desperation) {
            return (dx, dz);
        }
    }
    (0.0, 0.0) // cornered: every escape heading drops off a cliff it won't risk
}

/// How high a walking creature can step up in a single move (blocks). A ledge
/// taller than this is a wall it must go around, not scale.
pub const STEP_UP_BLOCKS: i32 = 1;

/// Can a creature cross from a cell whose surface block is at `from_y` to an
/// adjacent cell whose surface block is at `to_y`? Rising more than `step_up` is
/// an unclimbable wall (this is what stops creatures from pathing straight up a
/// ten-block cliff); level ground or a small step up is fine; going *down* is
/// allowed only when [`drop_is_acceptable`] says the fall is worth it — so calm
/// animals won't wander off ledges and none will leap to their death.
pub fn can_traverse(from_y: i32, to_y: i32, step_up: i32, current_hp: i32, desperation: f32) -> bool {
    let rise = to_y - from_y;
    if rise > step_up {
        return false; // too tall to climb
    }
    if rise >= 0 {
        return true; // level or a permitted step up
    }
    drop_is_acceptable(-rise, current_hp, desperation)
}

/// Sight-based steering: choose a step that heads as close to `desired_heading`
/// as the terrain allows, *proactively* avoiding ledges the creature can't cross
/// rather than walking into them and stopping. `surface_at(x, z) -> Option<i32>`
/// is the creature's *eyes*: the ground height it perceives at a candidate cell
/// (`None` = a void / nothing seen). It scans the desired heading first, then
/// fans out to both sides, and returns the `(dx, dz)` of the first heading whose
/// landing is [traversable](can_traverse); if every direction it can see is
/// blocked, it returns no movement (hemmed in). This is what discourages a
/// creature from ever pathing up a cliff — it sees the wall and turns before it
/// reaches it.
pub fn sighted_step(
    self_xz: (f32, f32),
    desired_heading: f32,
    step: f32,
    current_hp: i32,
    desperation: f32,
    surface_at: impl Fn(f32, f32) -> Option<i32>,
) -> (f32, f32) {
    if step <= 0.0 {
        return (0.0, 0.0);
    }
    let from = match surface_at(self_xz.0, self_xz.1) {
        Some(y) => y,
        None => return (0.0, 0.0),
    };
    const FAN: [f32; 9] = [0.0, 0.4, -0.4, 0.8, -0.8, 1.2, -1.2, 1.6, -1.6];
    for d in FAN {
        let h = desired_heading + d;
        let (dx, dz) = (h.cos() * step, h.sin() * step);
        if path_traversable(from, self_xz, dx, dz, step, current_hp, desperation, &surface_at) {
            return (dx, dz);
        }
    }
    (0.0, 0.0)
}

/// Whether the straight path from `self_xz` by `(dx, dz)` is walkable the WHOLE
/// way — not just at the destination. Sub-samples the path at <=0.5-block
/// intervals and requires each cell along it be traversable from the previous,
/// so a single large step can't tunnel through a thin wall (the cell it would
/// jump is sampled and rejected). For short steps (<=0.5) this is exactly the
/// old single-cell check.
fn path_traversable(
    from: i32,
    self_xz: (f32, f32),
    dx: f32,
    dz: f32,
    step: f32,
    current_hp: i32,
    desperation: f32,
    surface_at: &impl Fn(f32, f32) -> Option<i32>,
) -> bool {
    const SUBSTEP: f32 = 0.5;
    let n = (step / SUBSTEP).ceil().max(1.0) as i32;
    let mut prev = from;
    for k in 1..=n {
        let t = k as f32 / n as f32;
        match surface_at(self_xz.0 + dx * t, self_xz.1 + dz * t) {
            Some(s) => {
                if !can_traverse(prev, s, STEP_UP_BLOCKS, current_hp, desperation) {
                    return false;
                }
                prev = s;
            }
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sight_steers_around_a_cliff_instead_of_into_it() {
        // A cliff wall to the +X side (anything past x=0.5 rises 20 blocks); flat
        // ground (y=64) everywhere else. The creature wants to go straight +X.
        let surface = |x: f32, _z: f32| Some(if x > 0.5 { 84 } else { 64 });
        let (dx, dz) = sighted_step((0.0, 0.0), 0.0, 1.0, 20, 0.0, surface);
        // It must NOT step onto the wall, and it does move (deflects to a side).
        assert!(surface(dx, dz).unwrap() <= 64 + STEP_UP_BLOCKS, "landed on traversable ground");
        assert!(dz.abs() > 0.1, "veered off the straight-ahead cliff heading (dz={dz})");
    }

    #[test]
    fn sight_goes_straight_on_open_ground_and_stops_when_walled_in() {
        let flat = |_x: f32, _z: f32| Some(64);
        let (dx, dz) = sighted_step((0.0, 0.0), 0.0, 1.0, 20, 0.0, flat);
        assert!(dx > 0.9 && dz.abs() < 1e-3, "open ground: heads straight along the desired heading");
        // Standing on a low spot (64) ringed by tall cliffs (90) in every
        // direction → it holds rather than climbing out.
        let walls = |x: f32, z: f32| Some(if x.abs() < 0.4 && z.abs() < 0.4 { 64 } else { 90 });
        assert_eq!(sighted_step((0.0, 0.0), 0.0, 1.0, 20, 0.0, walls), (0.0, 0.0));
    }

    #[test]
    fn fauna_marching_at_a_cliff_for_many_ticks_never_climb_it() {
        // The time-extended cliff test: a 12-block face at x>=8, flat (y=64) below.
        // Spawn low, march +X for ~30s of ticks. It may walk to the base and slide
        // along it, but must never gain the cliff's height.
        let surface = |x: f32, _z: f32| Some(if x >= 8.0 { 76 } else { 64 });
        let mut pos = (0.0f32, 0.0f32);
        let mut peak = 64i32;
        let mut heading = 0.0f32; // +X, into the cliff
        for i in 0..300 {
            let (dx, dz) = sighted_step(pos, heading, 0.4, 20, 0.0, surface);
            if dx == 0.0 && dz == 0.0 {
                heading += 0.7; // momentarily walled in — turn and keep probing
                continue;
            }
            pos.0 += dx;
            pos.1 += dz;
            if let Some(s) = surface(pos.0, pos.1) {
                peak = peak.max(s);
            }
            if i % 40 == 39 {
                heading = 0.0; // keep aiming back at the cliff
            }
        }
        assert!(peak <= 64 + STEP_UP_BLOCKS, "never scaled the cliff over 300 ticks (peak y={peak})");
        assert!(pos.0 < 8.0, "stayed on the low side of the face (x={})", pos.0);
    }

    #[test]
    fn fauna_on_flat_ground_never_drift_in_height() {
        // Spawn low on a flat plain and wander every which way for many ticks; the
        // surface it stands on must stay put (no spurious stepping up/down).
        let flat = |_x: f32, _z: f32| Some(40);
        let mut pos = (0.0f32, 0.0f32);
        for i in 0..300 {
            let heading = i as f32 * 0.37; // sweep all directions over time
            let (dx, dz) = sighted_step(pos, heading, 0.4, 20, 0.0, flat);
            pos.0 += dx;
            pos.1 += dz;
            assert_eq!(flat(pos.0, pos.1), Some(40), "flat ground stays at y=40");
        }
    }

    // A walled cave region split by an impassable plane at column x=5 (top y=60).
    // Floor heights left/right are configurable to test flat vs stepped chambers.
    fn caved(left_floor: i32, right_floor: i32) -> impl Fn(f32, f32) -> Option<i32> {
        move |x: f32, _z: f32| {
            let xi = x.floor() as i32;
            if xi == 5 {
                Some(60) // the dividing wall
            } else if xi > 5 {
                Some(right_floor)
            } else {
                Some(left_floor)
            }
        }
    }

    fn ghost_check(surface: impl Fn(f32, f32) -> Option<i32>) -> f32 {
        // A creature in the LEFT chamber shoving toward the wall (+X) for 500
        // ticks, with z-wiggle, must never cross x=5. Returns the max x reached.
        let mut pos = (2.0f32, 0.5f32);
        let mut max_x = pos.0;
        for i in 0..500 {
            let desired = ((i as f32) * 0.3).sin() * 0.3; // mostly +X, some wiggle
            let (dx, dz) = sighted_step(pos, desired, 0.4, 20, 0.7, &surface);
            pos.0 += dx;
            pos.1 += dz;
            max_x = max_x.max(pos.0);
            assert!(pos.0 < 5.0, "tick {i}: ghosted through the wall to x={}", pos.0);
        }
        max_x
    }

    #[test]
    fn entities_cannot_ghost_through_a_cave_wall_flat() {
        // Both chambers at the same floor (y=40). The wall must confine.
        let reached = ghost_check(caved(40, 40));
        assert!(reached < 5.0 && reached > 4.0, "walked up to the wall face but not through (max x={reached})");
    }

    #[test]
    fn entities_cannot_ghost_through_a_cave_wall_with_elevation() {
        // Right chamber is a 10-block drop (y=30) — even tempting elevation must
        // not let the creature slip past the wall.
        ghost_check(caved(40, 30));
        // And a raised right chamber (y=48) is likewise sealed off.
        ghost_check(caved(40, 48));
    }

    #[test]
    fn a_large_step_cannot_tunnel_through_a_one_block_wall() {
        // The regression: with a big per-tick step a creature used to be able to
        // jump clean over a 1-cell wall (only the destination was checked). Now
        // the path is sub-sampled, so even a huge step is blocked by the wall.
        let surface = caved(40, 40);
        let mut pos = (4.0f32, 0.5f32);
        for _ in 0..50 {
            // Step length 1.5 — easily enough to clear the 1-wide wall cell.
            let (dx, dz) = sighted_step(pos, 0.0, 1.5, 20, 1.0, &surface);
            pos.0 += dx;
            pos.1 += dz;
            assert!(pos.0 < 5.0, "a large step tunneled through the wall to x={}", pos.0);
        }
    }

    #[test]
    fn cannot_scale_a_cliff_but_can_step_a_ledge_and_drop_sensibly() {
        let hp = 20;
        // The bug: a ten-block rise must be impassable (no unbounded step height).
        assert!(!can_traverse(64, 75, STEP_UP_BLOCKS, hp, 0.0), "can't climb an 11-block cliff");
        assert!(!can_traverse(64, 66, STEP_UP_BLOCKS, hp, 1.0), "even desperate, can't climb 2 blocks");
        // A single-block step up and level ground are fine.
        assert!(can_traverse(64, 65, STEP_UP_BLOCKS, hp, 0.0), "a 1-block ledge is climbable");
        assert!(can_traverse(64, 64, STEP_UP_BLOCKS, hp, 0.0), "level ground");
        // Going down: a small drop is free; a big drop only when desperate and
        // survivable; a lethal drop never.
        assert!(can_traverse(64, 62, STEP_UP_BLOCKS, hp, 0.0), "a 2-block drop is a free hop");
        assert!(!can_traverse(64, 57, STEP_UP_BLOCKS, hp, 0.0), "calm: won't drop 7 blocks");
        assert!(can_traverse(64, 57, STEP_UP_BLOCKS, hp, 1.0), "desperate: takes the survivable 7-block drop");
        let lethal = 64 - (SAFE_FALL_BLOCKS..40).find(|&h| fall_damage(h) >= hp).unwrap();
        assert!(!can_traverse(64, lethal, STEP_UP_BLOCKS, hp, 1.0), "never a lethal drop");
    }

    #[test]
    fn small_drops_are_free_then_damage_climbs_fast() {
        for h in 0..=SAFE_FALL_BLOCKS {
            assert_eq!(fall_damage(h), 0, "a {h}-block hop is harmless");
        }
        // Beyond the safe height it rises super-linearly and monotonically.
        let mut prev = 0;
        for h in SAFE_FALL_BLOCKS + 1..=20 {
            let d = fall_damage(h);
            assert!(d > prev, "damage climbs with height ({h}: {d})");
            prev = d;
        }
        // A long fall dwarfs a marginal one (super-linear, not 1-per-block).
        assert!(fall_damage(13) > fall_damage(7) * 3, "long falls hurt disproportionately");
    }

    #[test]
    fn a_calm_animal_refuses_a_damaging_drop_a_cornered_one_gambles() {
        let hp = 20;
        // Calm: a hop is fine, anything that hurts is not.
        assert!(drop_is_acceptable(SAFE_FALL_BLOCKS, hp, 0.0));
        assert!(!drop_is_acceptable(SAFE_FALL_BLOCKS + 1, hp, 0.0));
        // Cornered (desperation 1): take any *survivable* drop...
        assert!(drop_is_acceptable(8, hp, 1.0), "fall_damage(8)={} < {hp}", fall_damage(8));
        // ...but never a fatal one, however panicked.
        let lethal = (SAFE_FALL_BLOCKS..40).find(|&h| fall_damage(h) >= hp).unwrap();
        assert!(!drop_is_acceptable(lethal, hp, 1.0), "won't leap to its death");
        // A mid-panic animal accepts a small cost but not a large one.
        assert!(drop_is_acceptable(5, hp, 0.5)); // fall_damage(5)=3 -> 15% <= 50%
        assert!(!drop_is_acceptable(5, hp, 0.05)); // 15% > 5%
    }

    #[test]
    fn flee_deflects_along_a_cliff_edge_when_calm() {
        // A cliff lies straight away from the threat (to the +X side); level
        // ground to the sides. A calm animal should NOT step over the cliff; it
        // deflects to a safe heading instead.
        let self_xz = (0.0, 0.0);
        let threat_xz = (-1.0, 0.0); // threat behind (-X) -> flee toward +X
        let drop_ahead = |x: f32, _z: f32| if x > 0.5 { 20 } else { 0 };
        let (dx, dz) = flee_step_avoiding_cliffs(self_xz, threat_xz, 1.0, 20, 0.0, drop_ahead);
        assert!(drop_ahead(self_xz.0 + dx, self_xz.1 + dz) <= SAFE_FALL_BLOCKS, "landed somewhere safe");
        assert!(dz.abs() > 0.1, "it veered sideways rather than off the edge (dz={dz})");
    }

    #[test]
    fn flee_holds_at_the_edge_when_boxed_in_but_leaps_when_desperate() {
        // A lethal drop in every direction.
        let drop_ahead = |_x: f32, _z: f32| 30;
        // Calm/boxed-in: refuse to move (better cornered than dead).
        let calm = flee_step_avoiding_cliffs((0.0, 0.0), (-1.0, 0.0), 1.0, 20, 0.0, drop_ahead);
        assert_eq!(calm, (0.0, 0.0), "won't jump off a lethal cliff");
        // Even fully desperate, a *lethal* drop is still refused.
        let panicked = flee_step_avoiding_cliffs((0.0, 0.0), (-1.0, 0.0), 1.0, 20, 1.0, drop_ahead);
        assert_eq!(panicked, (0.0, 0.0), "a lethal drop is off-limits even in panic");
        // But a survivable drop everywhere IS taken when desperate.
        let survivable = |_x: f32, _z: f32| 7; // fall_damage(7)=10 < 20
        let bolt = flee_step_avoiding_cliffs((0.0, 0.0), (-1.0, 0.0), 1.0, 20, 1.0, survivable);
        assert!(bolt.0 > 0.1, "cornered but the drop is survivable, so it bolts (dx={})", bolt.0);
    }
}
