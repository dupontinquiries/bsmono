//! Parkour movement — the leap / wall-leap / wall-run state machine and
//! kinematics, ported from the mod's `ParkourBase`.
//!
//! Pure and engine-agnostic: it owns the timers (cooldown, charge, wall-leap
//! window, ready warm-up) and computes velocity impulses, but never touches
//! the world or the player directly. The player/input layer calls
//! [`tick`] each frame with the player's situation (selected? airborne?
//! against a wall? how fast?) and [`try_leap`] on the use button, then
//! applies the returned [`Impulse`] to the player's velocity and the
//! [`TickEffects`] flags (fall-damage negation, the "ready" chime).
//!
//! ## What was clarified from the original
//!
//! The leap math, wall detection (`countWalls`/glide), wall-run grip, charge
//! gate, cooldown, and ready warm-up are ported faithfully. The original's
//! `wallLeap` afterboost was murky — it decremented in two places per tick
//! and was never re-armed after a leap, so it fired at most once ever. Here
//! a wall leap arms the window (`wall_leap = max_wall_leap`) and it
//! decrements once per tick while it lasts — the evident intent: a brief
//! extra lift right after kicking off a wall.

/// Tunables for a parkour movement item.
#[derive(Debug, Clone, Copy)]
pub struct ParkourConfig {
    /// Ticks between leaps.
    pub max_cooldown: u32,
    /// Charge that must drain to 0 (while wall-running slowly) before a wall
    /// leap is allowed.
    pub max_charge: u32,
    /// Length of the post-wall-leap afterboost window, in ticks.
    pub max_wall_leap: u32,
    /// Warm-up after selecting the item before it does anything.
    pub max_ready: u32,
    /// Horizontal velocity scale.
    pub multiplier_xz: f64,
    /// Vertical velocity scale.
    pub multiplier_y: f64,
    /// Elytra-flyer variant: stronger vertical kick off the ground.
    pub elytra_flyer: bool,
}

impl Default for ParkourConfig {
    fn default() -> Self {
        // ParkourBase defaults: 13/13/13 timers, 9-tick ready, unit mults.
        Self {
            max_cooldown: 13,
            max_charge: 13,
            max_wall_leap: 13,
            max_ready: 9,
            multiplier_xz: 1.0,
            multiplier_y: 1.0,
            elytra_flyer: false,
        }
    }
}

/// Live per-item state (the NBT counters in the original).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkourState {
    pub cooldown: u32,
    pub wall_leap: u32,
    pub charge: u32,
    pub ready: u32,
}

impl ParkourState {
    /// Fresh state: all timers at their maxima (matches the `-1 → max`
    /// lazy-init in the original).
    pub fn new(cfg: &ParkourConfig) -> Self {
        Self {
            cooldown: cfg.max_cooldown,
            wall_leap: cfg.max_wall_leap,
            charge: cfg.max_charge,
            ready: cfg.max_ready,
        }
    }
}

/// Which leap fired — selects the velocity profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapKind {
    /// Normal mid-air / running leap (also elytra-flyer while airborne).
    Air,
    /// Elytra-flyer kicking off the ground — a stronger vertical launch.
    Ground,
    /// Wall leap — stronger horizontal and vertical than a normal leap.
    Wall,
}

/// A velocity delta to add to the player (`addVelocity` in the original).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Impulse {
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
}

/// The unit look direction for a `(pitch, yaw)` in degrees, matching
/// Minecraft's `Vec3d.fromPitchYaw`:
/// `(-cos(p)·sin(y), -sin(p), cos(p)·cos(y))`.
pub fn look_dir(pitch_deg: f64, yaw_deg: f64) -> [f64; 3] {
    let p = pitch_deg.to_radians();
    let y = yaw_deg.to_radians();
    [-p.cos() * y.sin(), -p.sin(), p.cos() * y.cos()]
}

/// The velocity impulse for a leap of `kind` in look direction `look`,
/// scaled by the item's horizontal/vertical multipliers. Mirrors the
/// `addVelocity` calls in `leapTriggered` / `wallLeapTriggered`:
/// horizontal `mxz · f · look.xz`, vertical `my · g + 0.2 · look.y`.
pub fn leap_impulse(look: [f64; 3], mxz: f64, my: f64, kind: LeapKind) -> Impulse {
    let (xz_f, y_f) = match kind {
        LeapKind::Wall => (0.4, 0.6),
        LeapKind::Ground => (0.3, 1.0),
        LeapKind::Air => (0.3, 0.5),
    };
    Impulse {
        dx: mxz * xz_f * look[0],
        dy: my * y_f + 0.2 * look[1],
        dz: mxz * xz_f * look[2],
    }
}

/// Count adjacent walls ("glide") around the player's feet, replicating the
/// original `countWalls`: for each of the 4 horizontal directions, a solid
/// block two cells up counts, and a solid block one cell up counts only if
/// the feet cell itself is open. Range 0..=8. `solid(dx, dy, dz)` queries
/// solidity relative to the feet cell (so the feet cell is `solid(0,0,0)`).
pub fn count_walls<F: Fn(i32, i32, i32) -> bool>(solid: F) -> u32 {
    let feet_open = !solid(0, 0, 0);
    let mut glide = 0;
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        if solid(dx, 2, dz) {
            glide += 1;
        }
        if feet_open && solid(dx, 1, dz) {
            glide += 1;
        }
    }
    glide
}

/// The player's situation when the use button is pressed.
#[derive(Debug, Clone, Copy)]
pub struct LeapSituation {
    pub airborne: bool,
    pub in_water: bool,
    pub flying: bool,
    pub elytra_flying: bool,
    pub on_ground: bool,
    /// Adjacent-wall count from [`count_walls`].
    pub wall_count: u32,
    /// Unit look direction from [`look_dir`].
    pub look: [f64; 3],
}

/// Attempt a leap (the use action). Returns the impulse and which leap fired,
/// or `None` if blocked (not warmed up, on cooldown, in water, or flying).
/// On any otherwise-valid attempt the cooldown resets, matching the original
/// (even an attempt that produces no leap still spends the cooldown).
pub fn try_leap(
    state: &mut ParkourState,
    cfg: &ParkourConfig,
    sit: &LeapSituation,
) -> Option<(Impulse, LeapKind)> {
    if state.ready != 0 {
        return None; // still warming up
    }
    if state.cooldown != 0 || sit.in_water || sit.flying {
        return None;
    }
    state.cooldown = cfg.max_cooldown;

    // Wall leap: airborne, charged up, and more than one wall to kick off.
    if sit.airborne && state.charge == 0 && sit.wall_count > 1 {
        state.wall_leap = cfg.max_wall_leap; // arm the afterboost window
        let imp = leap_impulse(sit.look, cfg.multiplier_xz, cfg.multiplier_y, LeapKind::Wall);
        return Some((imp, LeapKind::Wall));
    }

    // Normal leap (or elytra-flyer): elytra-flyer launches harder off ground.
    if cfg.elytra_flyer || !sit.elytra_flying {
        let kind = if cfg.elytra_flyer && sit.on_ground {
            LeapKind::Ground
        } else {
            LeapKind::Air
        };
        let imp = leap_impulse(sit.look, cfg.multiplier_xz, cfg.multiplier_y, kind);
        return Some((imp, kind));
    }

    None
}

/// The player's situation each tick (the `inventoryTick` inputs).
#[derive(Debug, Clone, Copy)]
pub struct TickSituation {
    pub selected: bool,
    pub airborne: bool,
    /// Is the feet cell solid (standing on the ground)?
    pub feet_solid: bool,
    /// Is the player facing into the wall they're gliding along?
    pub facing_wall: bool,
    /// Adjacent-wall count from [`count_walls`].
    pub wall_count: u32,
    /// Horizontal speed magnitude.
    pub speed: f64,
}

/// Per-tick outputs the caller applies to the player.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TickEffects {
    /// Upward velocity to add this tick (wall-run grip or wall-leap afterboost).
    pub vertical_boost: f64,
    /// Cancel fall damage this tick (the item negates it while ready).
    pub negate_fall_damage: bool,
    /// Play the "ready" chime this tick (warm-up just completed).
    pub ready_chime: bool,
}

/// Advance the timers one tick and report the per-tick effects. Mirrors
/// `inventoryTick`: cooldown drains always; deselecting resets the warm-up;
/// once warmed up the item negates fall damage, applies wall-run grip while
/// gliding (and drains charge when slow), and otherwise spends the
/// post-wall-leap afterboost window.
pub fn tick(state: &mut ParkourState, cfg: &ParkourConfig, sit: &TickSituation) -> TickEffects {
    let mut fx = TickEffects::default();

    if state.cooldown > 0 {
        state.cooldown -= 1;
    }

    if !sit.selected {
        state.ready = cfg.max_ready;
        return fx;
    }

    if state.ready > 0 {
        if state.ready == 1 {
            fx.ready_chime = true;
        }
        state.ready -= 1;
        return fx;
    }

    // Warmed up.
    fx.negate_fall_damage = true;

    let wall_running = sit.wall_count > 0 && sit.airborne && !sit.feet_solid;
    if wall_running {
        if sit.facing_wall {
            // Grab-hold grip: stronger when moving slowly up the wall.
            fx.vertical_boost += if sit.speed < 0.6 { 0.068 } else { 0.015 };
        }
        // Charge gates the wall leap: drains while clinging slowly, resets
        // if you're moving fast (you have to actually hang on the wall).
        if state.charge > 0 {
            if sit.speed < 0.33 {
                state.charge -= 1;
            } else {
                state.charge = cfg.max_charge;
            }
        }
    } else if state.wall_leap > 0 {
        // Brief extra lift right after a wall leap.
        fx.vertical_boost += 0.024;
        state.wall_leap -= 1;
    }

    fx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }
    fn approx3(v: [f64; 3], e: [f64; 3]) -> bool {
        approx(v[0], e[0]) && approx(v[1], e[1]) && approx(v[2], e[2])
    }

    #[test]
    fn look_dir_cardinals() {
        assert!(approx3(look_dir(0.0, 0.0), [0.0, 0.0, 1.0]), "{:?}", look_dir(0.0, 0.0));
        assert!(approx3(look_dir(0.0, 90.0), [-1.0, 0.0, 0.0]), "{:?}", look_dir(0.0, 90.0));
        assert!(approx3(look_dir(-90.0, 0.0), [0.0, 1.0, 0.0]), "{:?}", look_dir(-90.0, 0.0));
        // Always unit length.
        let v = look_dir(33.0, 217.0);
        assert!(approx(v[0] * v[0] + v[1] * v[1] + v[2] * v[2], 1.0));
    }

    #[test]
    fn leap_profiles() {
        let fwd = [0.0, 0.0, 1.0];
        let air = leap_impulse(fwd, 1.0, 1.0, LeapKind::Air);
        assert!(approx(air.dx, 0.0) && approx(air.dy, 0.5) && approx(air.dz, 0.3));
        let wall = leap_impulse(fwd, 1.0, 1.0, LeapKind::Wall);
        assert!(approx(wall.dx, 0.0) && approx(wall.dy, 0.6) && approx(wall.dz, 0.4));
        let ground = leap_impulse(fwd, 1.0, 1.0, LeapKind::Ground);
        assert!(approx(ground.dy, 1.0) && approx(ground.dz, 0.3));
    }

    #[test]
    fn multipliers_scale_impulse() {
        let imp = leap_impulse([0.0, 0.0, 1.0], 2.0, 3.0, LeapKind::Air);
        assert!(approx(imp.dz, 0.6) && approx(imp.dy, 1.5)); // .3*2, .5*3
    }

    #[test]
    fn count_walls_counts_two_heights() {
        // Wall on +x at both heights, feet open → 2.
        let one_wall = |dx: i32, _dy: i32, dz: i32| dx == 1 && dz == 0;
        assert_eq!(count_walls(one_wall), 2);
        // Walls all around at both heights, feet open → 8.
        let boxed = |dx: i32, _dy: i32, dz: i32| (dx, dz) != (0, 0);
        assert_eq!(count_walls(boxed), 8);
        // Feet solid suppresses the lower (body) checks → only the 4 upper.
        let feet_solid_all = |_dx: i32, _dy: i32, _dz: i32| true;
        assert_eq!(count_walls(feet_solid_all), 4);
    }

    fn ready_state(cfg: &ParkourConfig) -> ParkourState {
        let mut s = ParkourState::new(cfg);
        s.ready = 0;
        s.cooldown = 0;
        s
    }

    #[test]
    fn wall_leap_when_airborne_charged_and_walled() {
        let cfg = ParkourConfig::default();
        let mut s = ready_state(&cfg);
        s.charge = 0;
        let sit = LeapSituation {
            airborne: true,
            in_water: false,
            flying: false,
            elytra_flying: false,
            on_ground: false,
            wall_count: 2,
            look: [0.0, 0.0, 1.0],
        };
        let (imp, kind) = try_leap(&mut s, &cfg, &sit).expect("should wall leap");
        assert_eq!(kind, LeapKind::Wall);
        assert!(approx(imp.dz, 0.4));
        assert_eq!(s.cooldown, cfg.max_cooldown, "cooldown reset");
        assert_eq!(s.wall_leap, cfg.max_wall_leap, "afterboost armed");
    }

    #[test]
    fn normal_leap_when_grounded() {
        let cfg = ParkourConfig::default();
        let mut s = ready_state(&cfg);
        let sit = LeapSituation {
            airborne: false,
            in_water: false,
            flying: false,
            elytra_flying: false,
            on_ground: true,
            wall_count: 0,
            look: [0.0, 0.0, 1.0],
        };
        let (imp, kind) = try_leap(&mut s, &cfg, &sit).expect("should leap");
        assert_eq!(kind, LeapKind::Air);
        assert!(approx(imp.dy, 0.5));
    }

    #[test]
    fn leap_blocked_when_not_ready_or_cooling_or_in_water() {
        let cfg = ParkourConfig::default();
        let base = LeapSituation {
            airborne: false, in_water: false, flying: false, elytra_flying: false,
            on_ground: true, wall_count: 0, look: [0.0, 0.0, 1.0],
        };
        let mut warming = ParkourState::new(&cfg); // ready > 0
        assert!(try_leap(&mut warming, &cfg, &base).is_none());

        let mut cooling = ready_state(&cfg);
        cooling.cooldown = 5;
        assert!(try_leap(&mut cooling, &cfg, &base).is_none());

        let mut s = ready_state(&cfg);
        let wet = LeapSituation { in_water: true, ..base };
        assert!(try_leap(&mut s, &cfg, &wet).is_none());
    }

    #[test]
    fn tick_warmup_then_chime() {
        let cfg = ParkourConfig::default();
        let mut s = ParkourState::new(&cfg); // ready = 9
        let sit = TickSituation {
            selected: true, airborne: false, feet_solid: true,
            facing_wall: false, wall_count: 0, speed: 0.0,
        };
        let mut chimed = false;
        for _ in 0..cfg.max_ready {
            chimed |= tick(&mut s, &cfg, &sit).ready_chime;
        }
        assert!(chimed, "ready chime should fire as warm-up completes");
        assert_eq!(s.ready, 0);
        // Now warmed up: fall damage negated.
        assert!(tick(&mut s, &cfg, &sit).negate_fall_damage);
    }

    #[test]
    fn tick_deselect_resets_warmup() {
        let cfg = ParkourConfig::default();
        let mut s = ready_state(&cfg); // ready = 0
        let sit = TickSituation {
            selected: false, airborne: false, feet_solid: true,
            facing_wall: false, wall_count: 0, speed: 0.0,
        };
        tick(&mut s, &cfg, &sit);
        assert_eq!(s.ready, cfg.max_ready, "deselect re-arms warm-up");
    }

    #[test]
    fn tick_wallrun_grip_and_charge_drain() {
        let cfg = ParkourConfig::default();
        let mut s = ready_state(&cfg); // charge = max
        let slow_cling = TickSituation {
            selected: true, airborne: true, feet_solid: false,
            facing_wall: true, wall_count: 2, speed: 0.1,
        };
        let fx = tick(&mut s, &cfg, &slow_cling);
        assert!(approx(fx.vertical_boost, 0.068), "slow grip boost");
        assert_eq!(s.charge, cfg.max_charge - 1, "charge drains while clinging slowly");

        // Moving fast against the wall resets the charge.
        let fast = TickSituation { speed: 0.9, ..slow_cling };
        let fx2 = tick(&mut s, &cfg, &fast);
        assert!(approx(fx2.vertical_boost, 0.015), "fast grip boost");
        assert_eq!(s.charge, cfg.max_charge, "charge resets when moving fast");
    }

    #[test]
    fn tick_afterboost_after_wall_leap() {
        let cfg = ParkourConfig::default();
        let mut s = ready_state(&cfg);
        s.wall_leap = 3; // armed
        let airborne_open = TickSituation {
            selected: true, airborne: true, feet_solid: false,
            facing_wall: false, wall_count: 0, speed: 0.5,
        };
        let fx = tick(&mut s, &cfg, &airborne_open);
        assert!(approx(fx.vertical_boost, 0.024), "afterboost lift");
        assert_eq!(s.wall_leap, 2, "afterboost window drains once per tick");
    }
}
