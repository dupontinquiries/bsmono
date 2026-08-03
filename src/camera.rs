//! Chase camera, now gravity-aware. The camera keeps a smoothed local UP
//! (following world.gravity_dir under a rate cap), so inside a perplexus
//! tube the horizon rolls with you and "down" on screen is the tube wall
//! under your wheels. CAMERA -> GRAVITY CAMERA blends between classic Y-up
//! (0%) and full field-following (100%). The temporal heading that keeps
//! flips from whipping the car cam lives in the local-up plane now.

use glam::{Mat4, Quat, Vec3};

use crate::config::CamCfg;
use crate::physics::{Ball, Car};
use crate::world::World;

/// Which camera you are looking through.
///
/// This was a bool — ball cam on or off — and "off" meant a chase camera
/// aimed twelve metres ahead of the car. That framing works for a car,
/// which is low and wide and reads fine at the bottom of frame. It does not
/// work for a robot: the subject leaves the shot entirely and you are
/// looking at empty ground with your own feet somewhere below the bezel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CamMode {
    /// look at the ball / locked target
    Ball,
    /// proper third person: the body is the subject
    Chase,
    /// from the head — or, on a flyer, from the nose
    First,
    /// a defensive gun position: rear-facing, or ventral, depending on what
    /// the model actually carries
    Gunner,
}

/// Where a gunner sits. Which one a chassis offers is a property OF THE
/// MODEL — tools/find-mounts.py already reports whether a hull has rearward
/// thrusters to sight past or ventral hardpoints to shoot through — so this
/// is chosen from the chassis rather than cycled blindly. A hull with
/// neither simply has no gunner mode, and the camera skips it rather than
/// showing an empty seat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GunSeat {
    /// looking back down the tail
    Rear,
    /// looking down through the belly
    Ventral,
}

/// What the model provides.
#[derive(Clone, Copy, Debug, Default)]
pub struct GunMounts {
    pub rear: bool,
    pub ventral: bool,
}

impl GunMounts {
    /// The seat to use, preferring the rear gun: on a flyer being chased,
    /// the thing you want to see is behind you.
    pub fn seat(self) -> Option<GunSeat> {
        if self.rear {
            Some(GunSeat::Rear)
        } else if self.ventral {
            Some(GunSeat::Ventral)
        } else {
            None
        }
    }
    pub fn any(self) -> bool {
        self.rear || self.ventral
    }
}

impl CamMode {
    /// Step to the next mode, skipping the gunner seat on a chassis that
    /// does not have one. Cycling into a mode that cannot exist is how a
    /// camera key stops feeling reliable.
    pub fn next(self, guns: GunMounts) -> CamMode {
        match self {
            CamMode::Ball => CamMode::Chase,
            CamMode::Chase => CamMode::First,
            CamMode::First => {
                if guns.any() {
                    CamMode::Gunner
                } else {
                    CamMode::Ball
                }
            }
            CamMode::Gunner => CamMode::Ball,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CamMode::Ball => "BALL CAM",
            CamMode::Chase => "CHASE CAM",
            CamMode::First => "FIRST PERSON",
            CamMode::Gunner => "GUNNER",
        }
    }
}

pub struct Camera {
    /// smoothed velocity of the platform under the car (bend)
    plat_lag: Vec3,
    pub pos: Vec3,
    pub look: Vec3,
    pub up: Vec3, // smoothed local up used for the view basis
    pub ball_cam: bool,
    pub mode: CamMode,
    heading: Vec3,  // smoothed forward, kept in the up-plane
    reverse_t: f32, // how long the car has pointed "the other way"
    shake: f32,
    shake_t: f32,
    /// FREE LOOK, used in robot mode. Yaw turns the boom around the body,
    /// pitch raises and lowers it. Kept as an OFFSET on the chase camera
    /// rather than a camera of its own, so ball cam, the gravity-aware up
    /// and the terrain clamp all keep working underneath it.
    pub look_yaw: f32,
    pub look_pitch: f32,
    /// AUTOTARGET. Ball cam is a point-of-interest camera that happens to
    /// have been given the ball; hand it something else and the same rig
    /// becomes a combat camera. Set this and Ball mode follows it instead.
    pub poi: Option<Vec3>,
    /// true when the body is a flyer: first person sits at the nose
    pub nose_cam: bool,
    /// which gun seat this chassis offers, if any
    pub gun_seat: Option<GunSeat>,
    /// true while the POI is an opponent rather than the ball: the boom
    /// stops flattening itself into the horizontal plane, which is right for
    /// a ground game and wrong for a dogfight
    pub combat: bool,
}

impl Camera {
    pub fn new() -> Camera {
        Camera {
            plat_lag: Vec3::ZERO,
            pos: Vec3::new(0.0, 3.0, 34.0),
            look: Vec3::ZERO,
            up: Vec3::Y,
            ball_cam: true,
            mode: CamMode::Ball,
            heading: Vec3::NEG_Z,
            reverse_t: 0.0,
            shake: 0.0,
            shake_t: 0.0,
            look_yaw: 0.0,
            look_pitch: 0.0,
            poi: None,
            nose_cam: false,
            gun_seat: None,
            combat: false,
        }
    }

    /// Current shake, for the SHIVER rig (parts jolt when the car is hit).
    pub fn shake_amt(&self) -> f32 {
        self.shake.clamp(0.0, 1.0)
    }

    pub fn add_shake(&mut self, amt: f32) {
        self.shake = (self.shake + amt).min(1.2);
    }

    /// Rotate `v` toward `target` by at most `max_ang` radians.
    fn rotate_toward(v: Vec3, target: Vec3, max_ang: f32) -> Vec3 {
        let dot = v.dot(target).clamp(-1.0, 1.0);
        let ang = dot.acos();
        if ang < 1e-4 {
            return target;
        }
        let t = (max_ang / ang).min(1.0);
        v.lerp(target, t).normalize_or(target)
    }

    fn update_heading(&mut self, car: &Car, flipping: bool, dt: f32) {
        // project everything into the current up-plane
        self.heading = (self.heading - self.up * self.heading.dot(self.up)).normalize_or(Vec3::NEG_Z);
        if flipping {
            self.reverse_t = 0.0;
            return;
        }
        let f = car.forward();
        let fp = (f - self.up * f.dot(self.up)).normalize_or(self.heading);
        if (f - self.up * f.dot(self.up)).length_squared() < 0.05 {
            return; // nose along gravity: keep the old heading
        }
        let ang = self.heading.dot(fp).clamp(-1.0, 1.0).acos();
        if ang > 2.2 {
            self.reverse_t += dt;
            if self.reverse_t < 0.35 {
                return;
            }
        } else {
            self.reverse_t = 0.0;
        }
        let rate = (ang * 7.0).min(5.0);
        self.heading = Self::rotate_toward(self.heading, fp, rate * dt);
    }

    pub fn update(
        &mut self,
        car: &Car,
        ball: &Ball,
        world: &World,
        cfg: &CamCfg,
        flipping: bool,
        rear: bool,
        dt: f32,
    ) {
        // ---- gravity-aware up basis: smoothed, rate-capped, blendable ----
        let g = world.gravity_dir(car.pos);
        // The horizon leans toward local gravity, but not all the way and
        // not quickly: at full follow and 2.4 rad/s the world rolled out
        // from under you every time a tube's "down" swung round, which is
        // disorienting rather than dramatic. Two thirds of the way, at half
        // the rate, keeps the sense of being inside a pipe without the room
        // spinning.
        let follow = cfg.grav_follow * 0.68;
        let want_up = (Vec3::Y * (1.0 - follow) + (-g) * follow).normalize_or(Vec3::Y);
        self.up = Self::rotate_toward(self.up, want_up, 1.15 * dt);

        self.update_heading(car, flipping, dt);

        // FRAME THE SUBJECT.
        //
        // Chase used to aim `car.pos + heading * 12`, which puts the look
        // point twelve metres past the thing you are driving — fine for a
        // low wide car sitting at the bottom of frame, wrong for a robot,
        // which walks straight out of the shot.
        //
        // A third-person camera looks at the BODY with a little lead, not at
        // a patch of ground ahead of it. The lead shrinks and the look point
        // rises with the body's own height, so a tall subject is centred and
        // a flat one still shows the road.
        let eye_h = car.half_h * 1.35;
        let lead = if car.robot { 2.6 } else { 12.0 };
        let target = match self.mode {
            CamMode::Ball => self.poi.unwrap_or(ball.pos),
            CamMode::Chase | CamMode::First => {
                car.pos + self.up * eye_h + self.heading * lead
            }
            // a gunner looks where the gun points: back down the tail, or
            // down through the belly
            CamMode::Gunner => match self.gun_seat {
                Some(GunSeat::Ventral) => car.pos - self.up * 40.0,
                _ => car.pos - self.heading * 40.0 + self.up * eye_h * 0.5,
            },
        };
        let mut dir = target - car.pos;
        if dir.length_squared() < 1e-4 {
            dir = self.heading;
        }
        let mut dir = dir.normalize();
        if rear {
            dir = -dir;
        }
        // the BOOM lives in the up-plane: a high ball must not drag the
        // camera down into the terrain clamp (which was eating the dist,
        // height and angle settings whole — the cam sim caught boom_up
        // pinned at the floor regardless of cfg). Only the LOOK tilts up.
        // THE BOOM, and the one line that decides whether a dogfight works.
        //
        // Normally the boom lives in the up-plane: a high ball must not drag
        // the camera down into the terrain clamp. In air-to-air that is
        // exactly wrong — an opponent forty metres above you flattens to the
        // same bearing as one level with you, so the camera never gets under
        // you and you lose them behind your own body. In combat the boom
        // takes the full 3D bearing, so the eye swings below when they are
        // above and the pair stay in frame.
        let mut dir_flat = if self.combat && self.mode == CamMode::Ball {
            dir
        } else {
            (dir - self.up * dir.dot(self.up)).normalize_or(self.heading)
        };
        // free look rides on the flattened boom direction: yaw about the
        // local up (so it still works upside down in a tube), pitch folded
        // into the height below
        if self.look_yaw.abs() > 1e-4 {
            dir_flat = Quat::from_axis_angle(self.up, self.look_yaw) * dir_flat;
        }
        // MOVING-PLATFORM BEND: while you are near a fast deck, the camera
        // adopts a slice of its motion. The platform then reads as (nearly)
        // still and only your own approach moves — which is what makes a
        // small, rapidly moving target landable.
        // pitch trades boom distance for height, so looking up does not push
        // the eye through the body
        let (pc, ps) = (self.look_pitch.cos(), self.look_pitch.sin());
        // a close opponent needs a longer boom or the two of you do not fit
        // in the shot; a distant one does not, and pulling back for it would
        // just shrink them
        let close = if self.combat {
            let d = (target - car.pos).length();
            1.0 + (1.0 - (d / 30.0).clamp(0.0, 1.0)) * 0.55
        } else {
            1.0
        };
        let mut want = car.pos - dir_flat * (cfg.dist * pc.max(0.25) * close)
            + self.up * (cfg.height + cfg.dist * ps);
        if cfg.plat_bend > 0.01 {
            if let Some(pv) = world.platform_vel(car.pos, 6.0) {
                self.plat_lag += (pv - self.plat_lag) * (1.0 - (-3.0 * dt).exp());
            } else {
                self.plat_lag *= (-2.0 * dt).exp();
            }
            // lead the camera along the platform's travel, capped so a
            // spinner's rim speed cannot fling the view
            want += (self.plat_lag * cfg.plat_bend * 0.22).clamp_length_max(3.0);
        }
        // terrain clamp only makes sense while up is world-ish
        if self.up.y > 0.7 {
            let floor = world.ground_h(want.x, want.z) + 0.35;
            if want.y < floor {
                want.y = floor;
            }
        }
        let k = 1.0 - (-9.0 * cfg.stiffness * dt).exp();
        if self.mode == CamMode::Gunner {
            // A GUN POSITION IS NOT A CHASE CAMERA. It is bolted to the
            // hull, so it snaps like first person, and it looks where the
            // gun points rather than where the nose does.
            let f = self.heading;
            let up = self.up;
            self.pos = match self.gun_seat {
                Some(GunSeat::Ventral) => car.pos - up * (car.half_h * 0.9),
                _ => car.pos - f * (car.half_len * 0.85) + up * (car.half_h * 0.5),
            };
        } else if self.mode == CamMode::First {
            // ATTACHED TO THE HEAD. No boom, no smoothing on position — a
            // first-person eye that lags its own body reads as seasickness,
            // so this snaps and only the LOOK is smoothed.
            //
            // Nudged forward of the head rather than sat inside it: the
            // player's own model is still drawn, and an eye at the exact
            // head centre looks at the inside of its own face.
            // ON A FLYER THE EYE IS THE NOSE, not the head. A jet has no
            // head to sit behind, and putting the camera where a robot's
            // would be leaves it hanging above the fuselage looking at its
            // own spine. Out at the tip and level, it reads as a cockpit.
            let f = if self.nose_cam {
                self.heading
            } else {
                (self.heading - self.up * self.heading.dot(self.up)).normalize_or(self.heading)
            };
            self.pos = if self.nose_cam {
                car.pos + f * (car.half_len * 0.95) + self.up * (car.half_h * 0.35)
            } else {
                car.pos + self.up * (car.half_h * 1.55) + f * (car.half_len * 0.55 + 0.25)
            };
        } else {
            self.pos += (want - self.pos) * k;
        }

        let aim = if rear { car.pos - (target - car.pos) } else { target };
        // the boom's downward tilt is a third-person framing device; from
        // the head there is no boom, so there is nothing to tilt
        let tilt = if self.mode == CamMode::First {
            0.0
        } else {
            cfg.dist * (-cfg.angle_deg.to_radians()).tan()
        };
        let want_look = aim + self.up * (0.35 - tilt);
        self.look += (want_look - self.look) * (1.0 - (-14.0 * cfg.stiffness * dt).exp());

        self.shake_t += dt;
        self.shake = (self.shake - dt * 2.2).max(0.0);
        if self.shake > 0.001 && cfg.shake > 0.001 {
            let a = self.shake * cfg.shake * 0.12;
            let t = self.shake_t * 43.0;
            self.pos += Vec3::new(t.sin() * 0.9, (t * 1.31 + 1.7).sin(), (t * 0.83 + 4.1).sin() * 0.7) * a;
        }
    }

    pub fn view_proj(&self, cfg: &CamCfg, aspect: f32) -> (Mat4, Mat4) {
        // guard against a degenerate basis when look-dir ~ up
        let fwd = (self.look - self.pos).normalize_or(Vec3::NEG_Z);
        let up = if fwd.dot(self.up).abs() > 0.985 { fwd.cross(Vec3::X).normalize_or(Vec3::Y) } else { self.up };
        let view = Mat4::look_at_rh(self.pos, self.look, up);
        let proj = Mat4::perspective_rh(cfg.fov_deg.to_radians(), aspect, 0.08, 2600.0);
        let vp = proj * view;
        (vp, vp.inverse())
    }
}

// quiet the unused import if Quat drops out of future edits
#[allow(unused)]
fn _k(_q: Quat) {}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gunner_seat_is_skipped_when_the_hull_has_no_gun() {
        let none = GunMounts::default();
        let armed = GunMounts { rear: true, ventral: false };
        // a hull with no gun position never lands on the empty seat
        let mut m = CamMode::Ball;
        for _ in 0..12 {
            m = m.next(none);
            assert_ne!(m, CamMode::Gunner, "cycled into a seat that does not exist");
        }
        // and one with a gun reaches it, exactly once per lap
        let mut seen = 0;
        let mut m = CamMode::Ball;
        for _ in 0..4 {
            m = m.next(armed);
            if m == CamMode::Gunner {
                seen += 1;
            }
        }
        assert_eq!(seen, 1, "the gunner seat appeared {seen} times in one lap");
    }

    #[test]
    fn a_rear_gun_is_preferred_to_a_belly_one() {
        // being chased is the common case, so the default seat looks back
        assert_eq!(GunMounts { rear: true, ventral: true }.seat(), Some(GunSeat::Rear));
        assert_eq!(GunMounts { rear: false, ventral: true }.seat(), Some(GunSeat::Ventral));
        assert_eq!(GunMounts::default().seat(), None);
        assert!(!GunMounts::default().any());
    }

    #[test]
    fn every_mode_is_reachable_and_the_cycle_closes() {
        let armed = GunMounts { rear: true, ventral: false };
        let mut seen: Vec<CamMode> = vec![CamMode::Ball];
        let mut m = CamMode::Ball;
        for _ in 0..8 {
            m = m.next(armed);
            if !seen.contains(&m) {
                seen.push(m);
            }
        }
        assert_eq!(seen.len(), 4, "not every camera mode is reachable: {seen:?}");
        assert_eq!(m, CamMode::Ball, "the cycle does not return to where it started");
    }

    #[test]
    fn every_mode_has_a_name() {
        for m in [CamMode::Ball, CamMode::Chase, CamMode::First, CamMode::Gunner] {
            assert!(!m.label().is_empty());
        }
    }
}
