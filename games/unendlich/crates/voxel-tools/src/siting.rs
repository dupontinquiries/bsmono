//! Where should this go, and why.
//!
//! Everything here is about the spot the player is standing on.
//! `survey` asks whether a footprint is flat; `scatter` places many
//! small things; the layout arranges rooms inside a boundary someone
//! else chose. Nothing asks the question a map is made of: OF ALL THE
//! PLACES THIS COULD GO, WHICH IS RIGHT, AND WHAT MAKES IT RIGHT.
//!
//! That question is most of what distinguishes a world from a
//! collection of buildings. A castle on flat ground in the open is a
//! castle nobody would have built: castles are on the high ground
//! above a crossing, because that is what a castle is FOR. A mill is
//! on a river or it is not a mill. A village is where shelter, water
//! and a road meet, and that is why villages are where they are.
//!
//! WHAT THIS READS
//!
//! Five things, from the world itself:
//!
//!   RELIEF     how high this stands over what surrounds it
//!   LEVEL      how much cut and fill a footprint would cost
//!   WATER      how far to the nearest water
//!   SHELTER    how much higher ground is close enough to break wind
//!              and hide the place
//!   OUTLOOK    how far you can see, which is the opposite of shelter
//!
//! Shelter and outlook oppose each other on purpose. A watchtower
//! wants outlook and does not care about shelter; a farmstead wants
//! shelter and does not care about outlook. A siting that could
//! maximise both would be answering a question nobody asked.

use crate::VoxelWorld;

/// What the land is like at one spot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    pub at: (i32, i32),
    /// Ground height here.
    pub y: i32,
    /// How high this stands over the surrounding ring, in blocks.
    /// Negative in a hollow.
    pub relief: f32,
    /// Blocks of cut and fill a footprint here would need.
    pub unevenness: f32,
    /// Distance to the nearest water, or None if none is near.
    pub water: Option<f32>,
    /// How much higher ground is within a short walk: 0 open, 1 enclosed.
    pub shelter: f32,
    /// How far the eye carries, 0 to 1.
    pub outlook: f32,
}

impl Reading {
    pub fn describe(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        v.push(match self.relief {
            r if r > 8.0 => "high above the land around it".into(),
            r if r > 3.0 => "on a rise".into(),
            r if r < -4.0 => "in a hollow".into(),
            _ => "on level ground".into(),
        });
        if let Some(w) = self.water {
            v.push(if w < 8.0 { "on the water".into() } else { format!("{w:.0} blocks from water") });
        } else { v.push("no water near".into()); }
        if self.shelter > 0.5 { v.push("sheltered".into()); }
        if self.outlook > 0.6 { v.push("with a long view".into()); }
        if self.unevenness > 6.0 { v.push("but steep".into()); }
        v.join(", ")
    }
}

fn ground<W: VoxelWorld>(w: &W, x: i32, z: i32, from_y: i32) -> i32 {
    (0..=from_y + 120).rev().find(|&y| w.get(x, y, z) != 0).unwrap_or(0)
}

fn is_water<W: VoxelWorld>(w: &W, x: i32, y: i32, z: i32) -> bool {
    crate::palette::by_id(w.get(x, y, z)).map(|b| b.family == crate::palette::Family::Liquid)
        .unwrap_or(false)
}

/// Read one spot. `span` is roughly how big the thing being placed is,
/// which decides how far out "around it" means.
pub fn read<W: VoxelWorld>(w: &W, at: (i32, i32), span: i32, from_y: i32) -> Reading {
    let span = span.clamp(4, 200);
    let y = ground(w, at.0, at.1, from_y);
    // the ring around it, at the thing's own scale
    let r = span;
    let ring: Vec<i32> = (0..16).map(|i| {
        let a = i as f32 / 16.0 * std::f32::consts::TAU;
        ground(w, at.0 + (a.cos() * r as f32) as i32, at.1 + (a.sin() * r as f32) as i32, from_y)
    }).collect();
    let mean = ring.iter().sum::<i32>() as f32 / ring.len() as f32;
    let relief = y as f32 - mean;
    // cut and fill over the footprint itself
    let half = (span / 2).max(2);
    let mut hs: Vec<i32> = Vec::new();
    let step = (half / 4).max(1);
    for dx in (-half..=half).step_by(step as usize) {
        for dz in (-half..=half).step_by(step as usize) {
            hs.push(ground(w, at.0 + dx, at.1 + dz, from_y));
        }
    }
    hs.sort_unstable();
    let med = hs[hs.len() / 2];
    let unevenness = hs.iter().map(|h| (h - med).abs() as f32).sum::<f32>() / hs.len() as f32;
    // water: search outward in rings rather than a full square, which
    // is the difference between quick and unusable at this scale
    let mut water = None;
    'search: for d in (4..=span * 3).step_by((span / 3).max(3) as usize) {
        for i in 0..24 {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            let (wx, wz) = (at.0 + (a.cos() * d as f32) as i32, at.1 + (a.sin() * d as f32) as i32);
            let g = ground(w, wx, wz, from_y);
            if is_water(w, wx, g, wz) || is_water(w, wx, g + 1, wz) {
                water = Some(d as f32);
                break 'search;
            }
        }
    }
    // shelter is higher ground close by; outlook is its absence, seen
    // further out
    let near_high = ring.iter().filter(|h| **h > y + 3).count() as f32 / ring.len() as f32;
    let far: Vec<i32> = (0..16).map(|i| {
        let a = i as f32 / 16.0 * std::f32::consts::TAU;
        ground(w, at.0 + (a.cos() * (r * 4) as f32) as i32, at.1 + (a.sin() * (r * 4) as f32) as i32, from_y)
    }).collect();
    let far_high = far.iter().filter(|h| **h > y + 6).count() as f32 / far.len() as f32;
    Reading { at, y, relief, unevenness, water,
              shelter: near_high, outlook: (1.0 - far_high).clamp(0.0, 1.0) }
}

/// What a kind of place wants from the land.
///
/// Weights, not rules: a castle PREFERS high ground and will take
/// level ground if that is all there is. A rule would refuse to build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wants {
    pub relief: f32,
    pub level: f32,
    pub water: f32,
    pub shelter: f32,
    pub outlook: f32,
    /// Why, in one line, for the player.
    pub because: &'static str,
}

/// What a kind of thing wants. Matched on whole words, the way the
/// room table is: a substring inside an unrelated phrase deciding
/// where a castle goes would be worse here than anywhere.
pub fn wants_for(kind: &str) -> Wants {
    let l = kind.to_lowercase();
    let words: Vec<&str> = l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| *x == w || x.trim_end_matches('s') == w);
    if has("castle") || has("fortress") || has("keep") || has("citadel") || has("watchtower")
        || has("tower") || has("fort") || has("outpost") {
        Wants { relief: 1.0, level: 0.4, water: 0.2, shelter: -0.3, outlook: 1.0,
                because: "a fortress holds the high ground and sees who is coming" }
    } else if has("mill") || has("watermill") || has("dock") || has("quay") || has("harbour")
        || has("bridge") || has("fishery") {
        Wants { relief: -0.2, level: 0.6, water: 1.0, shelter: 0.0, outlook: 0.1,
                because: "it needs the water more than it needs anything else" }
    } else if has("village") || has("town") || has("hamlet") || has("farm") || has("farmstead")
        || has("inn") || has("cottage") || has("hut") || has("house") {
        Wants { relief: 0.1, level: 1.0, water: 0.8, shelter: 0.7, outlook: 0.1,
                because: "people settle where the ground is flat, the water is close \
                          and the wind is broken" }
    } else if has("temple") || has("shrine") || has("monastery") || has("cathedral")
        || has("mausoleum") || has("tomb") {
        Wants { relief: 0.8, level: 0.5, water: 0.1, shelter: 0.2, outlook: 0.7,
                because: "it is meant to be seen and to be apart" }
    } else if has("mine") || has("quarry") || has("cave") || has("bunker") || has("crypt") {
        Wants { relief: 0.3, level: 0.1, water: -0.4, shelter: 0.5, outlook: -0.2,
                because: "it goes into the ground, and dry ground at that" }
    } else {
        // ANYTHING ELSE still wants ground it can stand on. This is the
        // honest default: level and not in a bog.
        Wants { relief: 0.2, level: 1.0, water: 0.2, shelter: 0.2, outlook: 0.2,
                because: "somewhere flat enough to build and not far from water" }
    }
}

/// A spot, scored, with the reason it scored that way.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub reading: Reading,
    pub score: f32,
    /// What earned or cost it points, best first.
    pub reasons: Vec<String>,
}

impl Candidate {
    pub fn summary(&self) -> String {
        format!("({}, {}) at y={}: {} -- {}", self.reading.at.0, self.reading.at.1,
                self.reading.y, self.reading.describe(),
                self.reasons.first().cloned().unwrap_or_default())
    }
}

fn score(r: &Reading, w: &Wants, span: i32) -> (f32, Vec<String>) {
    let mut parts: Vec<(f32, String)> = Vec::new();
    // relief, normalised against the thing's own size: eight blocks of
    // rise means something different to a hut and to a citadel
    let rel = (r.relief / (span as f32 * 0.25).max(4.0)).clamp(-1.5, 1.5);
    parts.push((rel * w.relief, if rel > 0.3 { "stands above the land".into() }
                                else if rel < -0.3 { "sits in a hollow".into() }
                                else { "level with its surroundings".into() }));
    let lvl = (1.0 - r.unevenness / 8.0).clamp(-1.0, 1.0);
    parts.push((lvl * w.level, if lvl > 0.5 { "the ground is flat".into() }
                               else { format!("{:.0} blocks of cut and fill", r.unevenness) }));
    let wat = match r.water {
        Some(d) => (1.0 - d / (span as f32 * 2.0).max(30.0)).clamp(-0.5, 1.0),
        None => -0.5,
    };
    parts.push((wat * w.water, match r.water {
        Some(d) if d < 12.0 => "water at hand".into(),
        Some(d) => format!("water {d:.0} blocks off"),
        None => "no water near".into(),
    }));
    parts.push((r.shelter * w.shelter, if r.shelter > 0.4 { "sheltered by higher ground".into() }
                                       else { "open to the weather".into() }));
    parts.push((r.outlook * w.outlook, if r.outlook > 0.6 { "a long view".into() }
                                       else { "little to see".into() }));
    let total: f32 = parts.iter().map(|(s, _)| s).sum();
    // the reasons that MATTERED, largest effect first, so the answer
    // explains itself rather than listing everything
    parts.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap_or(std::cmp::Ordering::Equal));
    (total, parts.into_iter().filter(|(s, _)| s.abs() > 0.15).map(|(_, w)| w).take(3).collect())
}

/// Look around and find where this should go.
///
/// Searches a ring of candidates out to `reach`, reads each, and
/// scores it against what the kind of place wants. Returns them best
/// first, so a caller can take the best or offer a few.
pub fn choose<W: VoxelWorld>(w: &W, from: (i32, i32), reach: i32, span: i32,
                             kind: &str, from_y: i32) -> (Vec<Candidate>, Wants) {
    let wants = wants_for(kind);
    let reach = reach.clamp(span.max(8), 400);
    let mut out: Vec<Candidate> = Vec::new();
    // where the player is, always considered: they pointed at it
    let mut spots: Vec<(i32, i32)> = vec![from];
    for ring in 1..=3 {
        let d = reach * ring / 3;
        for i in 0..(ring * 6) {
            let a = i as f32 / (ring * 6) as f32 * std::f32::consts::TAU;
            spots.push((from.0 + (a.cos() * d as f32) as i32,
                        from.1 + (a.sin() * d as f32) as i32));
        }
    }
    for at in spots {
        let r = read(w, at, span, from_y);
        let (s, reasons) = score(&r, &wants, span);
        out.push(Candidate { reading: r, score: s, reasons });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    (out, wants)
}

/// What to tell the player about where it went.
pub fn explain(best: &Candidate, here: &Candidate, wants: &Wants) -> String {
    let moved = best.reading.at != here.reading.at;
    if !moved {
        return format!("here is right: {}", best.reasons.first().cloned()
                       .unwrap_or_else(|| best.reading.describe()));
    }
    let d = (((best.reading.at.0 - here.reading.at.0).pow(2)
            + (best.reading.at.1 - here.reading.at.1).pow(2)) as f32).sqrt();
    format!("moved {d:.0} blocks: {}. {} Here would have been {}.",
            best.reasons.join(", "), wants.because,
            here.reasons.first().cloned().unwrap_or_else(|| here.reading.describe()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hill at (60,0), a lake at (-70,0), flat elsewhere.
    struct Land;
    impl VoxelWorld for Land {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            let d = (((x - 60).pow(2) + z.pow(2)) as f32).sqrt();
            let h = 64 + (24.0 - d * 0.5).max(0.0) as i32;
            let lake = (((x + 70).pow(2) + z.pow(2)) as f32).sqrt() < 22.0;
            if lake {
                if y <= 62 { return 1; }
                if y <= 64 { return crate::palette::resolve_or_suggest("water").0; }
                return 0;
            }
            if y <= h { 1 } else { 0 }
        }
        fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
    }

    #[test]
    fn a_fortress_goes_to_the_high_ground_and_a_mill_goes_to_the_water() {
        // a castle on flat ground in the open is a castle nobody would
        // have built
        let (castle, w1) = choose(&Land, (0, 0), 90, 20, "a castle", 64);
        let best = &castle[0];
        assert!(best.reading.at.0 > 25, "it went toward the hill: {}", best.summary());
        assert!(best.reading.relief > 3.0, "and it is high: {}", best.summary());
        assert!(w1.because.contains("high ground"));
        let (mill, w2) = choose(&Land, (0, 0), 90, 20, "a watermill", 64);
        let m = &mill[0];
        assert!(m.reading.at.0 < -20, "it went toward the lake: {}", m.summary());
        assert!(m.reading.water.map(|d| d < 60.0).unwrap_or(false), "{}", m.summary());
        assert!(w2.because.contains("water"));
        // AND THEY DISAGREE, which is the whole point: one question,
        // two right answers depending on what is being built
        assert_ne!(best.reading.at, m.reading.at);
    }

    #[test]
    fn a_spot_says_why_it_was_chosen() {
        let (c, wants) = choose(&Land, (0, 0), 80, 20, "a watchtower", 64);
        let best = &c[0];
        assert!(!best.reasons.is_empty(), "it explains itself");
        assert!(best.summary().contains("y="), "{}", best.summary());
        // the reasons are the ones that MATTERED, not everything
        assert!(best.reasons.len() <= 3, "{:?}", best.reasons);
        // and moving is explained against staying
        let here = c.iter().find(|x| x.reading.at == (0, 0)).expect("the player's spot is considered");
        let e = explain(best, here, &wants);
        assert!(e.contains("moved") || e.contains("here is right"), "{e}");
        if e.contains("moved") {
            assert!(e.contains("Here would have been"), "it says what was given up: {e}");
        }
    }

    #[test]
    fn shelter_and_outlook_pull_against_each_other() {
        // a siting that could maximise both would be answering a
        // question nobody asked
        let tower = wants_for("a watchtower");
        let farm = wants_for("a farmstead");
        assert!(tower.outlook > 0.5 && tower.shelter < 0.0, "a tower wants to see and be seen");
        assert!(farm.shelter > 0.5 && farm.outlook < 0.3, "a farm wants the wind broken");
        // and the flat default still wants somewhere buildable
        let other = wants_for("a scriptorium");
        assert!(other.level > 0.5, "anything wants ground it can stand on");
        assert!(other.because.contains("flat enough"));
    }

    #[test]
    fn the_reading_describes_the_land_in_words() {
        let hill = read(&Land, (60, 0), 20, 64);
        assert!(hill.relief > 8.0, "{:?}", hill);
        assert!(hill.describe().contains("high above"), "{}", hill.describe());
        let shore = read(&Land, (-48, 0), 20, 64);
        assert!(shore.water.is_some(), "the lake is found: {:?}", shore);
        assert!(shore.describe().contains("water") || shore.describe().contains("on the water"),
                "{}", shore.describe());
        let flat = read(&Land, (0, 90), 20, 64);
        assert!(flat.relief.abs() < 3.0, "{:?}", flat);
        assert!(flat.unevenness < 2.0, "flat ground is level: {:?}", flat);
    }

    #[test]
    fn a_place_matched_by_a_substring_does_not_decide_where_it_goes() {
        // "a substring inside an unrelated phrase deciding where a
        // castle goes would be worse here than anywhere"
        let a = wants_for("the housing of the mechanism");
        let b = wants_for("a house");
        assert_ne!(a.because, b.because, "`housing` is not `house`");
        // and the plural is the same word
        assert_eq!(wants_for("cottages").because, wants_for("a cottage").because);
    }

    #[test]
    fn wanting_the_high_ground_does_not_refuse_flat_ground() {
        // weights, not rules: a rule would refuse to build
        struct Flat;
        impl VoxelWorld for Flat {
            fn get(&self, _: i32, y: i32, _: i32) -> u16 { if y <= 64 { 1 } else { 0 } }
            fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
        }
        let (c, _) = choose(&Flat, (0, 0), 60, 20, "a castle", 64);
        assert!(!c.is_empty(), "it still chooses somewhere");
        assert!(c[0].reading.y == 64);
    }
}
