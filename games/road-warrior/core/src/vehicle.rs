//! Vehicle physics model: side-view combat racer.
//! Fixed-timestep, deterministic, no rendering/input dependencies here.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CarKind {
    /// Heavy, high-armor, slow to turn in air — the "Behemoth" archetype.
    Behemoth,
    /// Six-wheeled, low center of mass, best flip control — stunt specialist.
    SixWheeler,
    /// Light sedan, highest top speed, poor rotational inertia (hard to flip cleanly).
    RedSedan,
    /// Balanced all-rounder, mid stats across the board.
    Pillager,
}

#[derive(Debug, Clone, Copy)]
pub struct VehicleStats {
    pub top_speed: f32,       // m/s
    pub accel: f32,           // m/s^2
    pub mass: f32,            // kg, affects collision knockback + gas burn
    pub armor: f32,           // hit points
    /// Torque applied per unit of gyro tilt input while airborne. Higher = easier flips.
    pub flip_torque: f32,
    /// Angular drag while airborne — how quickly uncontrolled spin bleeds off.
    pub air_drag: f32,
    pub gas_capacity: f32,
    pub gas_burn_rate: f32,   // per second at full throttle
}

impl CarKind {
    /// All car variants, for UI enumeration (garage/select screen) and for
    /// tests that check every car is actually reachable through the campaign.
    pub const ALL: [CarKind; 4] = [CarKind::Behemoth, CarKind::SixWheeler, CarKind::RedSedan, CarKind::Pillager];

    pub fn stats(self) -> VehicleStats {
        match self {
            CarKind::Behemoth => VehicleStats {
                top_speed: 34.0,
                accel: 6.0,
                mass: 2400.0,
                armor: 180.0,
                flip_torque: 2.2,   // heavy, sluggish to rotate in air
                air_drag: 0.9,
                gas_capacity: 120.0,
                gas_burn_rate: 9.0,
            },
            CarKind::SixWheeler => VehicleStats {
                top_speed: 30.0,
                accel: 7.5,
                mass: 1800.0,
                armor: 120.0,
                flip_torque: 5.5,   // best in class — built for flips
                air_drag: 0.5,
                gas_capacity: 100.0,
                gas_burn_rate: 7.0,
            },
            CarKind::RedSedan => VehicleStats {
                top_speed: 44.0,
                accel: 9.5,
                mass: 1200.0,
                armor: 70.0,
                flip_torque: 1.4,   // fast but rotationally unforgiving
                air_drag: 1.4,
                gas_capacity: 80.0,
                gas_burn_rate: 6.0,
            },
            CarKind::Pillager => VehicleStats {
                top_speed: 38.0,
                accel: 7.8,
                mass: 1700.0,
                armor: 130.0,
                flip_torque: 3.2,
                air_drag: 0.8,
                gas_capacity: 100.0,
                gas_burn_rate: 7.5,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GroundContact {
    pub height: f32,
    pub slope: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Vehicle {
    pub kind: CarKind,
    pub stats: VehicleStats,
    pub pos_x: f32,
    pub pos_y: f32,
    pub vel_x: f32,
    pub vel_y: f32,
    pub rotation: f32,      // radians, 0 = wheels-down
    pub angular_vel: f32,
    pub airborne: bool,
    pub gas: f32,
    pub health: f32,
    pub nitro: f32,         // 0..100, filled by clean flips
}

impl Vehicle {
    pub fn new(kind: CarKind) -> Self {
        let stats = kind.stats();
        Self {
            kind,
            stats,
            pos_x: 0.0,
            pos_y: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            rotation: 0.0,
            angular_vel: 0.0,
            airborne: false,
            gas: stats.gas_capacity,
            health: stats.armor,
            nitro: 0.0,
        }
    }

    /// Advance physics by `dt` seconds, on flat ground at height 0. Convenience
    /// wrapper over `step_on_ground` for tests and any non-track use of the model.
    /// Out of fuel, stopped, and on the ground — the run cannot continue.
    ///
    /// Nothing else notices this. `respawn` only fires on `health <= 0`, and
    /// an empty tank leaves the vehicle alive and motionless: with
    /// `effective_throttle` gated on `gas > 0`, the car coasts to a halt and
    /// stays there. Observed in a headless run of level one — the vehicle
    /// ended at x = -17, velocity zero, health 75, and simply sat there for
    /// the remaining eighty seconds of simulation. The level cannot
    /// complete, the player cannot die, and nothing refuels. A softlock.
    ///
    /// Grounded is part of the test because mid-air with an empty tank is a
    /// normal jump you are still going to land; only a stationary car on the
    /// ground is genuinely stuck.
    pub fn is_stranded(&self) -> bool {
        self.gas <= 0.0 && !self.airborne && self.vel_x.abs() < 0.1
    }

    pub fn step(&mut self, dt: f32, throttle: f32, tilt: f32, nitro_active: bool, gravity: f32) {
        self.step_on_ground(dt, throttle, tilt, nitro_active, gravity, Some(GroundContact { height: 0.0, slope: 0.0 }));
    }

    /// Advance physics by `dt` seconds against arbitrary terrain.
    /// `ground` is the caller's ground query at the vehicle's *current* pos_x
    /// (e.g. `Some(GroundContact { height: track.height_at(x)?, slope: track.slope_at(x) })`,
    /// or `None` over a gap). Vehicle doesn't know about `Track` directly —
    /// height/slope is the whole contract, so any terrain source can drive it.
    ///
    /// `throttle` in [-1, 1] (reverse..forward), `tilt` is gyro input in [-1, 1]
    /// (device roll, used to control rotation while airborne, exactly like the
    /// original's flip mechanic), `nitro_active` burns banked nitro for a speed boost.
    pub fn step_on_ground(
        &mut self,
        dt: f32,
        throttle: f32,
        tilt: f32,
        nitro_active: bool,
        gravity: f32,
        ground: Option<GroundContact>,
    ) {
        let effective_throttle = if self.gas > 0.0 { throttle } else { 0.0 };
        self.gas = (self.gas - effective_throttle.abs() * self.stats.gas_burn_rate * dt).max(0.0);

        if self.airborne {
            // Gyro tilt drives angular acceleration; air_drag bleeds off spin
            // that isn't actively being steered, so uncontrolled flips settle
            // instead of spinning forever.
            let control_torque = tilt * self.stats.flip_torque;
            self.angular_vel += control_torque * dt;
            self.angular_vel *= (1.0 - self.stats.air_drag * dt).max(0.0);
            self.rotation += self.angular_vel * dt;

            self.vel_y += gravity * dt;
            self.pos_y += self.vel_y * dt;

            // Land only where there's ground to land on; over a Gap (`None`),
            // keep falling even past height 0 — that's the pit hazard.
            if let Some(g) = ground {
                if self.pos_y <= g.height {
                    self.land(g.height, g.slope);
                }
            }
        } else {
            let target_speed = effective_throttle * self.stats.top_speed;
            let accel = self.stats.accel * dt;
            if self.vel_x < target_speed {
                self.vel_x = (self.vel_x + accel).min(target_speed);
            } else {
                self.vel_x = (self.vel_x - accel).max(target_speed);
            }

            if nitro_active && self.nitro > 0.0 {
                self.vel_x += self.stats.top_speed * 0.6 * dt;
                self.nitro = (self.nitro - 40.0 * dt).max(0.0);
            }

            match ground {
                // Grounded: stick to the terrain surface and bank its slope
                // into rotation, so the car sits flush on ramps instead of
                // clipping through them.
                Some(g) => {
                    self.pos_y = g.height;
                    self.rotation = g.slope.atan();
                    self.vel_y = 0.0;
                }
                // Drove off the edge of a ramp into a Gap: become airborne
                // from wherever we currently are, carrying current momentum.
                None => {
                    self.airborne = true;
                }
            }
        }

        self.pos_x += self.vel_x * dt;
    }

    pub fn launch(&mut self, vertical_speed: f32) {
        self.airborne = true;
        self.vel_y = vertical_speed;
    }

    fn land(&mut self, ground_height: f32, slope: f32) {
        self.pos_y = ground_height;
        self.airborne = false;
        self.vel_y = 0.0;

        // Reward a clean landing (rotation matching the slope, not just flat)
        // with nitro, same as the original's flip-for-boost loop.
        let upright_target = slope.atan();
        let normalized = (self.rotation - upright_target).rem_euclid(std::f32::consts::TAU);
        let upright_error = normalized.min(std::f32::consts::TAU - normalized);
        let clean = upright_error < 0.35; // ~20 degrees tolerance
        if clean {
            self.nitro = (self.nitro + 25.0).min(100.0);
        } else {
            // Rough landing: knock some armor off, proportional to how far
            // from upright, scaled by mass (heavier cars punish harder hits less).
            let penalty = (upright_error / std::f32::consts::PI) * 15.0 * (1500.0 / self.stats.mass);
            self.health = (self.health - penalty).max(0.0);
        }
        self.rotation = upright_target;
        self.angular_vel = 0.0;
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn an_empty_tank_at_a_standstill_is_stranded() {
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.gas = 0.0;
        v.vel_x = 0.0;
        assert!(v.is_stranded());
    }

    #[test]
    fn still_rolling_is_not_stranded_yet() {
        // Coasting to a halt is normal; the run is only over once it stops.
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.gas = 0.0;
        v.vel_x = 5.0;
        assert!(!v.is_stranded());
    }

    #[test]
    fn mid_air_with_an_empty_tank_is_not_stranded() {
        // You are still going to land. Failing the run here would end it in
        // the middle of a jump the player has already committed to.
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.gas = 0.0;
        v.vel_x = 0.0;
        v.airborne = true;
        assert!(!v.is_stranded());
    }

    #[test]
    fn a_full_tank_at_a_standstill_is_not_stranded() {
        // Sitting at the start line before touching the throttle.
        let v = Vehicle::new(CarKind::RedSedan);
        assert!(!v.is_stranded());
    }
    use super::*;

    #[test]
    fn sedan_out_accelerates_behemoth_on_ground() {
        let mut sedan = Vehicle::new(CarKind::RedSedan);
        let mut behemoth = Vehicle::new(CarKind::Behemoth);
        for _ in 0..120 {
            sedan.step(1.0 / 60.0, 1.0, 0.0, false, -9.8);
            behemoth.step(1.0 / 60.0, 1.0, 0.0, false, -9.8);
        }
        assert!(sedan.pos_x > behemoth.pos_x, "sedan should be ahead after 2s");
    }

    #[test]
    fn six_wheeler_rotates_faster_than_sedan_for_same_tilt() {
        let mut six = Vehicle::new(CarKind::SixWheeler);
        let mut sedan = Vehicle::new(CarKind::RedSedan);
        six.airborne = true;
        six.pos_y = 5.0; // clear of ground so the landing check doesn't fire mid-test
        sedan.airborne = true;
        sedan.pos_y = 5.0;
        for _ in 0..30 {
            six.step(1.0 / 60.0, 0.0, 1.0, false, 0.0);
            sedan.step(1.0 / 60.0, 0.0, 1.0, false, 0.0);
        }
        assert!(
            six.rotation.abs() > sedan.rotation.abs(),
            "six-wheeler should flip faster than sedan under identical tilt input"
        );
    }

    #[test]
    fn clean_landing_grants_nitro_rough_landing_costs_health() {
        let mut v = Vehicle::new(CarKind::Pillager);
        v.airborne = true;
        v.pos_y = 0.02;
        v.vel_y = -5.0; // fast enough to cross pos_y <= 0 within one step
        v.rotation = 0.0; // upright
        v.step(1.0 / 60.0, 0.0, 0.0, false, -9.8);
        assert!(v.nitro > 0.0, "clean landing should grant nitro");

        let mut crashed = Vehicle::new(CarKind::Pillager);
        crashed.airborne = true;
        crashed.pos_y = 0.02;
        crashed.vel_y = -5.0;
        crashed.rotation = std::f32::consts::PI; // upside down
        let starting_health = crashed.health;
        crashed.step(1.0 / 60.0, 0.0, 0.0, false, -9.8);
        assert!(crashed.health < starting_health, "rough landing should cost health");
    }

    #[test]
    fn gas_depletes_and_halts_throttle_response() {
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.gas = 0.01;
        v.step(1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert_eq!(v.gas, 0.0, "gas should clamp to zero, not go negative");
        let speed_before = v.vel_x;
        v.step(1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(v.vel_x <= speed_before, "empty tank should stop further acceleration");
    }
}
