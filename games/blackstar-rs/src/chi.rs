//! CHI — what a human body can hold of the seven elements.
//!
//! The bots run on valence (`valence.rs`); the WATCH-BEARER does not.
//! A human ice-clone shapeshifter carrying the Watch levels the way a
//! monk does, not the way a machine does: attunement is EARNED per
//! element, in the element, and the body's worthiness to wear serious
//! kit follows from it. Mana by way of chi: seven pools, leveled on a
//! widening curve, spent nowhere — chi is not ammunition, it is what
//! the body has become.
//!
//! The bearer is learning what an Ice Clone already knows without
//! trying: per the treatment (`Clones.md`, and the worldbuilding
//! addendum `TF_CLONE_CIVILIZATIONS_AND_CONVERGENCE.md`), Ice Clones
//! and the elemental Clone races generally do not age the way anything
//! else in this cast does — a native of one of these worlds who has
//! stood watch for a thousand years is not old by their own reckoning.
//! Each people mostly keeps to its own world or outpost; contact
//! between them is occasional, not constant. `arena_element` below —
//! frostline/kuiper/pluto to Ice, emberpit/unicron to Fire, trench to
//! Water, and so on — is that geography: every arena a suited raid
//! visits is somebody's home, not a neutral battlefield, and the chi
//! banked there is the body being taught by the world itself rather
//! than by a teacher who bothered to travel.
//!
//! Where chi comes from (both wired into the sims today):
//! * a NIGHTFALL cleared banks chi in the burn element — the Watch
//!   attunes to whatever ran hot that week;
//! * a raid cleared IN A SUIT banks chi in the arena's element — the
//!   deep sites each have a nature, and standing in it as a human is
//!   the training. Machines earn valence there instead; a body that
//!   walked frostline in a suit earned something else.
//!
//! What chi is FOR:
//! * WORTHINESS — human-fit armor mods (`loadout.rs`) carry an
//!   elemental governor: LEAD-WEAVE SHELL answers to Rock, RADIATOR
//!   VANES to Fire. Heavier mods (more slots) demand higher levels.
//!   `worthy` is the gate the gear menu shows the reason for.
//! * THE WATCH — attunement level shortens the attune delay toward
//!   that element (`attune_mul`), which is the shapeshifter's craft:
//!   the ice clone does not WAIT for ice.
//! * THE SHIFT — at ice level 3 the bearer can hold the ice-clone
//!   form (`can_shift`); the model itself lives in `clones.rs`.
//!
//! Persistence: one lenient `chi` save line, `elem:points` pairs.

use crate::content::Ability;
use crate::loadout::{BodyFit, Mod};
use crate::weapons::{Element, ELEMENTS};

/// Points needed to STAND at each level: a widening staircase. Level 5
/// is mastery and the table is the whole curve — no formula to misread.
pub const LEVELS: [u32; 6] = [0, 3, 8, 15, 25, 40];

/// The seven pools.
#[derive(Clone, Default)]
pub struct Chi {
    points: [u32; 7],
}

fn slot(e: Element) -> Option<usize> {
    ELEMENTS.iter().position(|x| *x == e)
}

impl Chi {
    pub fn from_line(line: &str) -> Chi {
        let mut c = Chi::default();
        for part in line.split(',') {
            let Some((name, pts)) = part.trim().split_once(':') else { continue };
            let Some(e) = ELEMENTS.iter().find(|e| e.name() == name.trim()) else { continue };
            if let Ok(p) = pts.trim().parse() {
                c.points[slot(*e).unwrap()] = p;
            }
        }
        c
    }
    /// Canonical: only non-zero pools, ring order.
    pub fn line(&self) -> String {
        ELEMENTS
            .iter()
            .filter(|e| self.points[slot(**e).unwrap()] > 0)
            .map(|e| format!("{}:{}", e.name(), self.points[slot(*e).unwrap()]))
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn points(&self, e: Element) -> u32 {
        slot(e).map(|i| self.points[i]).unwrap_or(0)
    }
    pub fn bank(&mut self, e: Element, pts: u32) {
        if let Some(i) = slot(e) {
            self.points[i] += pts;
        }
    }
    /// Level from the staircase: the highest step the points reach.
    pub fn level(&self, e: Element) -> u8 {
        let p = self.points(e);
        LEVELS.iter().rposition(|t| p >= *t).unwrap_or(0) as u8
    }

    /// The shapeshifter's threshold: holding the ice-clone form is an
    /// ice discipline, and it is not an opener.
    pub fn can_shift(&self) -> bool {
        self.level(Element::Ice) >= 3
    }

    /// The Watch attunes faster toward elements the body has earned:
    /// 8% off the delay per level, floored — mastery still takes a
    /// beat, because the beat is the cost that stops a hot-swap meta.
    pub fn attune_mul(&self, toward: Element) -> f32 {
        (1.0 - 0.08 * self.level(toward) as f32).max(0.60)
    }

    /// How many of the seven pools stand at mastery (level 5). Not tied
    /// to Photon Man — `watch.rs` owns that gate, and its own doc is
    /// explicit that Photon Man is reached by a declared last resort at
    /// resolve cost, not by mastering anything ("not gated by having
    /// found anything... it is gated by a caller-declared moment of
    /// last resort"). An earlier version of this doc comment claimed
    /// the opposite — that mastering all seven opened the ceiling —
    /// written before `watch.rs` was found in a fuller read of the
    /// codebase. That was wrong and has been removed rather than left
    /// to disagree with the module that actually owns the mechanic.
    /// `mastered()` stays as a plain completion count: how much of the
    /// Watch's seven pools a save has topped out, useful for a status
    /// screen or an achievement, nothing more.
    pub fn mastered(&self) -> u8 {
        ELEMENTS.iter().filter(|e| self.level(**e) == 5).count() as u8
    }
}

/// Which element GOVERNS a mod, read off what the mod does: heat gear
/// answers to Fire, cold gear to Ice, shielding to Rock (mass is mass),
/// endurance to Vine (the sustaining element), mobility to Air.
pub fn governor(m: &Mod) -> Element {
    use crate::loadout::Effect;
    match m.effect {
        Effect::HeatCeiling => Element::Fire,
        Effect::ColdFloor => Element::Ice,
        Effect::Shielding => Element::Rock,
        Effect::Endurance => Element::Vine,
        Effect::Mobility => Element::Air,
    }
}

/// The level a mod demands: its slot weight, straight across. A 1-slot
/// genetic tweak asks level 1; the 3-slot LEAD-WEAVE SHELL asks Rock 3.
/// Machine-fit mods ask nothing of chi — plating is bolted, not earned.
pub fn required(m: &Mod) -> u8 {
    if m.fits == BodyFit::Machine {
        0
    } else {
        m.slots
    }
}

/// The worthiness gate, with the reason on the outside: Err carries the
/// line the gear menu prints.
pub fn worthy(chi: &Chi, m: &Mod) -> Result<(), String> {
    let need = required(m);
    if need == 0 {
        return Ok(());
    }
    let gov = governor(m);
    let have = chi.level(gov);
    if have >= need {
        Ok(())
    } else {
        Err(format!("unworthy: {} asks {} level {}, the body holds {}", m.name, gov.name(), need, have))
    }
}

/// What a deep site trains, for the suited-raid chi bank. The mapping
/// follows each arena's weather and premise in `raidcon.rs`; patrol
/// sites train the generalist's element.
pub fn arena_element(arena: &str) -> Element {
    match arena {
        "frostline" | "kuiper" | "pluto" => Element::Ice,
        "emberpit" | "unicron" => Element::Fire,
        "trench" => Element::Water,
        "shield_edge" => Element::Electric,
        "outer_rim" => Element::Air,
        "cybertron" => Element::Rock,
        _ => Element::Water,
    }
}

/// What one clear banks.
pub const PER_NIGHTFALL: u32 = 2;
pub const PER_SUITED_RAID: u32 = 3;

/// One line for a status screen: leveled pools only.
pub fn sheet(chi: &Chi) -> String {
    let mut parts: Vec<String> = ELEMENTS
        .iter()
        .filter(|e| chi.points(**e) > 0)
        .map(|e| format!("{} L{} ({})", e.name(), chi.level(*e), chi.points(*e)))
        .collect();
    if parts.is_empty() {
        parts.push("nothing yet -- the body is a stranger to all seven".into());
    }
    parts.join(", ")
}

/// The same read, short enough for a console row.
///
/// `sheet` is written for the wide `--gear` view and runs to a sentence: an
/// unattuned body reads "nothing yet -- the body is a stranger to all seven",
/// 50 characters before the caller's own prefix. The station console gives 46
/// TOTAL, so the medbay heading was truncated on every fresh save — and would
/// have overflowed again at three attuned elements even after the body was no
/// longer a stranger.
///
/// Caps at `max` characters, hard: the strongest few by points, then a count.
pub fn sheet_short(chi: &Chi, max: usize) -> String {
    let mut v: Vec<(Element, u32)> =
        ELEMENTS.iter().map(|e| (*e, chi.points(*e))).filter(|(_, p)| *p > 0).collect();
    if v.is_empty() {
        return "none yet".into();
    }
    // strongest first, so what gets dropped is what matters least
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name().cmp(b.0.name())));
    let mut out = String::new();
    let mut shown = 0;
    for (e, _) in &v {
        let piece = format!("{} L{}", e.name(), chi.level(*e));
        let extra = v.len() - shown - 1;
        // leave room for the "+N" tail if anything would be left over
        let tail = if extra > 0 { format!(" +{extra}").len() } else { 0 };
        let sep = if out.is_empty() { 0 } else { 1 };
        if out.len() + sep + piece.len() + tail > max {
            break;
        }
        if sep > 0 {
            out.push(' ');
        }
        out.push_str(&piece);
        shown += 1;
    }
    if shown == 0 {
        // Even one entry does not fit. The fallback has to respect the cap
        // too — "2 attuned" is nine characters and was being returned under a
        // cap of eight, so the one branch whose job was to guarantee the
        // bound was the branch that broke it.
        let long = format!("{} attuned", v.len());
        let short = format!("x{}", v.len());
        return if long.len() <= max {
            long
        } else if short.len() <= max {
            short
        } else {
            String::new()
        };
    }
    if shown < v.len() {
        out.push_str(&format!(" +{}", v.len() - shown));
    }
    out
}

/// The abilities module already names actives; expose the chi read the
/// shell will want next to each: does the bearer's discipline discount
/// this ability's element? (Abilities have no element today, so this is
/// the seam, stated: it returns the watch discount for the element the
/// CALLER says the ability channels.)
pub fn ability_mul(chi: &Chi, _a: &Ability, channels: Element) -> f32 {
    chi.attune_mul(channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loadout::MODS;

    #[test]
    fn the_line_roundtrips_and_tolerates_junk() {
        let mut c = Chi::from_line("ice:9, fire:2, plasma:9, rock:abc,,vine:1");
        assert_eq!(c.points(Element::Ice), 9);
        assert_eq!(c.points(Element::Fire), 2);
        assert_eq!(c.points(Element::Vine), 1);
        assert_eq!(c.points(Element::Rock), 0, "junk points banked");
        c.bank(Element::Ice, 1);
        let back = Chi::from_line(&c.line());
        assert_eq!(back.points(Element::Ice), 10);
        assert_eq!(back.line(), c.line());
    }

    #[test]
    fn levels_climb_the_staircase() {
        let mut c = Chi::default();
        assert_eq!(c.level(Element::Ice), 0);
        c.bank(Element::Ice, 3);
        assert_eq!(c.level(Element::Ice), 1);
        c.bank(Element::Ice, 37); // 40 total
        assert_eq!(c.level(Element::Ice), 5);
        assert!(c.can_shift());
        assert!(!Chi::default().can_shift());
        // the watch discount follows, floored
        assert!((c.attune_mul(Element::Ice) - 0.60).abs() < 1e-6);
        assert!((c.attune_mul(Element::Fire) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn worthiness_gates_the_heavy_human_kit_and_ignores_plating() {
        let chi = Chi::from_line("rock:8"); // level 2
        let shell = MODS.iter().find(|m| m.name == "LEAD-WEAVE SHELL").unwrap();
        assert!(worthy(&chi, shell).is_err(), "a level-2 body wore the 3-slot shell");
        let earned = Chi::from_line("rock:15"); // level 3
        assert!(worthy(&earned, shell).is_ok());
        // machine plating never asks
        let plating = MODS.iter().find(|m| m.name == "ABLATIVE PLATING").unwrap();
        assert!(worthy(&Chi::default(), plating).is_ok());
        // every human mod has a governor and a nonzero ask
        for m in MODS.iter().filter(|m| m.fits != BodyFit::Machine) {
            assert!(required(m) >= 1, "{} asks nothing of the body", m.name);
        }
    }

    #[test]
    fn mastered_counts_the_pools_at_the_top_of_the_staircase() {
        let mut c = Chi::default();
        assert_eq!(c.mastered(), 0);
        for e in ELEMENTS {
            c.bank(e, 40); // top of the staircase, every pool
        }
        assert_eq!(c.mastered(), 7);
        let mut some = Chi::default();
        some.bank(Element::Fire, 40);
        some.bank(Element::Ice, 15); // level 3, not 5
        assert_eq!(some.mastered(), 1);
    }

    #[test]
    fn every_arena_trains_something() {
        for a in crate::raidcon::ARENAS {
            let _ = arena_element(a.id); // total over the roster's arenas
        }
        assert_eq!(arena_element("frostline"), Element::Ice);
        assert_eq!(arena_element("unicron"), Element::Fire);
    }
}

#[cfg(test)]
mod short_tests {
    use super::*;

    #[test]
    fn it_never_exceeds_the_cap_however_attuned_the_body_is() {
        // the property that matters: the console has a hard budget, and a
        // body can be attuned to all seven elements at any level
        for max in [8usize, 16, 28, 40] {
            let mut c = Chi::default();
            assert!(sheet_short(&c, max).len() <= max, "empty overflowed at {max}");
            for (i, e) in ELEMENTS.iter().enumerate() {
                c.bank(*e, 10 + i as u32 * 7);
                let s = sheet_short(&c, max);
                assert!(
                    s.len() <= max,
                    "{} element(s) gave {:?} ({} chars) at cap {max}",
                    i + 1,
                    s,
                    s.len()
                );
            }
        }
    }

    #[test]
    fn an_unattuned_body_says_so_briefly() {
        assert_eq!(sheet_short(&Chi::default(), 28), "none yet");
        // and the long form is left alone for the wide view
        assert!(sheet(&Chi::default()).len() > 40);
    }

    #[test]
    fn what_gets_dropped_is_what_matters_least() {
        let mut c = Chi::default();
        c.bank(Element::Rock, 5);
        c.bank(Element::Fire, 90);
        let s = sheet_short(&c, 12);
        assert!(s.contains("fire"), "the strongest attunement was dropped: {s:?}");
        assert!(s.contains('+') || !s.contains("rock"), "{s:?}");
    }

    #[test]
    fn the_count_of_hidden_entries_is_honest() {
        let mut c = Chi::default();
        for (i, e) in ELEMENTS.iter().enumerate() {
            c.bank(*e, 10 + i as u32);
        }
        let s = sheet_short(&c, 28);
        // seven attuned; whatever is shown, the tail must account for the rest
        if let Some(idx) = s.rfind('+') {
            let hidden: usize = s[idx + 1..].trim().parse().expect("a number after +");
            let shown = s[..idx].split_whitespace().filter(|w| w.starts_with('L')).count();
            assert_eq!(shown + hidden, ELEMENTS.len(), "the tail miscounts: {s:?}");
        }
    }
}

#[cfg(test)]
mod arena_join_tests {
    use super::*;
    use crate::patrol::ROUTE;
    use crate::raidcon::BOSSES;

    /// The arena names `arena_element` matches on, read out of its own source.
    fn mapped() -> Vec<String> {
        let src = include_str!("chi.rs");
        let body = src
            .split("pub fn arena_element(arena: &str) -> Element {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("arena_element");
        let mut out = Vec::new();
        for line in body.lines() {
            let Some((pats, _)) = line.split_once("=>") else { continue };
            for p in pats.split('|') {
                let p = p.trim().trim_matches(|c| c == '"' || c == ' ');
                if !p.is_empty() && p != "_" {
                    out.push(p.to_string());
                }
            }
        }
        out
    }

    /// THE JOIN NOBODY CHECKED. `Boss.arena` is a free string; `arena_element`
    /// matches on it and falls through to Water. So a typo — "frostlien" —
    /// compiles, runs, and silently trains the wrong element for the whole
    /// raid. Nothing anywhere connects the two tables.
    ///
    /// The rule that makes the fallback safe rather than a hole: an arena is
    /// either explicitly mapped, or it is a real patrol destination and the
    /// fallback is deliberate ("patrol sites train the generalist's element",
    /// per the doc). Anything else is a name that exists nowhere.
    #[test]
    fn every_raid_arena_is_mapped_or_is_a_real_destination() {
        let m = mapped();
        for b in BOSSES {
            let known = m.iter().any(|k| k == b.arena)
                || ROUTE.iter().any(|d| d.id == b.arena);
            assert!(
                known,
                "boss '{}' fights at '{}', which is neither mapped by \
                 chi::arena_element nor a patrol destination — it trains Water \
                 by accident",
                b.id, b.arena
            );
        }
    }

    #[test]
    fn no_arena_mapping_is_dead() {
        // the other direction: a mapped name nothing fights at is either a
        // typo or a boss that got removed, and both are worth knowing
        let used: std::collections::BTreeSet<&str> = BOSSES.iter().map(|b| b.arena).collect();
        for k in mapped() {
            assert!(
                used.contains(k.as_str()) || ROUTE.iter().any(|d| d.id == k),
                "chi::arena_element maps '{k}', which no boss fights at and which \
                 is not a destination either"
            );
        }
    }

    #[test]
    fn the_fallback_is_reachable_and_is_a_real_choice() {
        // `mars` is an arena that deliberately falls through. If that ever
        // stops being true the fallback becomes dead code pretending to be a
        // policy.
        let m = mapped();
        let falls_through: Vec<&str> =
            BOSSES.iter().map(|b| b.arena).filter(|a| !m.iter().any(|k| k == a)).collect();
        assert!(
            !falls_through.is_empty(),
            "nothing uses the generalist fallback any more; either map it \
             explicitly or delete the arm"
        );
        for a in falls_through {
            assert_eq!(arena_element(a), Element::Water);
        }
    }
}
