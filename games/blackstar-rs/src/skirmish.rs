//! SKIRMISH — squads actually fighting, headless and deterministic.
//!
//! Everything a fight needs has been sitting in the content tables for a
//! while: chassis carry bulk, elements carry damage/range/mobility and an
//! advantage ring, abilities carry perks, commanders carry doctrine — and,
//! as the content tests put it, `Element::advantage` was "implemented,
//! tested, and called by nothing." This module is the caller.
//!
//! It is a one-dimensional resolver on purpose: two squads on a range line,
//! closing, standing off, breaking away — the shape of a fight without the
//! geometry of one. That is enough for the campaign and raid runtimes to
//! get real kills, real durations and real losses out of real rosters, and
//! it is the model the shell's 3D combat must eventually agree with, the
//! same way `raidrun` is the contract for the raid renderer.
//!
//! Determinism: no RNG anywhere. Ties break by roster index. The same two
//! squads fight the same fight every time, which is what makes a balance
//! change a diff instead of an anecdote.
//!
//! What the commander dials DO here (each one a lever, not a label):
//! * `aggression` — closing speed, and focus fire: high aggression
//!   concentrates the squad on the weakest target; low spreads shots.
//! * `caution` — standoff distance and the break-off threshold: a cautious
//!   fighter disengages when hurt instead of dying where it stands.
//! * `encircle` — cutting off: bonus damage against targets that are
//!   currently breaking away, because the ring is already behind them.
//! * `regroup` — how quickly a broken-off fighter returns to the line.
//!
//! Active abilities (turret, pulse, blink...) are deliberately not modelled
//! yet — they are moment-to-moment decisions and belong to the layer that
//! has moments. Perks are: they are always on.

use crate::content::{ability, chassis, commander, troop, AbilityKind, Commander, Troop};
use crate::weapons::Element;

// ------------------------------------------------------------ fighters ---

/// One combatant, compiled from the content tables.
#[derive(Clone)]
pub struct Fighter {
    pub troop: &'static Troop,
    pub hp: f32,
    pub hp_max: f32,
    /// damage per second before matchup multipliers
    pub dps: f32,
    /// metres at which it can fight
    pub range: f32,
    /// metres per second on the line
    pub speed: f32,
    /// current distance to the shared firing line (1-D)
    pub at: f32,
    /// seconds left disengaged, 0 = on the line
    pub broken: f32,
    /// has already broken and come back this fight: a fighter rallies
    /// ONCE — the second time the line would fold, it holds instead,
    /// because an endless break-retreat loop is a stalemate, not a war
    pub rallied: bool,
    /// ELEMENTAL SHIELD, the coalition races' answer to raw power: a pool
    /// over the hp bar that only the ring meaningfully opens. Advantaged
    /// damage melts it, neutral chips it, disadvantaged pings off it —
    /// which is what turns "bring the counter element" from a 50% bonus
    /// into the difference between a fight and a wall.
    pub shield: f32,
}

/// The compile step: chassis gives body, element gives gun and gait, perks
/// multiply, and ACTIVES count: a fighter with buttons fights above its
/// baseline, modelled here as sustained output because this layer has no
/// moments to press them in. That is the player classes' edge over the
/// line troops, and it is derived from the content tables rather than
/// granted — a grunt has no buttons, so a grunt gets no edge.
pub fn fighter(id: &str, start_at: f32) -> Option<Fighter> {
    let t = troop(id)?;
    let c = chassis(t.chassis)?;
    let e = t.element.profile();
    let mut hp = 90.0 * c.bulk + 30.0 * c.arm;
    let mut dps = 11.0 * e.damage / e.cooldown;
    let mut speed = 7.0 * e.move_mul * c.leg;
    // THE HERO TIER. A player class is a named transformer against line
    // troops: worth roughly three of them (1.8 x 1.6 ~ 2.9 effective).
    // One visible constant instead of a hidden campaign-side fudge, so
    // "why did the squad win 3-against-9" has a one-line answer — and so
    // a champion (a variant with real abilities) still lands between a
    // grunt and a hero, where the fiction puts it.
    if t.role == crate::content::Role::PlayerClass {
        hp *= 1.8;
        dps *= 1.6;
    }
    // the coalition races fight behind elemental shields — the word is
    // load-bearing: a shield is elemental, so a Variant with no element
    // (the Corrupted apex, `Element::None` — the treatment never gives
    // them a ring position; see `corruption.rs`) does not get one. It
    // already carries the evasion `Fighter::hit` applies below; stacking
    // an elemental shield on top of that on a body the fiction never
    // calls a coalition race broke the campaign's own winnability
    // promise (caught by a test, not eyeballed).
    let shield = if t.role == crate::content::Role::Variant && t.element != Element::None {
        SHIELD_HP
    } else {
        0.0
    };
    for a in t.abilities {
        let Some(ab) = ability(a) else { continue };
        match ab.kind {
            // an active is worth a steady 15% — the time-average of a
            // button pressed on cooldown
            AbilityKind::Active => dps *= 1.15,
            AbilityKind::Perk => match ab.effect {
                "deploy_armor" => hp += ab.value,
                "move_mul" => speed *= ab.value,
                // fire_mul scales the COOLDOWN: below one is faster
                "fire_mul" => dps /= ab.value,
                _ => {}
            },
        }
    }
    Some(Fighter {
        troop: t,
        hp,
        hp_max: hp,
        dps,
        range: 26.0 * e.range,
        speed,
        at: start_at,
        broken: 0.0,
        rallied: false,
        shield,
    })
}

/// Shield pool for a variant, and what the ring does to it.
pub const SHIELD_HP: f32 = 55.0;
fn shield_mult(ring: f32) -> f32 {
    if ring > 1.0 {
        1.8 // the counter element MELTS it
    } else if ring < 1.0 {
        0.2 // the wrong element pings off
    } else {
        0.5
    }
}

/// Which corruption stage a troop id is, if it is one of the Corrupted
/// (`corruption.rs`, `content.rs`'s `corrupted_*` roster) — the one
/// place this module needs to know about that roster at all.
fn corrupted_stage(id: &str) -> Option<crate::corruption::Stage> {
    use crate::corruption::Stage;
    match id {
        "corrupted_drifter" => Some(Stage::One),
        "corrupted_bloom" => Some(Stage::Two),
        "corrupted_apex" => Some(Stage::Three),
        _ => None,
    }
}

impl Fighter {
    pub fn alive(&self) -> bool {
        self.hp > 0.0
    }
    fn engaged(&self) -> bool {
        self.alive() && self.broken <= 0.0
    }
    /// Route damage through the shield first. The ring decides how much
    /// of it the shield actually feels; once the shield is down, hp takes
    /// the ring-multiplied damage as usual.
    ///
    /// A CORRUPTED target routes through one more gate first: the doc's
    /// continuous-transformation evasion (`corruption.rs`) — "weapons
    /// that target a specific component miss because that component has
    /// shifted." Applied before the shield/hp split so an apex Corrupted
    /// (a Variant, so it also carries an elemental shield) genuinely
    /// stacks both defenses, which is the fiction's own point about it
    /// being the hardest thing either side has fought.
    fn hit(&mut self, dmg: f32, ring: f32) {
        let dmg = match corrupted_stage(self.troop.id) {
            Some(stage) => dmg * (1.0 - stage.evasion()),
            None => dmg,
        };
        if self.shield > 0.0 {
            self.shield -= dmg * shield_mult(ring);
        } else {
            self.hp -= dmg * ring;
        }
    }
}

// --------------------------------------------------------------- squad ---

pub struct Squad {
    pub fighters: Vec<Fighter>,
    pub doctrine: &'static Commander,
}

impl Squad {
    /// Build from troop ids under a commander. Unknown ids are skipped —
    /// lenient like everything content-adjacent.
    pub fn new(ids: &[&str], doctrine_id: &str, start_at: f32) -> Squad {
        Squad {
            fighters: ids.iter().filter_map(|id| fighter(id, start_at)).collect(),
            doctrine: commander(doctrine_id).unwrap_or(&crate::content::COMMANDERS[0]),
        }
    }
    pub fn alive(&self) -> usize {
        self.fighters.iter().filter(|f| f.alive()).count()
    }
    pub fn hp_total(&self) -> f32 {
        self.fighters.iter().map(|f| f.hp.max(0.0)).sum()
    }
}

// --------------------------------------------------------------- fight ---

/// Who a fighter shoots, per doctrine: aggression focuses the weakest
/// living target, its absence spreads by index. Deterministic either way.
fn pick_target(shooter_i: usize, agg: f32, them: &[Fighter]) -> Option<usize> {
    let living: Vec<usize> = them.iter().enumerate().filter(|(_, f)| f.alive()).map(|(i, _)| i).collect();
    if living.is_empty() {
        return None;
    }
    if agg >= 0.5 {
        living
            .into_iter()
            .min_by(|&a, &b| them[a].hp.partial_cmp(&them[b].hp).unwrap_or(std::cmp::Ordering::Equal))
    } else {
        Some(living[shooter_i % living.len()])
    }
}

/// What happened in a tick that the caller might narrate or count.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Casualty {
    /// index into squad A's fighters
    A(usize),
    /// index into squad B's fighters
    B(usize),
}

pub struct Fight {
    pub a: Squad,
    pub b: Squad,
    pub t: f32,
    /// a NIGHTFALL burn: damage of this element runs hot for BOTH sides
    /// — the Destiny bargain, danger and opportunity in the same modifier
    pub burn: Option<Element>,
}

/// How hot a burned element runs.
pub const BURN_MULT: f32 = 2.5;

impl Fight {
    pub fn new(a: Squad, b: Squad) -> Fight {
        Fight { a, b, t: 0.0, burn: None }
    }
    pub fn with_burn(mut self, burn: Option<Element>) -> Fight {
        self.burn = burn;
        self
    }

    pub fn over(&self) -> bool {
        self.a.alive() == 0 || self.b.alive() == 0
    }

    /// One tick. Movement, then fire, then break-offs — in that fixed
    /// order, so the resolution is one readable rule rather than a race.
    ///
    /// The line model: each fighter holds a distance `at` to the shared
    /// firing line, so the gap between a shooter and its target is
    /// `shooter.at + target.at`. Fire needs the GAP inside the shooter's
    /// range — which is what makes a retreat real (the gap opens for
    /// everyone shooting you) and a chase real (they have to spend speed
    /// to close it again).
    pub fn step(&mut self, dt: f32) -> Vec<Casualty> {
        self.t += dt;
        let (a, b) = (&mut self.a, &mut self.b);
        move_side(&mut a.fighters, a.doctrine, &b.fighters, dt);
        move_side(&mut b.fighters, b.doctrine, &a.fighters, dt);
        let mut deaths = Vec::new();
        let dmg_a = volley(&a.fighters, a.doctrine, &b.fighters, self.burn, dt);
        let dmg_b = volley(&b.fighters, b.doctrine, &a.fighters, self.burn, dt);
        for (i, d, ring) in dmg_a {
            let f = &mut b.fighters[i];
            let was = f.alive();
            f.hit(d, ring);
            if was && !f.alive() {
                deaths.push(Casualty::B(i));
            }
        }
        for (i, d, ring) in dmg_b {
            let f = &mut a.fighters[i];
            let was = f.alive();
            f.hit(d, ring);
            if was && !f.alive() {
                deaths.push(Casualty::A(i));
            }
        }
        break_offs(&mut a.fighters, a.doctrine);
        break_offs(&mut b.fighters, b.doctrine);
        deaths
    }

    /// Run to a decision or a cap. Returns (a won, seconds). A cap hit
    /// counts as a win for whoever holds more total hp — a judge's
    /// decision, deterministic like everything else.
    pub fn resolve(&mut self, max_s: f32) -> (bool, f32) {
        const DT: f32 = 0.1;
        while !self.over() && self.t < max_s {
            self.step(DT);
        }
        if self.b.alive() == 0 && self.a.alive() > 0 {
            (true, self.t)
        } else if self.a.alive() == 0 {
            (false, self.t)
        } else {
            (self.a.hp_total() >= self.b.hp_total(), self.t)
        }
    }
}

/// Close or hold: each engaged fighter moves until the GAP to its target
/// sits at its preferred fighting distance — its own range, stretched by
/// caution. Broken fighters fall back instead, which opens the gap for
/// everyone shooting them; the enemy has to chase, and the chase is the
/// mitigation.
fn move_side(side: &mut [Fighter], doc: &Commander, them: &[Fighter], dt: f32) {
    for i in 0..side.len() {
        let f = &side[i];
        if !f.alive() {
            continue;
        }
        if f.broken > 0.0 {
            let f = &mut side[i];
            f.broken -= dt * (0.5 + doc.regroup);
            f.at += f.speed * dt; // falling back opens the gap
            if f.broken <= 0.0 {
                f.rallied = true; // back on the line, and staying there
            }
            continue;
        }
        let Some(ti) = pick_target(i, doc.aggression, them) else { continue };
        let gap = f.at + them[ti].at;
        let pref = f.range * (0.55 + 0.45 * doc.caution);
        let step = f.speed * dt * (0.6 + 0.7 * doc.aggression);
        if gap > pref {
            let f = &mut side[i];
            f.at = (f.at - step).max(0.0);
        }
    }
}

/// Everyone in range fires. The volley carries BASE damage plus the ring
/// it was fired on: the ring is applied at the TARGET, because a shield
/// and a hull answer it differently. Encirclement punishes the breaking;
/// a nightfall burn runs the named element hot on both sides.
fn volley(
    us: &[Fighter],
    doc: &Commander,
    them: &[Fighter],
    burn: Option<Element>,
    dt: f32,
) -> Vec<(usize, f32, f32)> {
    let mut out = Vec::new();
    for (i, f) in us.iter().enumerate() {
        if !f.engaged() {
            continue;
        }
        let Some(ti) = pick_target(i, doc.aggression, them) else { continue };
        let tgt = &them[ti];
        // the contact rule: the pair gap, inside the shooter's reach
        if f.at + tgt.at > f.range {
            continue;
        }
        let ring = f.troop.element.advantage(tgt.troop.element);
        let cut = if tgt.broken > 0.0 { 1.0 + doc.encircle * 0.5 } else { 1.0 };
        let hot = if burn.is_some() && burn == Some(f.troop.element) { BURN_MULT } else { 1.0 };
        out.push((ti, f.dps * cut * hot * dt, ring));
    }
    out
}

/// A hurt fighter under a cautious doctrine breaks off instead of dying in
/// place: it stops shooting, opens the range, and comes back on regroup.
fn break_offs(side: &mut [Fighter], doc: &Commander) {
    for f in side.iter_mut() {
        if f.alive()
            && !f.rallied
            && f.broken <= 0.0
            && f.hp < f.hp_max * (0.15 + 0.35 * doc.caution)
            && doc.caution > 0.3
        {
            f.broken = 2.0 + 4.0 * doc.caution;
        }
    }
}

/// The distance two squads usually open at.
pub const OPEN_AT: f32 = 55.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn duel(a_id: &str, a_doc: &str, b_id: &str, b_doc: &str) -> (bool, f32) {
        let a = Squad::new(&[a_id, a_id, a_id], a_doc, OPEN_AT);
        let b = Squad::new(&[b_id, b_id, b_id], b_doc, OPEN_AT);
        Fight::new(a, b).resolve(600.0)
    }

    #[test]
    fn an_elementless_variant_carries_no_elemental_shield() {
        // the apex Corrupted is Role::Variant (for the wave density
        // cap) but Element::None (the treatment gives it no ring
        // position) — it must not double-dip into the coalition's
        // elemental shield on top of its own evasion
        let apex = fighter("corrupted_apex", OPEN_AT).unwrap();
        assert_eq!(apex.shield, 0.0, "an elementless body carried an elemental shield");
        // a real coalition variant still does
        let sentinel = fighter("hoarfrost_sentinel", OPEN_AT).unwrap();
        assert_eq!(sentinel.shield, SHIELD_HP);
    }

    #[test]
    fn a_corrupted_target_actually_shrugs_off_some_incoming_damage() {
        // hit() directly, rather than a full duel: dash/other abilities
        // on the corrupted roster would otherwise confound a timing
        // comparison, and the evasion is a property of ONE hit anyway
        let mut drifter = fighter("corrupted_drifter", OPEN_AT).unwrap();
        let mut plain = fighter("grunt", OPEN_AT).unwrap();
        let (before_d, before_p) = (drifter.hp, plain.hp);
        drifter.hit(50.0, 1.0);
        plain.hit(50.0, 1.0);
        let (lost_d, lost_p) = (before_d - drifter.hp, before_p - plain.hp);
        assert!(lost_d < lost_p, "a stage-one target took the full hit: {lost_d} vs {lost_p}");
        let expected = lost_p * (1.0 - crate::corruption::Stage::One.evasion());
        assert!((lost_d - expected).abs() < 1e-3, "{lost_d} vs expected {expected}");

        // and a heavier stage shrugs off more than a lighter one
        let mut bloom = fighter("corrupted_bloom", OPEN_AT).unwrap();
        let before_b = bloom.hp;
        bloom.hit(50.0, 1.0);
        let lost_b = before_b - bloom.hp;
        assert!(lost_b < lost_d, "stage two took as much as stage one: {lost_b} vs {lost_d}");
    }

    #[test]
    fn the_element_ring_finally_has_a_caller_and_it_decides_fights() {
        // pyre is Fire; interceptor is Air. On the ring Fire hits Air at
        // 1.5 and Air hits Fire at 0.66 — near-mirrored squads, so the
        // ring is the dominant asymmetry, and it must decide the fight
        let fire_hits = Element::Fire.advantage(Element::Air);
        let air_hits = Element::Air.advantage(Element::Fire);
        assert!(fire_hits > air_hits, "the ring itself moved; retune this test");
        let (fire_won, _) = duel("pyre", "skirmisher", "interceptor", "skirmisher");
        assert!(fire_won, "the ring did not decide the near-mirrored duel");
    }

    #[test]
    fn aggression_is_tempo() {
        // identical squads: a mutually aggressive fight resolves faster
        // than a mutually cautious one, because caution stands off and
        // breaks away — the dial is pace, not power
        let mirror = |doc: &str| {
            let mut f = Fight::new(
                Squad::new(&["vanguard"; 3], doc, OPEN_AT),
                Squad::new(&["vanguard"; 3], doc, OPEN_AT),
            );
            f.resolve(600.0).1
        };
        assert!(mirror("aggressor") < mirror("warden"), "caution resolved faster than aggression");
    }

    #[test]
    fn caution_breaks_off_instead_of_dying_in_place() {
        // run a warden-led squad against an aggressor and check that a
        // hurt fighter actually disengaged at some point — the dial DOES
        // something, visibly
        let mut f = Fight::new(
            Squad::new(&["vanguard"; 3], "warden", OPEN_AT),
            Squad::new(&["vanguard"; 3], "aggressor", OPEN_AT),
        );
        let mut ever_broken = false;
        while !f.over() && f.t < 600.0 {
            f.step(0.1);
            ever_broken |= f.a.fighters.iter().any(|x| x.alive() && x.broken > 0.0);
        }
        assert!(ever_broken, "a warden squad never broke a fighter off");
        // and an aggressor never does: its threshold is below the floor
        let mut g = Fight::new(
            Squad::new(&["vanguard"; 3], "aggressor", OPEN_AT),
            Squad::new(&["vanguard"; 3], "aggressor", OPEN_AT),
        );
        let mut agg_broke = false;
        while !g.over() && g.t < 600.0 {
            g.step(0.1);
            agg_broke |= g.a.fighters.iter().any(|x| x.broken > 0.0);
        }
        assert!(!agg_broke, "an aggressor broke off; the doctrine floor moved");
    }

    #[test]
    fn encirclement_punishes_the_break_away() {
        // volley damage against a breaking target carries the cut-off
        // bonus, scaled by the doctrine's encircle dial
        let us = vec![fighter("vanguard", 10.0).unwrap()];
        let mut them = vec![fighter("vanguard", 10.0).unwrap()];
        let doc = commander("surrounder").unwrap(); // encircle 0.9
        let held = volley(&us, doc, &them, None, 1.0)[0].1;
        them[0].broken = 3.0;
        let cut = volley(&us, doc, &them, None, 1.0)[0].1;
        assert!(cut > held * 1.4, "the ring closed and nothing happened: {held} -> {cut}");
    }

    #[test]
    fn a_shield_perk_is_worth_real_hp() {
        let plain = fighter("grunt", OPEN_AT).unwrap();
        let shielded = fighter("ravager", OPEN_AT).unwrap();
        // ravager is bruiser + shield; grunt is standard + nothing
        assert!(shielded.hp_max > plain.hp_max + 40.0 * 0.9, "the shield perk went missing");
    }

    #[test]
    fn a_shield_only_answers_to_the_ring() {
        // ice melts a fire variant's shield; electric pings off it. Same
        // shield, ~9x difference in how long it holds — the D1 lesson:
        // the counter element is the key, not a bonus
        let melt = shield_mult(Element::Ice.advantage(Element::Fire));
        let ping = shield_mult(Element::Electric.advantage(Element::Fire));
        assert!(melt / ping >= 8.0, "the shield stopped caring about the ring: {melt} vs {ping}");
        // and a variant actually spawns with one; a grunt does not
        assert!(fighter("magma_breaker", 0.0).unwrap().shield > 0.0);
        assert_eq!(fighter("grunt", 0.0).unwrap().shield, 0.0);
    }

    #[test]
    fn a_burn_runs_the_named_element_hot_for_both_sides() {
        // pyre (Fire) vs pyre (Fire) under a Fire burn: the mirror stays
        // a mirror, but it resolves much faster — danger AND opportunity
        let mirror = |burn: Option<Element>| {
            let mut f = Fight::new(
                Squad::new(&["pyre"; 3], "aggressor", OPEN_AT),
                Squad::new(&["pyre"; 3], "aggressor", OPEN_AT),
            )
            .with_burn(burn);
            f.resolve(600.0).1
        };
        let cold = mirror(None);
        let hot = mirror(Some(Element::Fire));
        assert!(hot < cold * 0.7, "the burn changed nothing: {cold:.1}s -> {hot:.1}s");
        // an unrelated burn changes nothing at all
        assert_eq!(mirror(Some(Element::Vine)), cold);
    }

    #[test]
    fn fights_are_deterministic() {
        let run = || {
            let a = Squad::new(&["titan", "interceptor", "specialist"], "surrounder", OPEN_AT);
            let b = Squad::new(&["ravager", "slipstream", "grunt"], "aggressor", OPEN_AT);
            let mut f = Fight::new(a, b);
            let mut deaths = Vec::new();
            while !f.over() && f.t < 600.0 {
                deaths.extend(f.step(0.1));
            }
            (deaths, f.a.alive(), f.b.alive(), (f.t * 10.0).round() as i64)
        };
        assert_eq!(run().0, run().0);
        let (a, b) = (run(), run());
        assert_eq!((a.1, a.2, a.3), (b.1, b.2, b.3));
    }

    #[test]
    fn variants_outclass_grunts_and_the_ring_decides_who_pays() {
        // body for body, a coalition variant is worth more than a grunt —
        // that is what makes trilogy two's pools an escalation at all
        let g = fighter("grunt", 0.0).unwrap();
        for id in ["ashfall_lancer", "hoarfrost_sentinel", "magma_breaker"] {
            let v = fighter(id, 0.0).unwrap();
            assert!(v.hp_max * v.dps > g.hp_max * g.dps, "{id} is not worth more than a grunt");
        }
        // and the waves themselves stay beatable by heroes — variants
        // escalate the price, not the possibility. (Deliberately NO claim
        // here about which composition pays more: range and speed confound
        // it — fire is short-range glass that pays a crossing toll against
        // long guns, and that is the profile table working, not a bug.
        // The ring's own effect is isolated by the near-mirror duel above.)
        let (won, _) = duel_sq(&["titan"; 3], &["grunt"; 3]);
        assert!(won);
        let (won2, hp_left) =
            duel_sq(&["titan"; 3], &["magma_breaker", "ashfall_lancer", "magma_breaker"]);
        assert!(won2, "heroes should still take a fire wave");
        assert!(hp_left > 0.0);
    }

    fn duel_sq(a: &[&str], b: &[&str]) -> (bool, f32) {
        let mut f = Fight::new(Squad::new(a, "skirmisher", OPEN_AT), Squad::new(b, "aggressor", OPEN_AT));
        let (won, _) = f.resolve(600.0);
        (won, f.a.hp_total())
    }
}
