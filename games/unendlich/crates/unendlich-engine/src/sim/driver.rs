//! Phase 0 — the driver that connects wall-clock time and raw input to the
//! deterministic [`crate::sim`] core, and produces an interpolated render
//! pose for the camera/renderer.
//!
//! # Why this module exists
//!
//! The sim ([`crate::sim::state::step`] + [`crate::sim::rollback::Rollback`])
//! is a pure, fixed-tick (60 Hz), integer-only world. The display runs at an
//! arbitrary refresh rate (60–500 Hz). This driver is the *only* glue between
//! them:
//!
//! 1. It accumulates real frame time and advances the sim by **whole** ticks
//!    (never a partial tick — that would break determinism).
//! 2. It exposes a [`RenderPose`] per character that **interpolates** between
//!    the two most recent sim ticks by the leftover fraction (`alpha`), so
//!    motion is smooth even though the sim only moves 60 times a second. That
//!    interpolation is, by construction, up to one tick (~16.7 ms) "behind"
//!    the latest simulated state — the buffer that later hides network jitter.
//!
//! The driver writes floats and reads the sim; it **never** feeds floats back
//! in. The sim stays bit-deterministic.
//!
//! # Single-player now, server-shaped for later
//!
//! Today [`SimDriver::advance`] is *both* the authoritative tick advance and
//! the local prediction — in single-player they are identical. The seams for
//! a future listen-server split are marked `SERVER-SPLIT:` in the code:
//!
//! - **Local player:** keep predicting immediately (what `advance` does), and
//!   on receiving an authoritative snapshot, reset the baseline and replay the
//!   unacked local inputs (the rollback core already resims from a baseline).
//! - **Remote players & mobs:** do *not* predict their logic — render them
//!   from interpolated authoritative snapshots ([`SimDriver::render_pose`] is
//!   exactly this), which is why they look real-time while staying server
//!   truth.
//! - **Hand-picked optimistic events** (e.g. a block place that is almost
//!   always accepted): would enter a small `predicted_events` queue applied on
//!   the client and reconciled against the server ack. Out of scope until the
//!   server exists; the hook is noted at `SERVER-SPLIT: optimistic events`.

use crate::sim::fixed::Fixed;
use crate::sim::physics::WorldQuery;
use crate::sim::rollback::{PlayerId, Rollback};
use crate::sim::state::{Character, Input, SimState, TICK_HZ};

/// Seconds per simulation tick.
pub const TICK_DT: f64 = 1.0 / TICK_HZ as f64;

/// Default ceiling on how many ticks one `advance` call may run, so a long
/// stall (debugger pause, alt-tab) can't trigger a "spiral of death" where the
/// driver tries to simulate seconds of catch-up in one frame.
pub const DEFAULT_MAX_CATCHUP_TICKS: u32 = 8;

/// A presentation-only pose for one character: floats, plus a `pitch` that the
/// simulation never stores (look-up/down is camera-only and not rolled back).
/// Produced by interpolating between the two most recent sim ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderPose {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Facing angle in radians (shortest-arc interpolated from the sim's `u16`).
    pub yaw: f32,
    pub on_ground: bool,
}

/// Raw per-frame input from the presentation layer, *before* quantization into
/// the deterministic [`Input`]. `move_x`/`move_z` are expected in `[-1, 1]`
/// (already resolved relative to facing by the caller, or world-space — the
/// sim only sees the components). `yaw_radians` comes from the camera; pitch is
/// handled by the renderer and never passed here.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawInput {
    pub move_x: f32,
    pub move_z: f32,
    pub yaw_radians: f32,
    pub buttons: u32,
}

/// Drives the deterministic sim from real time and produces interpolated poses.
pub struct SimDriver {
    rollback: Rollback,
    local: PlayerId,
    /// Unconsumed wall-clock seconds, always `< TICK_DT` after `advance`.
    accum: f64,
    /// Fractional progress toward the next tick, in `[0, 1)` — the interpolation
    /// weight between `prev` and `cur`.
    alpha: f32,
    /// State one tick behind `cur`; the `from` end of interpolation.
    prev: SimState,
    /// Latest stepped state; the `to` end of interpolation.
    cur: SimState,
    max_catchup_ticks: u32,
}

impl SimDriver {
    /// Build a driver. `initial` is the tick-0 state (e.g. `SimState::new(n)`
    /// with the player(s) placed); `max_prediction` is forwarded to the
    /// rollback engine.
    pub fn new(
        num_players: usize,
        local: PlayerId,
        initial: SimState,
        max_prediction: u64,
    ) -> SimDriver {
        let rollback = Rollback::new(num_players, initial.clone(), max_prediction);
        SimDriver {
            rollback,
            local,
            accum: 0.0,
            alpha: 0.0,
            prev: initial.clone(),
            cur: initial,
            max_catchup_ticks: DEFAULT_MAX_CATCHUP_TICKS,
        }
    }

    /// Override the catch-up ceiling (default [`DEFAULT_MAX_CATCHUP_TICKS`]).
    pub fn with_max_catchup(mut self, ticks: u32) -> SimDriver {
        self.max_catchup_ticks = ticks.max(1);
        self
    }

    /// Advance by however many whole 60 Hz ticks `real_dt` seconds covers,
    /// feeding `raw` as the local player's input for each tick consumed this
    /// frame, then update the interpolation `alpha`. Returns the number of sim
    /// ticks actually stepped (0 on a fast frame between ticks).
    ///
    /// SERVER-SPLIT: on a client this is the *local prediction* path. A server
    /// would instead step one authoritative tick per network tick with every
    /// player's input; a client would also reconcile here when an authoritative
    /// snapshot arrives (reset baseline + replay unacked local inputs).
    pub fn advance<W: WorldQuery>(&mut self, real_dt: f64, raw: RawInput, world: &W) -> u32 {
        self.accum += real_dt.max(0.0);
        // Spiral-of-death clamp: never let a single frame owe more than the
        // catch-up ceiling, so a long stall is absorbed (time is dropped) rather
        // than simulated all at once.
        let cap = self.max_catchup_ticks as f64 * TICK_DT;
        if self.accum > cap {
            self.accum = cap;
        }

        let input = self.map_input(raw);
        let mut stepped = 0u32;
        while self.accum >= TICK_DT && stepped < self.max_catchup_ticks {
            // Snapshot the pre-step state so we can interpolate toward the new one.
            self.prev = self.cur.clone();
            self.rollback.set_local_input(self.local, input, world);
            self.rollback.advance(world);
            self.cur = self.rollback.state().clone();
            self.accum -= TICK_DT;
            stepped += 1;
        }
        // Leftover fraction → interpolation weight. In f64 this is always in
        // [0, 1) because the loop consumed every whole tick — BUT the f64→f32
        // cast can round a value like 0.99999×TICK_DT up to exactly 1.0 (f32
        // has ~7 significant digits), which breaks the documented `alpha < 1.0`
        // invariant and the interpolation contract. Clamp just below 1.0.
        let a = (self.accum / TICK_DT) as f32;
        self.alpha = if a >= 1.0 { 1.0 - f32::EPSILON } else { a.max(0.0) };
        stepped
    }

    /// Interpolated render pose for `player`, blending the two most recent sim
    /// ticks by `alpha`. This is what the camera/renderer should read for
    /// smooth motion.
    ///
    /// SERVER-SPLIT: for the *local* player a client may prefer
    /// [`SimDriver::latest_pose`] (zero added delay, since local input is
    /// predicted), reserving interpolation for remote players/mobs.
    pub fn render_pose(&self, player: PlayerId) -> RenderPose {
        let a = self.prev.players.get(player).copied().unwrap_or_default();
        let b = self.cur.players.get(player).copied().unwrap_or_default();
        Self::interpolate(&a, &b, self.alpha)
    }

    /// The latest simulated pose for `player`, with no interpolation. Use for
    /// the local player when zero presentation delay is wanted.
    pub fn latest_pose(&self, player: PlayerId) -> RenderPose {
        let b = self.cur.players.get(player).copied().unwrap_or_default();
        Self::interpolate(&b, &b, 0.0)
    }

    fn interpolate(a: &Character, b: &Character, t: f32) -> RenderPose {
        RenderPose {
            x: lerp(a.pos.x.to_f32(), b.pos.x.to_f32(), t),
            y: lerp(a.pos.y.to_f32(), b.pos.y.to_f32(), t),
            z: lerp(a.pos.z.to_f32(), b.pos.z.to_f32(), t),
            yaw: lerp_yaw(a.yaw, b.yaw, t),
            // `on_ground` is a boolean — take the target tick's value rather
            // than blend it.
            on_ground: b.on_ground,
        }
    }

    /// Quantize raw float input into the deterministic [`Input`]. This is the
    /// presentation→sim boundary; floats stop here.
    fn map_input(&self, raw: RawInput) -> Input {
        let clamp = |v: f32| v.clamp(-1.0, 1.0);
        Input {
            buttons: raw.buttons,
            move_x: Fixed::from_f64(clamp(raw.move_x) as f64),
            move_z: Fixed::from_f64(clamp(raw.move_z) as f64),
            yaw: yaw_to_u16(raw.yaw_radians),
        }
    }

    /// Number of mobs in the latest sim state (for iterating [`SimDriver::mob_pose`]).
    pub fn mob_count(&self) -> usize {
        self.cur.mobs.len()
    }

    /// Interpolated render pose for mob `i`, mirroring [`SimDriver::render_pose`]
    /// for players. Newly-spawned or despawning mobs (present in only one of the
    /// two snapshots) render at their single known pose without interpolation.
    pub fn mob_pose(&self, i: usize) -> RenderPose {
        let a = self.prev.mobs.get(i).map(|m| m.body);
        let b = self.cur.mobs.get(i).map(|m| m.body);
        match (a, b) {
            (Some(a), Some(b)) => Self::interpolate(&a, &b, self.alpha),
            (Some(only), None) | (None, Some(only)) => Self::interpolate(&only, &only, 0.0),
            (None, None) => RenderPose::default(),
        }
    }

    // ── passthroughs for debug / telemetry ─────────────────────────────────
    pub fn local(&self) -> PlayerId { self.local }
    pub fn alpha(&self) -> f32 { self.alpha }
    pub fn current_tick(&self) -> u64 { self.rollback.current_tick() }
    pub fn confirmed_tick(&self) -> u64 { self.rollback.confirmed_tick() }
    pub fn checksum(&self) -> u64 { self.rollback.checksum() }
    /// Direct read of the latest simulated state (no interpolation).
    pub fn state(&self) -> &SimState { self.rollback.state() }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

/// Shortest-arc interpolation between two `u16` yaw angles, returning radians.
/// `u16` wraps the circle (`0..=65535` → `0..2π`), so we take the signed
/// 16-bit difference to always rotate the short way (e.g. 350°→10° passes
/// through 0°, not back through 180°).
#[inline]
fn lerp_yaw(a: u16, b: u16, t: f32) -> f32 {
    let delta = b.wrapping_sub(a) as i16 as f32; // shortest signed arc, in u16 units
    let interp = a as f32 + delta * t;
    (interp / 65536.0) * std::f32::consts::TAU
}

/// Quantize a yaw in radians (any value) to the sim's `u16` representation,
/// wrapping negatives via Euclidean remainder.
#[inline]
fn yaw_to_u16(radians: f32) -> u16 {
    let frac = radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    (frac * 65536.0) as u32 as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::FlatWorld;
    use crate::sim::state::SimState;

    fn driver() -> SimDriver {
        SimDriver::new(1, 0, SimState::new(1), 8)
    }

    /// One second of frames at any refresh rate advances ~60 sim ticks, and
    /// `alpha` always stays in `[0, 1)`.
    #[test]
    fn advances_sixty_ticks_per_second_regardless_of_refresh() {
        let world = FlatWorld::default();
        for &hz in &[30u32, 60, 144, 165, 500] {
            let mut d = driver();
            for _ in 0..hz {
                d.advance(1.0 / hz as f64, RawInput::default(), &world);
                assert!(d.alpha() >= 0.0 && d.alpha() < 1.0, "alpha out of range @ {hz}Hz");
            }
            let ticks = d.current_tick();
            assert!((ticks as i64 - 60).abs() <= 1, "{hz}Hz -> {ticks} ticks (want ~60)");
        }
    }

    /// A long stall is clamped to the catch-up ceiling — no spiral of death.
    #[test]
    fn long_stall_is_clamped() {
        let world = FlatWorld::default();
        let mut d = driver();
        let stepped = d.advance(2.0, RawInput::default(), &world);
        assert_eq!(stepped, DEFAULT_MAX_CATCHUP_TICKS, "stall should clamp to ceiling");
        assert_eq!(d.current_tick(), DEFAULT_MAX_CATCHUP_TICKS as u64);
    }

    fn deg(d: f32) -> u16 {
        ((d / 360.0 * 65536.0).round() as i64 & 0xFFFF) as u16
    }
    fn to_deg(rad: f32) -> f32 {
        rad.to_degrees().rem_euclid(360.0)
    }

    /// Yaw interpolation takes the short way around the wrap point, both ways.
    #[test]
    fn yaw_interpolation_is_shortest_arc() {
        // 350° -> 10° should land at ~0°/360° at the midpoint, not 180°.
        let m = to_deg(lerp_yaw(deg(350.0), deg(10.0), 0.5));
        assert!((((m + 180.0) % 360.0) - 180.0).abs() < 1.5, "350->10 mid {m}");
        // and the other direction
        let m2 = to_deg(lerp_yaw(deg(10.0), deg(350.0), 0.5));
        assert!((((m2 + 180.0) % 360.0) - 180.0).abs() < 1.5, "10->350 mid {m2}");
        // interior case is just linear
        let m3 = to_deg(lerp_yaw(deg(90.0), deg(100.0), 0.5));
        assert!((m3 - 95.0).abs() < 1.0, "90->100 mid {m3}");
    }

    /// Negative / over-2π yaw quantizes correctly (Euclidean wrap).
    #[test]
    fn yaw_quantization_wraps() {
        let half = yaw_to_u16(-std::f32::consts::PI); // -180° → 180°
        assert!((to_deg(half as f32 / 65536.0 * std::f32::consts::TAU) - 180.0).abs() < 1.0);
        assert_eq!(yaw_to_u16(0.0), 0);
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// Wires the deterministic SimDriver (real-time → fixed-tick + interp).
// Neutral: a 1-player tick-0 state, an empty (all-air) world, and zero
// RawInput each frame — advance() steps the sim deterministically and
// proves the wiring is panic-free.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use crate::sim::physics::BlockPhysics; // WorldQuery already imported at top of file
use linkme::distributed_slice;

/// Empty world: every voxel is air. Enough to tick the driver neutrally.
struct NeutralWorld;
impl WorldQuery for NeutralWorld {
    fn physics_at(&self, _x: i32, _y: i32, _z: i32) -> BlockPhysics {
        BlockPhysics::AIR
    }
}

struct DriverService {
    driver: SimDriver,
    world: NeutralWorld,
}
impl Service for DriverService {
    fn name(&self) -> &'static str {
        "driver"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.driver.advance(ctx.dt as f64, RawInput::default(), &self.world);
    }
}
#[distributed_slice(SERVICES)]
static DRIVER_SERVICE: ServiceEntry = ServiceEntry {
    name: "driver",
    make: || {
        Box::new(DriverService {
            driver: SimDriver::new(1, 0, SimState::new(1), 8),
            world: NeutralWorld,
        })
    },
};
