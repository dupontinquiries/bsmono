//! `sim::dragon_abilities` — what a dragon can bend, read from its genes.
//!
//! A dragon's bending is **dragon-scale**: it moves the land, not a handful of
//! blocks. Which abilities it has, and how big, fast and strong they are, is a
//! function of the genome alone ([`repertoire`]):
//!
//! * **Element channels pick the forms.** The dominant channel gives its pure
//!   form (Dust raises an *Upheaval* of stone pillars, Water drives a
//!   *Torrent*, Fire a *Firestorm*, Air a *Downburst*, Storm a *Thunderhead*,
//!   Wood a *Thornbrake*). A strong second channel gives a **hybrid** form
//!   (Dust+Fire → *Magma Rift*, Water+Air → *Blizzard*, Fire+Storm → *Plasma
//!   Lance*, Water+Dust → *Quagmire*, Air+Dust → *Sandstorm*). Wild genomes
//!   have one clear channel (dominant 0.7–1.0, the rest ≤ 0.35), so wild
//!   dragons are pure; hybrids are **bred**: `dragon_breeding::cross` averages
//!   the parents' channels, so a Fire × Storm clutch carries both.
//! * **Purity is power.** The dominant channel's weight scales potency; a
//!   hybrid trades some pure strength for a second, rarer form.
//! * **The body sets the scale.** Size class sets radius and range (a size-4
//!   Upheaval rings a 14 m circle; a size-0 one, 4 m). Bulk makes the wind-up
//!   slower and more visible, and the blow harder.
//! * **Mastery.** A large or very pure dragon also knows its element's
//!   *defensive* form — a *Rampart* of stone (Dust) or thorn (Wood), a *Mist
//!   Veil* (Water) — used to put terrain between itself and a weapon that
//!   hurts it.
//!
//! Numbers are in the duel's damage units and metres. Casting (wind-up, aim,
//! release, lingering areas, terrain edits) is `dragon_cast`. Pure.

use crate::sim::dragon_archetype::{self as arche, Genome, NUM_ELEMENTS};

/// The shape of an ability's effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A ring of effect around the target point (pillars, fire ring).
    Ring,
    /// A filled circle at the target point.
    Disc,
    /// A band from the dragon toward the target.
    Line,
    /// A wall across the line between dragon and target.
    Wall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    Upheaval,
    Torrent,
    Firestorm,
    Downburst,
    Thunderhead,
    Thornbrake,
    MagmaRift,
    Blizzard,
    PlasmaLance,
    Quagmire,
    Sandstorm,
    Rampart,
    MistVeil,
    /// Fire's ash smokescreen: a giant, tall wall of smoke (see `smoke_wall`).
    SmokeWall,
    /// A giant rock heaved and hurled; it stays where it lands.
    Boulder,
}

impl Form {
    pub fn name(self) -> &'static str {
        match self {
            Form::Upheaval => "Upheaval",
            Form::Torrent => "Torrent",
            Form::Firestorm => "Firestorm",
            Form::Downburst => "Downburst",
            Form::Thunderhead => "Thunderhead",
            Form::Thornbrake => "Thornbrake",
            Form::MagmaRift => "Magma Rift",
            Form::Blizzard => "Blizzard",
            Form::PlasmaLance => "Plasma Lance",
            Form::Quagmire => "Quagmire",
            Form::Sandstorm => "Sandstorm",
            Form::Rampart => "Rampart",
            Form::MistVeil => "Mist Veil",
            Form::SmokeWall => "Smoke Wall",
            Form::Boulder => "Boulder",
        }
    }
}

/// Base numbers for one form before the body scales them.
#[derive(Debug, Clone, Copy)]
struct Base {
    shape: Shape,
    radius: f32,
    windup: f32,
    linger: f32,
    cooldown: f32,
    /// Damage on release (a hit), and per second while inside a lingering area.
    hit: f32,
    dps: f32,
    /// Knockback speed (m/s); negative pulls toward the centre.
    knock: f32,
    /// Upward component of the knock (m/s).
    lift: f32,
    /// 0..1 movement slow while inside.
    slow: f32,
    defensive: bool,
}

const fn b(shape: Shape, radius: f32, windup: f32, linger: f32, cooldown: f32, hit: f32, dps: f32, knock: f32, lift: f32, slow: f32, defensive: bool) -> Base {
    Base { shape, radius, windup, linger, cooldown, hit, dps, knock, lift, slow, defensive }
}

fn base(f: Form) -> Base {
    use Shape::*;
    match f {
        //                              shape  r    wind linger cd    hit  dps  knock lift slow  def
        Form::Upheaval => b(Ring, 4.0, 1.6, 0.0, 9.0, 18.0, 0.0, 6.0, 7.0, 0.0, false),
        Form::Torrent => b(Line, 3.0, 1.2, 1.5, 8.0, 10.0, 4.0, 11.0, 1.0, 0.3, false),
        Form::Firestorm => b(Ring, 5.0, 1.4, 5.0, 12.0, 8.0, 7.0, 0.0, 0.0, 0.0, false),
        Form::Downburst => b(Disc, 4.0, 1.0, 0.0, 7.0, 14.0, 0.0, 9.0, -4.0, 0.2, false),
        Form::Thunderhead => b(Disc, 5.0, 2.0, 2.5, 13.0, 26.0, 0.0, 3.0, 2.0, 0.0, false),
        Form::Thornbrake => b(Disc, 4.5, 1.3, 7.0, 11.0, 4.0, 3.0, 0.0, 0.0, 0.65, false),
        Form::MagmaRift => b(Line, 3.0, 1.8, 6.0, 14.0, 16.0, 9.0, 4.0, 3.0, 0.3, false),
        Form::Blizzard => b(Disc, 6.0, 1.6, 6.0, 14.0, 6.0, 5.0, 2.0, 0.0, 0.55, false),
        Form::PlasmaLance => b(Line, 1.5, 1.5, 0.0, 12.0, 34.0, 0.0, 8.0, 2.0, 0.0, false),
        Form::Quagmire => b(Disc, 5.0, 1.5, 8.0, 14.0, 3.0, 2.0, 0.0, 0.0, 0.8, false),
        Form::Sandstorm => b(Disc, 7.0, 1.2, 6.0, 13.0, 3.0, 3.0, 2.0, 0.0, 0.35, false),
        Form::Rampart => b(Wall, 5.0, 0.9, 0.0, 16.0, 0.0, 0.0, 5.0, 2.0, 0.0, true),
        Form::MistVeil => b(Disc, 8.0, 0.8, 8.0, 18.0, 0.0, 0.0, 0.0, 0.0, 0.0, true),
        Form::SmokeWall => b(Wall, 9.0, 1.1, 25.0, 22.0, 0.0, 0.0, 0.0, 0.0, 0.0, true),
        Form::Boulder => b(Disc, 2.5, 1.8, 0.0, 10.0, 30.0, 0.0, 10.0, 4.0, 0.0, false),
    }
}

/// One ability, fully scaled for this dragon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ability {
    pub form: Form,
    /// Genome palette index of the element(s) it draws on.
    pub primary: u8,
    pub secondary: Option<u8>,
    pub shape: Shape,
    /// Effect radius (Ring/Disc), half-width (Line), half-length (Wall) (m).
    pub radius: f32,
    /// Farthest it can be cast (m).
    pub range: f32,
    pub windup_s: f32,
    pub linger_s: f32,
    pub cooldown_s: f32,
    pub hit: f32,
    pub dps: f32,
    pub knock: f32,
    pub lift: f32,
    pub slow01: f32,
    pub defensive: bool,
    /// 0..1 how well it wields it (purity of the channels it draws on).
    pub mastery: f32,
}

/// Everything a dragon can bend.
#[derive(Debug, Clone, PartialEq)]
pub struct Repertoire {
    pub abilities: Vec<Ability>,
    /// Dominant and second channel (index, weight).
    pub dominant: (u8, f32),
    pub second: (u8, f32),
}

/// A second channel at least this strong, and this large a share of the
/// dominant, opens a hybrid form.
pub const HYBRID_MIN: f32 = 0.4;
pub const HYBRID_RATIO: f32 = 0.6;

fn pure_form(e: u8) -> Form {
    match e % 6 {
        0 => Form::Thornbrake,
        1 => Form::Torrent,
        2 => Form::Downburst,
        3 => Form::Upheaval,
        4 => Form::Firestorm,
        _ => Form::Thunderhead,
    }
}

/// The hybrid form for an unordered pair, if the pair has one.
pub fn hybrid_form(a: u8, b: u8) -> Option<Form> {
    let (x, y) = if a % 6 <= b % 6 { (a % 6, b % 6) } else { (b % 6, a % 6) };
    match (x, y) {
        (3, 4) => Some(Form::MagmaRift),
        (1, 2) => Some(Form::Blizzard),
        (4, 5) => Some(Form::PlasmaLance),
        (1, 3) => Some(Form::Quagmire),
        (2, 3) => Some(Form::Sandstorm),
        _ => None,
    }
}

fn defensive_form(e: u8) -> Option<Form> {
    match e % 6 {
        0 | 3 => Some(Form::Rampart),
        1 => Some(Form::MistVeil),
        4 => Some(Form::SmokeWall),
        _ => None,
    }
}

fn scale(form: Form, primary: u8, secondary: Option<u8>, mastery: f32, g: &Genome, size_class: u8) -> Ability {
    let bs = base(form);
    let s = arche::size_scale(size_class) / arche::size_scale(0); // 1 ..≈ 9
    let body = s.sqrt(); // 1 .. 3: radius grows slower than length
    let bulk = g.bulk.clamp(0.0, 1.0);
    let m = mastery.clamp(0.0, 1.0);
    Ability {
        form,
        primary,
        secondary,
        shape: bs.shape,
        radius: bs.radius * (0.7 + 0.3 * body) * if bs.shape == Shape::Wall { 1.0 + 0.3 * body } else { 1.0 },
        range: 18.0 + 6.0 * body + 8.0 * m,
        windup_s: bs.windup * (0.75 + 0.5 * bulk) * (1.15 - 0.3 * m),
        linger_s: bs.linger * (0.7 + 0.6 * m),
        cooldown_s: bs.cooldown * (1.2 - 0.4 * m),
        hit: bs.hit * (0.5 + 0.8 * m) * (0.8 + 0.4 * bulk) * (0.8 + 0.2 * body),
        dps: bs.dps * (0.5 + 0.8 * m),
        knock: bs.knock * (0.8 + 0.4 * bulk),
        lift: bs.lift,
        slow01: (bs.slow * (0.7 + 0.4 * m)).clamp(0.0, 0.9),
        defensive: bs.defensive,
        mastery: m,
    }
}

/// Read a dragon's bending from its genome.
pub fn repertoire(g: &Genome) -> Repertoire {
    let a = arche::classify(g);
    let mut idx: Vec<usize> = (0..NUM_ELEMENTS).collect();
    idx.sort_by(|&x, &y| g.element[y].partial_cmp(&g.element[x]).unwrap_or(std::cmp::Ordering::Equal).then(x.cmp(&y)));
    let (e1, w1) = (idx[0] as u8, g.element[idx[0]].clamp(0.0, 1.0));
    let (e2, w2) = (idx[1] as u8, g.element[idx[1]].clamp(0.0, 1.0));
    let mut abilities = vec![scale(pure_form(e1), e1, None, w1, g, a.size_class)];
    if w2 >= HYBRID_MIN && w2 >= HYBRID_RATIO * w1 {
        if let Some(h) = hybrid_form(e1, e2) {
            abilities.push(scale(h, e1, Some(e2), 0.5 * (w1 + w2), g, a.size_class));
        }
    }
    // Mastery: the big, or the very pure, also know how to hide or wall off
    // -- and a Dust dragon of that kind heaves giant rocks.
    let master = a.size_class >= 2 || w1 >= 0.9;
    if master && e1 == 3 {
        abilities.push(scale(Form::Boulder, e1, None, w1, g, a.size_class));
    }
    if master {
        if let Some(d) = defensive_form(e1) {
            abilities.push(scale(d, e1, None, w1, g, a.size_class));
        }
    }
    Repertoire { abilities, dominant: (e1, w1), second: (e2, w2) }
}

/// One line per ability, for logs.
pub fn describe(r: &Repertoire) -> String {
    r.abilities
        .iter()
        .map(|a| {
            format!(
                "{} ({}{}; r {:.0} m, range {:.0} m, wind-up {:.1} s, hit {:.0}{}{})",
                a.form.name(),
                arche::ELEMENT_NAMES[a.primary as usize % 6],
                a.secondary.map_or(String::new(), |e| format!("+{}", arche::ELEMENT_NAMES[e as usize % 6])),
                a.radius,
                a.range,
                a.windup_s,
                a.hit,
                if a.dps > 0.0 { format!(", {:.0}/s for {:.0} s", a.dps, a.linger_s) } else { String::new() },
                if a.defensive { ", defensive" } else { "" },
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_breeding;

    fn of(el: u8, seed: u64) -> Genome {
        Genome::from_seed_element(seed, el)
    }

    #[test]
    fn every_element_gives_its_pure_form() {
        let want = [Form::Thornbrake, Form::Torrent, Form::Downburst, Form::Upheaval, Form::Firestorm, Form::Thunderhead];
        for (e, f) in want.iter().enumerate() {
            let r = repertoire(&of(e as u8, 11));
            assert_eq!(r.abilities[0].form, *f, "element {e}");
            assert_eq!(r.abilities[0].primary, e as u8);
        }
    }

    #[test]
    fn wild_dragons_are_never_hybrid() {
        for seed in 0..2000u64 {
            let r = repertoire(&Genome::from_seed(seed));
            assert!(r.abilities.iter().all(|a| a.secondary.is_none()), "seed {seed}: {}", describe(&r));
            assert!((1..=3).contains(&r.abilities.len()));
        }
    }

    #[test]
    fn bred_crosses_of_two_elements_carry_the_hybrid() {
        // Dust × Fire parents → Magma Rift in the clutch.
        let mut hybrids = 0;
        for seed in 0..50u64 {
            let child = dragon_breeding::cross(&of(3, seed), &of(4, seed + 1000), seed);
            let r = repertoire(&child);
            if r.abilities.iter().any(|a| a.form == Form::MagmaRift) {
                hybrids += 1;
                let m = r.abilities.iter().find(|a| a.form == Form::MagmaRift).unwrap();
                assert!(m.secondary.is_some());
                // Wielded as well as the two channels together, not the stronger alone.
                assert!((m.mastery - 0.5 * (r.dominant.1 + r.second.1)).abs() < 1e-5);
            }
        }
        assert!(hybrids >= 40, "only {hybrids}/50 Dust×Fire children carry Magma Rift");
        // A pair with no hybrid (Wood × Storm) just gets the stronger pure form.
        let child = dragon_breeding::cross(&of(0, 5), &of(5, 6), 7);
        assert!(repertoire(&child).abilities.iter().all(|a| a.secondary.is_none()));
    }

    #[test]
    fn hybrids_trade_pure_strength() {
        let wild = repertoire(&of(4, 3));
        let bred = repertoire(&dragon_breeding::cross(&of(4, 3), &of(5, 4), 9));
        let pure = |r: &Repertoire| r.abilities.iter().find(|a| a.secondary.is_none() && !a.defensive).unwrap().mastery;
        assert!(pure(&bred) < pure(&wild) - 0.15, "bred {} wild {}", pure(&bred), pure(&wild));
    }

    #[test]
    fn size_scales_the_land_it_moves() {
        let mut small = of(3, 21);
        let mut big = of(3, 21);
        small.bulk = 0.0;
        small.neck_len = 0.0;
        big.bulk = 1.0;
        big.neck_len = 1.0;
        big.wing_span = 1.0;
        let (sc, bc) = (arche::classify(&small).size_class, arche::classify(&big).size_class);
        assert!(bc > sc, "test genomes must differ in size class ({sc} vs {bc})");
        let (s, b) = (repertoire(&small).abilities[0], repertoire(&big).abilities[0]);
        assert!(b.radius > s.radius * 1.3 && b.range > s.range);
        assert!(b.windup_s > s.windup_s, "heavier = slower wind-up");
        assert!(b.hit > s.hit);
    }

    #[test]
    fn big_or_pure_dragons_know_the_defensive_form() {
        let mut g = of(3, 8);
        g.element[3] = 0.95;
        assert!(repertoire(&g).abilities.iter().any(|a| a.form == Form::Rampart && a.defensive));
        let mut weak = of(3, 8);
        weak.element[3] = 0.7;
        weak.bulk = 0.0;
        weak.neck_len = 0.0;
        weak.wing_span = 0.0;
        if arche::classify(&weak).size_class < 2 {
            assert!(!repertoire(&weak).abilities.iter().any(|a| a.defensive));
        }
    }

    #[test]
    fn fire_masters_raise_smoke_walls_dust_masters_hurl_boulders() {
        let mut fire = of(4, 8);
        fire.element[4] = 0.95;
        let r = repertoire(&fire);
        let sw = r.abilities.iter().find(|a| a.form == Form::SmokeWall).expect("smoke wall");
        assert!(sw.defensive && sw.shape == Shape::Wall && sw.linger_s > 10.0);
        let mut dust = of(3, 8);
        dust.element[3] = 0.95;
        let r = repertoire(&dust);
        let b = r.abilities.iter().find(|a| a.form == Form::Boulder).expect("boulder");
        assert!(!b.defensive && b.hit > r.abilities[0].hit * 0.8);
        // A small, impure Dust dragon has neither.
        let mut weak = of(3, 8);
        weak.element[3] = 0.72;
        weak.bulk = 0.0;
        weak.neck_len = 0.0;
        weak.wing_span = 0.0;
        if arche::classify(&weak).size_class < 2 {
            assert!(!repertoire(&weak).abilities.iter().any(|a| a.form == Form::Boulder));
        }
    }

    #[test]
    fn hybrid_pairs_are_unordered_and_sparse() {
        for a in 0..6u8 {
            for b in 0..6u8 {
                assert_eq!(hybrid_form(a, b), hybrid_form(b, a));
            }
        }
        let n = (0..6u8).flat_map(|a| (0..6u8).map(move |b| (a, b))).filter(|(a, b)| a < b && hybrid_form(*a, *b).is_some()).count();
        assert_eq!(n, 5);
    }

    #[test]
    fn deterministic_and_finite() {
        for seed in 0..300u64 {
            let g = Genome::from_seed(seed);
            let (r1, r2) = (repertoire(&g), repertoire(&g));
            assert_eq!(r1, r2);
            for a in &r1.abilities {
                for v in [a.radius, a.range, a.windup_s, a.cooldown_s, a.hit, a.dps, a.knock, a.slow01] {
                    assert!(v.is_finite() && v >= -20.0);
                }
                assert!(a.windup_s > 0.3 && a.cooldown_s > a.windup_s);
            }
        }
    }
}
