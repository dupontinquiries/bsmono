//! IDLE — the motion a machine makes when it is doing nothing.
//!
//! A model standing perfectly still reads as a prop. A model that breathes
//! reads as a thing that is switched on, and it costs almost nothing: a few
//! sinusoids at the right frequencies against the right axes.
//!
//! **The frequencies are not authored per chassis.** They come from the same
//! measured properties everything else here reads — mass, reach, lift class,
//! whether it has wheels — which is the shared latent space rather than a
//! second table to keep in sync. A heavy machine idles slowly because its
//! actuators are moving more metal, exactly as `actuator.rs` says its fold
//! is slower, and both fall out of the same number.
//!
//! That also means a chassis nobody has looked at gets a plausible idle for
//! free, which matters at 48 chassis and would matter more at 259.

use std::f32::consts::TAU;

/// What kind of thing is idling. Drives which axes move at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// stands on legs: weight shifts, chest rises, arms hang and drift
    Biped,
    /// many legs: small independent leg adjustments, low body
    Multiped,
    /// no ground contact: bobs and yaws, never settles
    Hoverer,
    /// wings and control surfaces twitch; the body does not
    Flyer,
    /// suspension settles and re-settles; the hull barely moves
    Tracked,
    /// two wheels: leans, because it cannot stand still upright
    Moto,
}

impl Kind {
    /// From the class label the roster already carries.
    pub fn from_class(c: &str) -> Kind {
        match c {
            "hoverer" | "hovertank" => Kind::Hoverer,
            "flyer" | "heli" | "spaceship" | "hawk" | "dragonflyer" | "jet" => Kind::Flyer,
            "tank" | "truck" => Kind::Tracked,
            "moto" => Kind::Moto,
            "spider" | "spiderbot" | "animal" | "mech" => Kind::Multiped,
            _ => Kind::Biped,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Biped => "BIPED",
            Kind::Multiped => "MULTIPED",
            Kind::Hoverer => "HOVERER",
            Kind::Flyer => "FLYER",
            Kind::Tracked => "TRACKED",
            Kind::Moto => "MOTO",
        }
    }
}

/// The pose an idle produces. Small offsets, applied on top of whatever the
/// chassis is already doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Idle {
    /// vertical, metres
    pub bob: f32,
    /// side to side, metres
    pub sway: f32,
    /// radians about the vertical
    pub yaw: f32,
    /// radians about the forward axis
    pub roll: f32,
    /// radians about the lateral axis
    pub pitch: f32,
    /// limb swing amplitude, radians. Zero for anything without limbs.
    pub limb: f32,
}

/// What the chassis is, as measured elsewhere.
#[derive(Clone, Copy, Debug)]
pub struct Chassis {
    pub kind: Kind,
    /// mass proxy, relative — 1.0 is an ordinary bot, 2.6 a boss
    pub scale: f32,
    /// longest dimension, metres. Reach is what makes an idle read slow.
    pub reach_m: f32,
}

impl Default for Chassis {
    fn default() -> Chassis {
        Chassis { kind: Kind::Biped, scale: 1.0, reach_m: 4.0 }
    }
}

/// BASE FREQUENCY FROM SIZE, not from a table.
///
/// A pendulum's period goes as the square root of its length, so a big
/// machine idles slowly for the same reason a big clock ticks slowly. This
/// is the same sqrt that makes a heavy fold slow in actuator.rs — one
/// relationship, two uses.
pub fn base_hz(c: &Chassis) -> f32 {
    let len = (c.reach_m * c.scale).max(0.3);
    (1.0 / len.sqrt()).clamp(0.12, 1.6)
}

/// The idle pose at time `t`.
pub fn pose(c: &Chassis, t: f32) -> Idle {
    let f = base_hz(c);
    let a = TAU * f * t;
    // amplitude scales with the machine, so a boss's sway is proportionally
    // the same and absolutely much larger
    let s = c.scale.max(0.1);
    match c.kind {
        Kind::Biped => Idle {
            bob: (a * 1.0).sin() * 0.012 * s,
            sway: (a * 0.5).sin() * 0.02 * s,
            yaw: (a * 0.33).sin() * 0.02,
            roll: (a * 0.5).cos() * 0.008,
            pitch: 0.0,
            limb: (a * 0.5).sin() * 0.05,
        },
        Kind::Multiped => Idle {
            bob: (a * 1.7).sin() * 0.006 * s,
            sway: (a * 0.8).sin() * 0.008 * s,
            yaw: (a * 0.4).sin() * 0.035,
            roll: 0.0,
            pitch: (a * 1.3).sin() * 0.01,
            // many small legs, each adjusting: faster and shallower
            limb: (a * 2.1).sin() * 0.08,
        },
        Kind::Hoverer => Idle {
            // never settles, which is the whole read of a hoverer
            bob: ((a * 0.7).sin() + (a * 1.9).sin() * 0.4) * 0.05 * s,
            sway: (a * 0.45).sin() * 0.045 * s,
            yaw: (a * 0.28).sin() * 0.05,
            roll: (a * 0.6).sin() * 0.03,
            pitch: (a * 0.5).cos() * 0.02,
            limb: 0.0,
        },
        Kind::Flyer => Idle {
            // the body is still; the surfaces are not
            bob: (a * 0.4).sin() * 0.004 * s,
            sway: 0.0,
            yaw: 0.0,
            roll: (a * 0.9).sin() * 0.012,
            pitch: (a * 0.7).sin() * 0.008,
            limb: (a * 1.4).sin() * 0.04,
        },
        Kind::Tracked => Idle {
            // suspension settling: almost nothing, and slow
            bob: (a * 0.5).sin() * 0.004 * s,
            sway: 0.0,
            yaw: 0.0,
            roll: (a * 0.3).sin() * 0.004,
            pitch: (a * 0.35).cos() * 0.003,
            limb: 0.0,
        },
        Kind::Moto => Idle {
            bob: (a * 0.8).sin() * 0.006 * s,
            sway: (a * 0.6).sin() * 0.03 * s,
            yaw: 0.0,
            // it cannot stand still upright, so it leans and corrects
            roll: (a * 0.6).sin() * 0.09,
            pitch: 0.0,
            limb: 0.0,
        },
    }
}

/// Peak displacement over a cycle — for checking an idle stays subtle.
pub fn peak(c: &Chassis) -> f32 {
    let mut m: f32 = 0.0;
    for i in 0..240 {
        let p = pose(c, i as f32 / 240.0 / base_hz(c).max(1e-3));
        m = m.max(p.bob.abs()).max(p.sway.abs());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bigger_machine_idles_slower() {
        // the same sqrt that makes a heavy fold slow in actuator.rs — one
        // relationship, two uses, rather than two tables to keep in sync
        let bot = Chassis::default();
        let boss = Chassis { scale: 2.6, ..bot };
        assert!(base_hz(&boss) < base_hz(&bot));
        let ratio = base_hz(&bot) / base_hz(&boss);
        assert!((ratio - 2.6f32.sqrt()).abs() < 0.05, "ratio {ratio} should track sqrt(scale)");
    }

    #[test]
    fn every_kind_moves_and_none_of_them_flails() {
        // an idle that does nothing is a prop; one you can see is a bug
        for k in [Kind::Biped, Kind::Multiped, Kind::Hoverer, Kind::Flyer, Kind::Tracked, Kind::Moto] {
            let c = Chassis { kind: k, ..Chassis::default() };
            let p = peak(&c);
            assert!(p > 0.0005, "{} does not move at all", k.label());
            assert!(p < 0.25, "{} moves {p} m at idle, which is a stagger", k.label());
        }
    }

    #[test]
    fn a_hoverer_never_settles_and_a_tank_barely_moves() {
        // the two ends of the range, and the reason kind exists at all
        let hover = Chassis { kind: Kind::Hoverer, ..Chassis::default() };
        let tank = Chassis { kind: Kind::Tracked, ..Chassis::default() };
        assert!(peak(&hover) > peak(&tank) * 5.0,
            "hover {} vs tank {}", peak(&hover), peak(&tank));
    }

    #[test]
    fn a_bike_leans_and_a_tank_does_not() {
        let moto = pose(&Chassis { kind: Kind::Moto, ..Chassis::default() }, 0.4);
        let tank = pose(&Chassis { kind: Kind::Tracked, ..Chassis::default() }, 0.4);
        assert!(moto.roll.abs() > tank.roll.abs() * 5.0);
    }

    #[test]
    fn only_limbed_things_swing_limbs() {
        for k in [Kind::Hoverer, Kind::Tracked, Kind::Moto] {
            let c = Chassis { kind: k, ..Chassis::default() };
            assert_eq!(pose(&c, 0.7).limb, 0.0, "{} swung a limb it has not got", k.label());
        }
        for k in [Kind::Biped, Kind::Multiped, Kind::Flyer] {
            let c = Chassis { kind: k, ..Chassis::default() };
            let any = (0..30).any(|i| pose(&c, i as f32 * 0.11).limb.abs() > 1e-4);
            assert!(any, "{} never moved a limb", k.label());
        }
    }

    #[test]
    fn a_boss_sways_further_in_metres_and_the_same_in_proportion() {
        // it should read as heavy, not as twitchy
        let bot = Chassis { kind: Kind::Biped, ..Chassis::default() };
        let boss = Chassis { kind: Kind::Biped, scale: 2.6, ..bot };
        assert!(peak(&boss) > peak(&bot), "a boss should sway further in absolute terms");
        let rel_bot = peak(&bot) / bot.reach_m;
        let rel_boss = peak(&boss) / (boss.reach_m * boss.scale);
        assert!((rel_bot - rel_boss).abs() < 0.002, "proportionally it should look the same");
    }

    #[test]
    fn the_class_labels_the_roster_uses_all_map_somewhere() {
        for c in ["hoverer", "tank", "moto", "spider", "flyer", "heli", "raidboss", "car", "racer"] {
            let _ = Kind::from_class(c);
        }
        assert_eq!(Kind::from_class("spiderbot"), Kind::Multiped);
        assert_eq!(Kind::from_class("hawk"), Kind::Flyer);
        assert_eq!(Kind::from_class("car"), Kind::Biped, "a car's ROBOT form is a biped");
    }
}
