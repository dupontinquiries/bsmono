//! targeting -- prepare an ability, see where it goes, then fire it.
//!
//! An ability key used to cast on the press. Now it PREPARES the ability: a
//! guide appears on the ground -- a line for a bolt or a dash, a fan for a
//! volley, a circle where a lob or a storm will land, a ring round the cat
//! for a nova -- and the attack button fires it where the guide shows.
//! Esc or the same key again cancels; another ability key switches.
//!
//! Headless: [`Targeting`] is the input state machine, [`guide_for`] the
//! guide's shape. Both are pure, so the rules are tested without a window;
//! the app feeds one and draws the other. The guide takes its reach from
//! the same rules `Game::cast` uses (the clamps below mirror abilities.rs),
//! and a test holds the two to agreement.
//!
//! Touch keeps its own two-step gesture (drag to aim, lift to fire), which
//! already shows its aim while dragging.

use glam::Vec3;

use crate::gods::{AbilityDef, Archetype};

/// What an input frame asks of the targeting.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Press {
    /// Ability keys pressed this frame (edges).
    pub cast: [bool; 4],
    /// The attack button pressed this frame (an edge, not held).
    pub attack: bool,
    /// Esc / back.
    pub cancel: bool,
}

/// What the targeting decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed.
    Idle,
    /// An ability is prepared (its guide should show).
    Aiming(u32),
    /// Fire this slot now, at the current aim.
    Fire(u32),
    /// The prepared ability was put away.
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Targeting {
    pub slot: Option<u32>,
}

impl Targeting {
    /// One frame of input. While an ability is prepared the attack button
    /// FIRES it (and must not also auto-attack: `eats_attack`).
    pub fn step(&mut self, p: Press) -> Outcome {
        if let Some(s) = self.slot {
            if p.cancel || p.cast[s as usize] {
                self.slot = None;
                return Outcome::Cancelled;
            }
            if let Some(other) = (0..4u32).find(|&k| p.cast[k as usize]) {
                self.slot = Some(other);
                return Outcome::Aiming(other);
            }
            if p.attack {
                self.slot = None;
                return Outcome::Fire(s);
            }
            return Outcome::Aiming(s);
        }
        if let Some(k) = (0..4u32).find(|&k| p.cast[k as usize]) {
            self.slot = Some(k);
            return Outcome::Aiming(k);
        }
        Outcome::Idle
    }

    /// Does the attack button belong to the targeting this frame (so it
    /// does not also start an auto-attack)?
    pub fn eats_attack(&self) -> bool {
        self.slot.is_some()
    }
}

/// The shape a guide draws on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Guide {
    /// From the cat along `dir` for `len`: a bolt, beam, dash or blink.
    Line { from: Vec3, dir: Vec3, len: f32 },
    /// A fan of `len` either side of `dir` by `half`: a volley.
    Fan { from: Vec3, dir: Vec3, len: f32, half: f32 },
    /// Where it lands: a lob, ring, storm or pull.
    Circle { at: Vec3, radius: f32 },
    /// Round the cat: a nova, heal, shield or buff.
    Around { at: Vec3, radius: f32 },
}

/// The guide for ability `ab` cast from `from` at `aim`. The reaches are
/// the ones `Game::cast` uses (abilities.rs): a bolt flies speed x 1.6 s,
/// a lob lands within 20 u, a ring within 14, a storm within 12, a pull
/// within its radius, a dash or blink goes its distance.
pub fn guide_for(ab: &AbilityDef, from: Vec3, aim: Vec3) -> Guide {
    let from = Vec3::new(from.x, 0.0, from.z);
    let d = Vec3::new(aim.x - from.x, 0.0, aim.z - from.z);
    let dist = d.length();
    let dir = if dist < 0.3 { Vec3::Z } else { d / dist };
    let at = |max: f32| from + dir * dist.min(max);
    match ab.arch {
        Archetype::Bolt { speed, .. } => Guide::Line { from, dir, len: speed * 1.6 },
        Archetype::Beam { length, .. } => Guide::Line { from, dir, len: length },
        Archetype::Dash { dist, .. } | Archetype::Blink { dist, .. } => Guide::Line { from, dir, len: dist },
        Archetype::Volley { spread_deg, speed, .. } => Guide::Fan { from, dir, len: speed * 1.6, half: spread_deg.to_radians() * 0.5 },
        Archetype::Lob { radius, .. } => Guide::Circle { at: at(20.0), radius },
        Archetype::Ring { radius, .. } => Guide::Circle { at: at(14.0), radius },
        Archetype::Storm { radius, .. } => Guide::Circle { at: at(12.0), radius },
        Archetype::Pull { radius, .. } => Guide::Circle { at: at(radius), radius },
        Archetype::Meteor { range, radius, .. } => Guide::Circle { at: at(range), radius },
        Archetype::DanceParty { radius, .. } => Guide::Circle { at: at(crate::kit::PARTY_RANGE), radius },
        Archetype::Litter { speed, life, .. } => Guide::Fan { from, dir, len: (speed * life).min(crate::kit::SEEK * 2.0), half: crate::kit::FAN_DEG.to_radians() },
        Archetype::Giggle { radius, .. } => Guide::Around { at: from, radius },
        Archetype::Nova { radius, .. } | Archetype::Heal { radius, .. } => Guide::Around { at: from, radius },
        Archetype::Shield { .. } | Archetype::Buff { .. } => Guide::Around { at: from, radius: 1.6 },
    }
}

/// Dots on the ground plane (y = 0; the drawer lifts them onto the
/// terrain) that trace a guide, about `spacing` apart.
pub fn guide_points(g: &Guide, spacing: f32) -> Vec<Vec3> {
    let sp = spacing.max(0.2);
    let line = |from: Vec3, dir: Vec3, len: f32| -> Vec<Vec3> {
        let n = (len / sp).ceil().max(1.0) as usize;
        (1..=n).map(|k| from + dir * (len * k as f32 / n as f32)).collect()
    };
    let circle = |at: Vec3, r: f32| -> Vec<Vec3> {
        let n = ((std::f32::consts::TAU * r) / sp).ceil().clamp(8.0, 64.0) as usize;
        (0..n)
            .map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                at + Vec3::new(a.cos() * r, 0.0, a.sin() * r)
            })
            .collect()
    };
    match *g {
        Guide::Line { from, dir, len } => line(from, dir, len),
        Guide::Fan { from, dir, len, half } => {
            let rot = |a: f32| Vec3::new(dir.x * a.cos() - dir.z * a.sin(), 0.0, dir.x * a.sin() + dir.z * a.cos());
            let mut v = line(from, rot(-half), len);
            v.extend(line(from, dir, len));
            v.extend(line(from, rot(half), len));
            v
        }
        Guide::Circle { at, radius } | Guide::Around { at, radius } => {
            let mut v = circle(at, radius);
            if matches!(g, Guide::Circle { .. }) {
                v.push(at);
            }
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    fn press(f: impl FnOnce(&mut Press)) -> Press {
        let mut p = Press::default();
        f(&mut p);
        p
    }

    #[test]
    fn a_key_prepares_the_attack_button_fires_and_esc_or_the_key_cancels() {
        let mut t = Targeting::default();
        assert_eq!(t.step(Press::default()), Outcome::Idle);
        assert_eq!(t.step(press(|p| p.cast[1] = true)), Outcome::Aiming(1));
        assert!(t.eats_attack());
        assert_eq!(t.step(Press::default()), Outcome::Aiming(1), "held until fired");
        assert_eq!(t.step(press(|p| p.attack = true)), Outcome::Fire(1));
        assert!(!t.eats_attack(), "fired: the next click is an attack again");
        assert_eq!(t.step(press(|p| p.attack = true)), Outcome::Idle, "an attack with nothing prepared is just an attack");
        t.step(press(|p| p.cast[2] = true));
        assert_eq!(t.step(press(|p| p.cancel = true)), Outcome::Cancelled);
        t.step(press(|p| p.cast[2] = true));
        assert_eq!(t.step(press(|p| p.cast[2] = true)), Outcome::Cancelled, "the same key again puts it away");
        t.step(press(|p| p.cast[0] = true));
        assert_eq!(t.step(press(|p| p.cast[3] = true)), Outcome::Aiming(3), "another key switches");
    }

    /// Every ability's guide points where the cast will go (along the
    /// aim), and lands where the cast lands (the same range clamps).
    #[test]
    fn every_guide_points_where_its_cast_goes() {
        let from = Vec3::new(3.0, 0.0, -7.0);
        let aim = from + Vec3::new(0.0, 0.0, 30.0); // far: exercises the clamps
        for def in ROSTER.iter() {
            for ab in def.abilities.iter() {
                let g = guide_for(ab, from, aim);
                match g {
                    Guide::Line { dir, len, .. } | Guide::Fan { dir, len, .. } => {
                        assert!(dir.dot(Vec3::Z) > 0.999, "{}: {dir}", ab.name);
                        assert!(len > 0.5, "{}: len {len}", ab.name);
                    }
                    Guide::Circle { at, .. } => {
                        let max = match ab.arch {
                            Archetype::Lob { .. } => 20.0,
                            Archetype::Ring { .. } => 14.0,
                            Archetype::Storm { .. } => 12.0,
                            Archetype::Pull { radius, .. } => radius,
                            Archetype::Meteor { range, .. } => range,
                            Archetype::DanceParty { .. } => crate::kit::PARTY_RANGE,
                            _ => unreachable!(),
                        };
                        let v = at - from;
                        assert!((v.length() - max).abs() < 1e-3 && v.normalize().dot(Vec3::Z) > 0.999, "{}: lands at {at}", ab.name);
                    }
                    Guide::Around { at, .. } => assert!((at - from).length() < 1e-4, "{}", ab.name),
                }
                assert!(!guide_points(&g, 1.2).is_empty());
            }
        }
    }

    #[test]
    fn guide_points_trace_the_shape() {
        let g = Guide::Line { from: Vec3::ZERO, dir: Vec3::X, len: 12.0 };
        let p = guide_points(&g, 1.2);
        assert!(p.len() >= 10 && (p.last().unwrap().x - 12.0).abs() < 1e-4);
        let c = guide_points(&Guide::Circle { at: Vec3::new(5.0, 0.0, 5.0), radius: 3.0 }, 1.0);
        for q in &c[..c.len() - 1] {
            assert!(((*q - Vec3::new(5.0, 0.0, 5.0)).length() - 3.0).abs() < 1e-4);
        }
    }
}
