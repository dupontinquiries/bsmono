//! style — the play analyzer. It watches the same event stream the game
//! already produces (touches, flips, resets, pinches, airtime, landings)
//! and decides what just happened was worth NAMING.
//!
//! The rule it works by: a trick is a RELATIONSHIP between things, not a
//! single flag. A flip is nothing; a flip that launches the ball you were
//! carrying, while airborne, off a wall, is a tornado flick. So each award
//! is a small predicate over a short window of history, and the analyzer
//! keeps only what it needs to evaluate them.

use glam::Vec3;

/// One thing the analyzer noticed this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Award {
    pub name: &'static str,
    pub points: u32,
}

/// What the game tells the analyzer each step.
#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub dt: f32,
    pub car_pos: Vec3,
    pub car_vel: Vec3,
    pub car_up: Vec3,
    pub on_ground: bool,
    pub flipping: bool,
    /// the car's measured spin, which is what actually throws the ball
    pub omega: f32,
    pub ball_pos: Vec3,
    pub ball_vel: Vec3,
    /// contact this frame, with the closing speed
    pub touch: Option<f32>,
    pub flip_reset: bool,
    pub pinch: bool,
    pub goal: bool,
    /// nearest surface normal under the car (wall/ceiling detection)
    pub surf_up: f32,
    pub supersonic: bool,
}

const COMBO_WINDOW: f32 = 3.0;

pub struct Analyzer {
    // rolling history
    air_t: f32,
    air_touches: u32,
    carry_t: f32,
    since_touch: f32,
    since_flip: f32,
    last_ball_speed: f32,
    ball_speed_at_touch: f32,
    car_speed_at_touch: f32,
    touch_was_airborne: bool,
    touch_was_flipping: bool,
    touch_omega: f32,
    touch_height: f32,
    wall_recent: f32,
    ceiling_recent: f32,
    reset_recent: f32,
    launch_pending: f32,
    /// combo state
    combo: u32,
    combo_t: f32,
    pub total: u32,
    pub best_combo: u32,
    /// awards raised this frame (drained by the caller)
    pub fresh: Vec<Award>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Analyzer::new()
    }
}

impl Analyzer {
    pub fn new() -> Analyzer {
        Analyzer {
            air_t: 0.0,
            air_touches: 0,
            carry_t: 0.0,
            since_touch: 9.0,
            since_flip: 9.0,
            last_ball_speed: 0.0,
            ball_speed_at_touch: 0.0,
            car_speed_at_touch: 0.0,
            touch_was_airborne: false,
            touch_was_flipping: false,
            touch_omega: 0.0,
            touch_height: 0.0,
            wall_recent: 9.0,
            ceiling_recent: 9.0,
            reset_recent: 9.0,
            launch_pending: 0.0,
            combo: 0,
            combo_t: 0.0,
            total: 0,
            best_combo: 0,
            fresh: Vec::new(),
        }
    }

    fn award(&mut self, name: &'static str, base: u32) {
        // combo multiplier: tricks chained inside the window compound
        self.combo = (self.combo + 1).min(9);
        self.combo_t = COMBO_WINDOW;
        self.best_combo = self.best_combo.max(self.combo);
        let points = base * self.combo;
        self.total += points;
        self.fresh.push(Award { name, points });
    }

    pub fn combo(&self) -> u32 {
        self.combo
    }

    /// Feed one simulation step. Returns the number of awards raised.
    pub fn step(&mut self, f: &Frame) -> usize {
        let before = self.fresh.len();
        let dt = f.dt.max(1e-5);
        self.since_touch += dt;
        self.since_flip += dt;
        self.wall_recent += dt;
        self.ceiling_recent += dt;
        self.reset_recent += dt;
        if self.combo_t > 0.0 {
            self.combo_t -= dt;
            if self.combo_t <= 0.0 {
                self.combo = 0;
            }
        }

        if f.on_ground {
            // surface memory: what were we driving on?
            if f.surf_up < 0.45 {
                self.wall_recent = 0.0;
            }
            if f.surf_up < -0.5 {
                self.ceiling_recent = 0.0;
            }
            self.air_t = 0.0;
            self.air_touches = 0;
        } else {
            self.air_t += dt;
        }
        if f.flipping {
            self.since_flip = 0.0;
        }
        if f.flip_reset {
            self.reset_recent = 0.0;
            self.award("FLIP RESET", 60);
        }
        if f.pinch {
            self.award("PINCH", 45);
        }

        // --- carrying: the ball riding on the car, near its speed --------
        let rel = f.ball_pos - f.car_pos;
        let riding = rel.dot(f.car_up) > 0.55
            && rel.length() < 2.0
            && (f.ball_vel - f.car_vel).length() < 6.0;
        if riding {
            self.carry_t += dt;
            if !f.on_ground {
                // air dribble milestones, once each as it grows
                if (self.carry_t - dt < 1.0 && self.carry_t >= 1.0) || (self.carry_t - dt < 2.5 && self.carry_t >= 2.5) {
                    self.award("AIR DRIBBLE", 70);
                }
            } else if self.carry_t - dt < 1.6 && self.carry_t >= 1.6 {
                self.award("DRIBBLE", 25);
            }
        } else if self.carry_t > 0.0 {
            // the ball left the car: was it thrown, and how?
            if self.carry_t > 0.45 {
                self.launch_pending = 0.18; // judge it once the ball is clear
            }
            self.carry_t = 0.0;
        }

        // --- the launch verdict, a beat after separation -----------------
        if self.launch_pending > 0.0 {
            self.launch_pending -= dt;
            if self.launch_pending <= 0.0 {
                let sp = f.ball_vel.length();
                // judge the launch against the BALL's own speed while it was
                // being carried, not the car's: a backflip slows the car, so
                // car-relative gain called the weakest flick the strongest
                let gain = sp / self.ball_speed_at_touch.max(1.0);
                let up = f.ball_vel.y;
                let flat = Vec3::new(f.ball_vel.x, 0.0, f.ball_vel.z).length();
                let elev = up.atan2(flat.max(0.01)).to_degrees();
                // a flip that begins just AFTER the last touch still threw
                // the ball — the contact is what ends as the hood swings
                let flip_driven = (self.touch_was_flipping && self.touch_omega > 3.0) || self.since_flip < 0.45;
                if flip_driven {
                    if self.touch_was_airborne && self.ceiling_recent < 2.5 {
                        self.award("CEILING FLICK", 150);
                    } else if up > 4.5 && elev > 25.0 {
                        // steep and lofted: the musty shape
                        self.award("MUSTY FLICK", 120);
                    } else if gain > 1.7 {
                        self.award("POWER FLICK", 100);
                    } else if gain > 1.25 {
                        self.award("FLICK", 60);
                    }
                } else if self.touch_was_airborne && sp > 12.0 {
                    self.award("AIR DRIBBLE SHOT", 90);
                }
            }
        }

        // --- contact bookkeeping ----------------------------------------
        if let Some(closing) = f.touch {
            self.since_touch = 0.0;
            self.ball_speed_at_touch = self.last_ball_speed;
            self.car_speed_at_touch = f.car_vel.length();
            self.touch_was_airborne = !f.on_ground;
            self.touch_was_flipping = f.flipping || self.since_flip < 0.25;
            self.touch_omega = f.omega;
            self.touch_height = f.ball_pos.y;
            if !f.on_ground {
                self.air_touches += 1;
                if self.air_touches == 3 {
                    self.award("AIR CONTROL", 55);
                }
            }
            if self.wall_recent < 1.2 && closing > 6.0 {
                self.award("WALL SHOT", 80);
            }
            if f.supersonic && closing > 14.0 {
                self.award("SUPERSONIC HIT", 70);
            }
            if self.reset_recent < 1.5 && closing > 8.0 {
                self.award("RESET SHOT", 130);
            }
            if self.air_t > 1.8 && closing > 8.0 && self.touch_height > 8.0 {
                self.award("AERIAL", 65);
            }
        }
        if f.goal {
            let base = if self.combo > 1 { 120 } else { 80 };
            self.award("GOAL", base);
        }
        self.last_ball_speed = f.ball_vel.length();
        self.fresh.len() - before
    }

    /// A one-line summary for a session or a sim run.
    pub fn summary(&self) -> String {
        format!("STYLE {}  BEST COMBO x{}", self.total, self.best_combo.max(1))
    }
}
