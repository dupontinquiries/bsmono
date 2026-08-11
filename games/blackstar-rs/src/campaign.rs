//! CAMPAIGN — the fifteen missions, runnable.
//!
//! `content::MISSIONS` has been authored data with no consumer: beats,
//! sites, objectives, champions, slides — and nothing that could play one.
//! This is the runtime. A mission is waves of real troops resolved by
//! `skirmish`, under an OBJECTIVE MACHINE that decides what winning means:
//!
//! * **Eliminate** — clear every wave; the champion leads the last one.
//! * **Survive** — the clock is the objective; the waves do not stop, and
//!   they do not need to be beaten, only outlived.
//! * **Breach** — a structure with hit points; hostiles interrupt the
//!   demolition, so the fight is for UNCONTESTED SECONDS.
//! * **Control** — a zone that fills while the field is clear and bleeds
//!   while it is not: the same currency as Breach, banked differently.
//! * **Protect** — an asset that every living hostile chips; the mission
//!   is lost the moment it dies, whatever else was going well.
//!
//! Wave composition is deterministic per mission (seeded off the mission
//! id, no external seed): a mission is authored, so its opposition is a
//! property of the mission, not of the evening. Fights themselves are
//! deterministic too, which makes "every mission is winnable by the
//! default squad" a TEST rather than a hope — the promise this module
//! exists to keep.
//!
//! Progression mirrors the raid log: cleared mission ids ride the save
//! file as one lenient `missions` line, and finishing the campaign is
//! what turns the nineteen warden signals from rumours into a roster.

use crate::content::{hostiles, missions_of, Mission, Objective, Role, MISSIONS};
use crate::energon;
use crate::patrol::{self, Faction};
use crate::raidgen::{strhash, Rng};
use crate::skirmish::{Casualty, Fight, Squad, OPEN_AT};
use crate::weapons::Element;

// --------------------------------------------------------------- waves ---

/// The opposition, wave by wave. Deterministic per mission: early films
/// are grunt lines with a raider stiffener; the middle films bring the
/// warband proper; trilogy two fields the elemental variants. The champion
/// — when the mission names one — leads the final wave.
///
/// The destination's own faction (`patrol::Destination.hostiles`) is now
/// load-bearing rather than flavor: a mission sited on a Corrupted world
/// pulls from the Corrupted roster first, same as the treatment's own
/// Film 2 logic — Jhiaxus' experiment does not politely wait for the
/// Hijack war to make room for it (see `corruption.rs`).
pub fn waves_for(m: &Mission) -> Vec<Vec<&'static str>> {
    let mut rng = Rng::new(strhash(m.id));
    let film = m.film as usize;
    let n_waves = 3 + film / 2; // 3,4,4,5,5,6 across the films
    let site_faction = patrol::find(m.dest).map(|d| d.hostiles).unwrap_or(Faction::None);
    let pool: Vec<&'static str> = hostiles()
        .filter(|t| match t.role {
            Role::Variant => film >= 4,
            _ => true,
        })
        .filter(|t| film >= 2 || matches!(t.id, "grunt" | "blackstar_raider"))
        // the champion is the FINALE's entrance, never a wave filler
        .filter(|t| Some(t.id) != m.champion)
        .map(|t| t.id)
        .collect();
    // prefer the site's own faction, but never starve a wave over it —
    // early films keep the grunt/raider-only gate above, and some
    // faction rosters are thin or elite-only (Coalition sites, for
    // instance, currently name nothing but Variant troops), so a
    // strict filter would occasionally empty the line-troop pool below
    let on_site: Vec<&'static str> = pool
        .iter()
        .copied()
        .filter(|id| crate::content::troop(id).map(|t| t.faction == site_faction).unwrap_or(false))
        .collect();
    let preferred = if on_site.is_empty() { &pool } else { &on_site };
    let is_variant = |id: &str| {
        crate::content::troop(id).map(|t| t.role == Role::Variant).unwrap_or(false)
    };
    // line troops for the shield-density substitution: prefer the
    // on-site roster's own line troops, but an all-elite site (or an
    // all-elite Corrupted apex encounter) falls back to the wider pool
    // rather than leaving nothing to substitute in
    let line_pool: Vec<&'static str> = {
        let narrow: Vec<&'static str> = preferred.iter().copied().filter(|id| !is_variant(id)).collect();
        if narrow.is_empty() {
            pool.iter().copied().filter(|id| !is_variant(id)).collect()
        } else {
            narrow
        }
    };
    let pool = preferred.clone();
    (0..n_waves)
        .map(|w| {
            let size = 2 + w.min(2) + usize::from(film >= 5);
            let mut shields = 0;
            let mut wave: Vec<&'static str> = (0..size)
                .map(|k| {
                    // grunts pad the front of every wave; the pool fills
                    // the back, harder picks later
                    if k == 0 && film <= 4 {
                        return "grunt";
                    }
                    let pick = *rng.pick(&pool);
                    // SHIELD DENSITY CAP: the coalition races are elites,
                    // and two per wave is a fight — three is a lottery
                    // loss. A third draw substitutes line troops, same
                    // rng stream, so determinism holds.
                    if is_variant(pick) {
                        if shields >= 2 {
                            return *rng.pick(&line_pool);
                        }
                        shields += 1;
                    }
                    pick
                })
                .collect();
            if w + 1 == n_waves {
                if let Some(c) = m.champion {
                    // the champion LEADS the last wave — it takes a slot
                    // rather than adding one, so naming a champion changes
                    // the wave's character, not its size
                    *wave.last_mut().unwrap() = c;
                }
            }
            wave
        })
        .collect()
}

// ------------------------------------------------------------- mission ---

/// What a tick of the mission produced, for the narrator and the tests.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CampEvent {
    WaveStart { wave: usize, of: usize, size: usize },
    /// an enemy fell (index within the wave)
    EnemyDown { wave_slot: usize },
    /// one of ours fell
    SquadDown { slot: usize },
    WaveCleared { wave: usize },
    /// Breach: demolition progress while the field is clear
    StructureHit { hp_left: f32 },
    /// Protect: the asset took fire from the living
    AssetHit { hp_left: f32 },
    /// Control: the zone filled or bled
    Hold { pct: f32 },
    Won,
    Lost,
}

// The campaign's answering curve used to live here as RENOWN (a flat 4%
// per mission cleared). It is now ENERGON — see `energon.rs` — because
// the same idea grew more sources (wardens, nightfalls) and a
// recommendation per film to scale against; one power scalar, one
// module that owns it, named for what actually fills a Cybertronian.

/// The five machines share one chassis: waves feed a `skirmish::Fight`,
/// the objective decides what the fighting was FOR.
pub struct MissionRun {
    pub mission: &'static Mission,
    waves: Vec<Vec<&'static str>>,
    wave_i: usize,
    fight: Option<Fight>,
    /// the squad persists across waves — attrition is real — and gets a
    /// breather's worth of field repair between them
    squad_ids: Vec<String>,
    doctrine: &'static str,
    /// the energon multiplier, resolved once against the mission's
    /// recommendation — the player side scales, the yardstick does not
    power: f32,
    /// nightfall dials: an element that runs hot, and whether the field
    /// repair between waves is switched off this week
    burn: Option<Element>,
    exposure: bool,
    carried: Option<Squad>,
    breather: f32,
    pub t: f32,
    // objective state, one field each, unused ones idle at zero
    pub struct_hp: f32,
    pub asset_hp: f32,
    pub hold_pct: f32,
    pub survive_left: f32,
    outcome: Option<bool>,
}

/// Seconds of breather between waves, and the field repair it buys.
const BREATHER_S: f32 = 5.0;
const REPAIR_PER_S: f32 = 9.0;
/// Breach demolition: squad damage lands on the structure only while the
/// field is clear.
const DEMO_RATE: f32 = 14.0;
/// Control: full from empty in 45 clear seconds; contested bleed.
const HOLD_RISE: f32 = 100.0 / 45.0;
const HOLD_BLEED: f32 = 3.0;
/// Protect: what each living hostile costs the asset per second.
const ASSET_CHIP: f32 = 1.5;

impl MissionRun {
    pub fn new(mission: &'static Mission, squad: &[&str], doctrine: &'static str, at_energon: u32) -> MissionRun {
        let film = mission.film as f32;
        MissionRun {
            mission,
            waves: waves_for(mission),
            wave_i: 0,
            fight: None,
            squad_ids: squad.iter().map(|s| s.to_string()).collect(),
            doctrine,
            power: energon::scale(at_energon, energon::rec_energon(mission)),
            burn: None,
            exposure: false,
            carried: None,
            breather: 0.0,
            t: 0.0,
            struct_hp: 320.0 + 90.0 * film,
            asset_hp: 420.0 + 110.0 * film,
            hold_pct: 0.0,
            survive_left: 75.0 + 15.0 * film,
            outcome: None,
        }
    }

    /// Nightfall dress: an element burn and, some weeks, no field repair.
    pub fn with_modifiers(mut self, burn: Option<Element>, exposure: bool) -> MissionRun {
        self.burn = burn;
        self.exposure = exposure;
        self
    }

    pub fn outcome(&self) -> Option<bool> {
        self.outcome
    }
    pub fn waves_total(&self) -> usize {
        self.waves.len()
    }
    pub fn squad_alive(&self) -> usize {
        match (&self.fight, &self.carried) {
            (Some(f), _) => f.a.alive(),
            (_, Some(s)) => s.alive(),
            _ => self.squad_ids.len(),
        }
    }

    fn spawn_wave(&mut self, ev: &mut Vec<CampEvent>) {
        // Survive cycles its waves forever; everything else runs the list
        let idx = if self.mission.objective == Objective::Survive {
            self.wave_i % self.waves.len()
        } else {
            self.wave_i
        };
        let comp = &self.waves[idx];
        let mut them = Squad::new(&comp.iter().map(|s| *s).collect::<Vec<_>>(), foe_doctrine(self.mission), OPEN_AT);
        // a big wave POURS IN rather than arriving as a wall: everyone
        // past the third starts deeper and walks up, so the squad fights
        // a line it can hold instead of a crowd it cannot
        for (i, f) in them.fighters.iter_mut().enumerate() {
            if i > 2 {
                f.at = OPEN_AT + 28.0 * (i as f32 - 2.0);
            }
        }
        let us = self.carried.take().unwrap_or_else(|| {
            let mut s = Squad::new(
                &self.squad_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                self.doctrine,
                OPEN_AT,
            );
            // energon lands once, at first muster; the carried squad
            // keeps it wave to wave
            for f in s.fighters.iter_mut() {
                f.hp *= self.power;
                f.hp_max *= self.power;
                f.dps *= self.power;
            }
            s
        });
        ev.push(CampEvent::WaveStart { wave: self.wave_i + 1, of: self.waves.len(), size: comp.len() });
        self.fight = Some(Fight::new(reset_line(us), them).with_burn(self.burn));
    }

    fn waves_exhausted(&self) -> bool {
        self.mission.objective != Objective::Survive && self.wave_i >= self.waves.len()
    }

    /// One tick. The chassis runs the wave schedule; the objective decides
    /// when it is over.
    pub fn step(&mut self, dt: f32) -> Vec<CampEvent> {
        let mut ev = Vec::new();
        if self.outcome.is_some() {
            return ev;
        }
        self.t += dt;

        // the wave schedule
        if self.fight.is_none() && !self.waves_exhausted() {
            if self.breather > 0.0 {
                self.breather -= dt;
                if !self.exposure {
                    if let Some(s) = &mut self.carried {
                        for f in s.fighters.iter_mut().filter(|f| f.alive()) {
                            f.hp = (f.hp + REPAIR_PER_S * dt).min(f.hp_max);
                        }
                    }
                }
            } else {
                self.spawn_wave(&mut ev);
            }
        }
        let mut contested = 0usize;
        if let Some(f) = &mut self.fight {
            for c in f.step(dt) {
                match c {
                    Casualty::A(i) => ev.push(CampEvent::SquadDown { slot: i }),
                    Casualty::B(i) => ev.push(CampEvent::EnemyDown { wave_slot: i }),
                }
            }
            contested = f.b.alive();
            if f.a.alive() == 0 {
                self.outcome = Some(false);
                ev.push(CampEvent::Lost);
                return ev;
            }
            if contested == 0 {
                ev.push(CampEvent::WaveCleared { wave: self.wave_i + 1 });
                self.wave_i += 1;
                self.breather = BREATHER_S;
                let done = self.fight.take().unwrap();
                self.carried = Some(done.a);
            }
        }

        // the objective machines
        match self.mission.objective {
            Objective::Eliminate => {
                if self.waves_exhausted() && self.fight.is_none() {
                    self.win(&mut ev);
                }
            }
            Objective::Survive => {
                self.survive_left -= dt;
                if self.survive_left <= 0.0 {
                    self.win(&mut ev);
                }
            }
            Objective::Breach => {
                if contested == 0 && self.fight.is_none() {
                    self.struct_hp -= DEMO_RATE * dt;
                    ev.push(CampEvent::StructureHit { hp_left: self.struct_hp.max(0.0) });
                    if self.struct_hp <= 0.0 {
                        self.win(&mut ev);
                    }
                }
            }
            Objective::Control => {
                if contested == 0 {
                    self.hold_pct = (self.hold_pct + HOLD_RISE * dt).min(100.0);
                } else {
                    self.hold_pct = (self.hold_pct - HOLD_BLEED * dt).max(0.0);
                }
                ev.push(CampEvent::Hold { pct: self.hold_pct });
                if self.hold_pct >= 100.0 {
                    self.win(&mut ev);
                }
            }
            Objective::Protect => {
                if contested > 0 {
                    self.asset_hp -= ASSET_CHIP * contested as f32 * dt;
                    ev.push(CampEvent::AssetHit { hp_left: self.asset_hp.max(0.0) });
                    if self.asset_hp <= 0.0 {
                        self.outcome = Some(false);
                        ev.push(CampEvent::Lost);
                        return ev;
                    }
                }
                if self.waves_exhausted() && self.fight.is_none() {
                    self.win(&mut ev);
                }
            }
        }
        ev
    }

    fn win(&mut self, ev: &mut Vec<CampEvent>) {
        if self.outcome.is_none() {
            self.outcome = Some(true);
            ev.push(CampEvent::Won);
        }
    }

    /// Drive to a decision under a cap. Uncapped missions do not exist:
    /// a mission that cannot end is a bug, and the cap converts it into a
    /// visible loss instead of a hang.
    pub fn resolve(&mut self, cap_s: f32) -> bool {
        while self.outcome.is_none() && self.t < cap_s {
            self.step(0.1);
        }
        self.outcome.unwrap_or(false)
    }
}

/// The squad walks back to the open line between waves.
fn reset_line(mut s: Squad) -> Squad {
    for f in s.fighters.iter_mut() {
        f.at = OPEN_AT;
        f.broken = 0.0;
        f.rallied = false; // a breather restores the nerve too
    }
    s
}

/// Who commands the opposition: the films escalate from a warlord's
/// pressure to something that holds ground and makes you pay for it.
pub fn foe_doctrine(m: &Mission) -> &'static str {
    match m.film {
        1 | 2 => "aggressor",
        3 | 4 => "surrounder",
        5 => "baiter",
        _ => "warden",
    }
}

/// The squad the balance promise is made against.
pub const DEFAULT_SQUAD: [&str; 3] = ["vanguard", "titan", "specialist"];
pub const DEFAULT_DOCTRINE: &str = "skirmisher";

// ----------------------------------------------------------------- log ---

/// Which missions are done, persisted as a `missions` line of ids in the
/// save — same lenient contract as the raid log.
#[derive(Clone, Default)]
pub struct CampaignLog {
    cleared: Vec<&'static str>,
}

impl CampaignLog {
    pub fn from_line(line: &str) -> CampaignLog {
        let mut log = CampaignLog::default();
        for id in line.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            log.record(id);
        }
        log
    }
    pub fn line(&self) -> String {
        self.cleared.join(",")
    }
    pub fn record(&mut self, id: &str) -> bool {
        let Some(m) = MISSIONS.iter().find(|m| m.id == id) else { return false };
        if self.cleared.contains(&m.id) {
            return false;
        }
        self.cleared.push(m.id);
        self.cleared.sort_by_key(|c| MISSIONS.iter().position(|x| x.id == *c));
        true
    }
    pub fn is_cleared(&self, id: &str) -> bool {
        self.cleared.contains(&id)
    }
    /// Slipstream defects at Film 2's close — `TF_DOMINION_SCENE_MINDMAP.md`'s
    /// Act 4 resolution: "Slipstream has defected — carries knowledge the
    /// Autobots will need in Film 3." The troop entry (`content.rs`,
    /// `Role::Enemy`, `Faction::Hijacks`) is left exactly as it is on
    /// purpose — one named pilot defecting does not retire the airframe,
    /// and `Squad::new` already builds from any id leniently, so a squad
    /// naming "slipstream" post-defection just works. This predicate is
    /// the fictional permission the mission text has already earned, not
    /// a gate the code was missing: `the_drowned_approach` is Film 2's
    /// actual final mission (`content.rs`'s own outro count confirms it —
    /// every mission carries exactly two outro lines except the campaign
    /// finale, so a defection beat was left for prose rather than forced
    /// into an already-composed mission on this pass).
    pub fn slipstream_defected(&self) -> bool {
        self.is_cleared("the_drowned_approach")
    }
    /// The shield is fully mapped once Slipstream's Film 2 intel and
    /// Film 3's own Cybertron archive reveal are both in — the exact
    /// sequence `TF_ADDITIONAL_SCENE_AIDS.md` section 10 lays out:
    /// "Cybertron archives — first written reference" comes first,
    /// then "Slipstream provides missing coordinates — shield fully
    /// mapped." Neither fact alone is the doc's ending; the archive
    /// without the coordinates is a shape with no location, and the
    /// coordinates without the archive are numbers with no context.
    /// `beneath_the_pacific` (Film 3, sited on Cybertron) is where the
    /// archive half of that sequence already happens in this campaign.
    pub fn shield_mapped(&self) -> bool {
        self.slipstream_defected() && self.is_cleared("beneath_the_pacific")
    }
    pub fn done(&self) -> usize {
        self.cleared.len()
    }
    /// The campaign is linear: next is the first thing not yet done.
    pub fn next(&self) -> Option<&'static Mission> {
        MISSIONS.iter().find(|m| !self.is_cleared(m.id))
    }
    /// How far into a film the player is, for the mission list.
    pub fn film_done(&self, n: u8) -> (usize, usize) {
        let total = missions_of(n).count();
        let done = missions_of(n).filter(|m| self.is_cleared(m.id)).count();
        (done, total)
    }
    /// The wardens stop being rumours when the story is over.
    pub fn raids_unlocked(&self) -> bool {
        self.done() == MISSIONS.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_campaign_is_winnable_in_sequence_by_the_default_squad() {
        // THE balance promise, stated the way a campaign is played: each
        // mission at the energon the road provides — missions alone, no
        // wardens, no nightfalls. Deterministic waves, deterministic
        // fights: if a content edit breaks the sequence, this names the
        // mission it breaks at.
        for (i, m) in MISSIONS.iter().enumerate() {
            let at = i as u32 * energon::PER_MISSION;
            let mut run = MissionRun::new(m, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, at);
            let won = run.resolve(1200.0);
            assert!(
                won,
                "{} ({:?}, film {}, energon {} vs rec {}) breaks the campaign: t={:.0} squad={} struct={:.0} asset={:.0} hold={:.0}",
                m.id, m.objective, m.film, at, energon::rec_energon(m),
                run.t, run.squad_alive(), run.struct_hp, run.asset_hp, run.hold_pct
            );
        }
    }

    #[test]
    fn low_energon_is_a_wall() {
        // the counterpart promise: energon is LOAD-BEARING. A dry
        // default squad must fail somewhere in trilogy two, or the curve
        // is decoration and the campaign is flat — Destiny's recommended
        // number only teaches if being under it actually hurts.
        let flat_loses_somewhere = MISSIONS.iter().filter(|m| m.film >= 4).any(|m| {
            !MissionRun::new(m, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, 0).resolve(1200.0)
        });
        assert!(flat_loses_somewhere, "trilogy two folded to a dry squad; re-tune the films");
    }

    #[test]
    fn the_shield_needs_both_slipstreams_coordinates_and_the_cybertron_archive() {
        // TF_ADDITIONAL_SCENE_AIDS.md's own sequence: neither fact
        // alone maps the shield, in either order
        let mut only_archive = CampaignLog::default();
        only_archive.record("the_pluto_line");
        only_archive.record("the_scavenger_king");
        only_archive.record("the_red_extraction");
        only_archive.record("the_beach_opens");
        only_archive.record("what_jhiaxus_grew");
        only_archive.record("the_mantle_dive");
        // note: the_drowned_approach (Slipstream's own mission) is
        // deliberately skipped here
        only_archive.record("the_ashen_truce");
        only_archive.record("beneath_the_pacific");
        assert!(!only_archive.slipstream_defected());
        assert!(!only_archive.shield_mapped(), "the shield mapped without Slipstream's coordinates");

        let mut only_slipstream = CampaignLog::default();
        only_slipstream.record("the_drowned_approach");
        assert!(only_slipstream.slipstream_defected());
        assert!(!only_slipstream.shield_mapped(), "the shield mapped without the Cybertron archive");

        let mut both = only_archive.clone();
        both.record("the_drowned_approach");
        assert!(both.shield_mapped());
    }

    #[test]
    fn slipstream_defects_at_film_twos_actual_close_and_a_squad_can_use_her_after() {
        let mut log = CampaignLog::default();
        assert!(!log.slipstream_defected());
        // clearing an earlier film-2 mission is not the same as clearing
        // the film's actual last one
        log.record("what_jhiaxus_grew");
        assert!(!log.slipstream_defected());
        log.record("the_drowned_approach");
        assert!(log.slipstream_defected());
        // the fictional permission does not need a code gate: the roster
        // was always lenient enough to field a defector
        let squad = Squad::new(&["slipstream", "vanguard"], "skirmisher", OPEN_AT);
        assert_eq!(squad.fighters.len(), 2, "slipstream did not compile into a fighter");
    }

    #[test]
    fn a_corrupted_site_actually_fields_the_corrupted() {
        // the gap this closes: `patrol::Destination.hostiles` existed
        // and was read by nothing. earth is Faction::Corrupted; the
        // beach mission (film 1, no champion) should now draw from the
        // Corrupted roster rather than a generic grunt line, once the
        // film-gate that reserves early films for grunts/raiders lifts.
        let beach = MISSIONS.iter().find(|m| m.id == "the_beach_opens").unwrap();
        assert_eq!(patrol::find(beach.dest).unwrap().hostiles, Faction::Corrupted);
        // find a later mission actually sited on a Corrupted world with
        // no name-collision on champion, to see the roster past the
        // early-film grunt gate
        let corrupted_site = MISSIONS
            .iter()
            .find(|m| m.film >= 4 && patrol::find(m.dest).map(|d| d.hostiles) == Some(Faction::Corrupted));
        if let Some(m) = corrupted_site {
            let seen: std::collections::HashSet<&str> = waves_for(m).into_iter().flatten().collect();
            assert!(
                seen.iter().any(|id| id.starts_with("corrupted_")),
                "{} sits on a Corrupted world and never fielded one: {seen:?}",
                m.id
            );
        }
        // and an all-elite Coalition site (currently pluto's neighbours)
        // must not panic by starving the shield-density substitution
        let coalition_site =
            MISSIONS.iter().find(|m| patrol::find(m.dest).map(|d| d.hostiles) == Some(Faction::Coalition));
        if let Some(m) = coalition_site {
            let _ = waves_for(m); // must not panic
        }
    }

    #[test]
    fn waves_are_deterministic_and_escalate_with_the_films() {
        let m1 = &MISSIONS[0];
        let a: Vec<_> = waves_for(m1);
        let b: Vec<_> = waves_for(m1);
        assert_eq!(a, b, "the same mission fielded two different armies");
        let count = |film: u8| -> usize {
            missions_of(film).map(|m| waves_for(m).iter().map(|w| w.len()).sum::<usize>()).max().unwrap()
        };
        assert!(count(6) > count(1), "the last film fields no more than the first");
        // the champion leads the final wave, once, when named
        for m in MISSIONS.iter().filter(|m| m.champion.is_some()) {
            let w = waves_for(m);
            let c = m.champion.unwrap();
            assert_eq!(w.last().unwrap().iter().filter(|t| **t == c).count(), 1, "{}", m.id);
            assert!(w[..w.len() - 1].iter().all(|wv| !wv.contains(&c)), "{} leaks its champion early", m.id);
        }
        // trilogy two actually fields variants somewhere
        let late_has_variant = MISSIONS
            .iter()
            .filter(|m| m.film >= 4)
            .any(|m| waves_for(m).iter().flatten().any(|id| {
                crate::content::troop(id).map(|t| t.role == Role::Variant).unwrap_or(false)
            }));
        assert!(late_has_variant, "the coalition races never take the field");
    }

    #[test]
    fn survive_is_won_on_the_clock_not_the_bodycount() {
        let (i, m) = MISSIONS.iter().enumerate().find(|(_, m)| m.objective == Objective::Survive).unwrap();
        let mut run = MissionRun::new(m, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, i as u32 * energon::PER_MISSION);
        let won = run.resolve(1200.0);
        assert!(won);
        // the clock is the win condition, so the run lasts at least the
        // hold duration — and the waves were still cycling when it ended
        assert!(run.t >= 75.0, "a survive mission ended before its clock");
    }

    #[test]
    fn protect_can_be_lost() {
        // a lone grunt-tier escort cannot hold a protect mission: it
        // fails through the asset or through the squad, and either way it
        // FAILS — the objective has teeth. (A grunt, deliberately: heroes
        // are the tier that wins these, and that is the point.)
        let m = MISSIONS.iter().find(|m| m.objective == Objective::Protect).unwrap();
        let mut run = MissionRun::new(m, &["grunt"], "warden", 0);
        let mut lost_by_asset = false;
        while run.outcome().is_none() && run.t < 1200.0 {
            for e in run.step(0.1) {
                if e == CampEvent::Lost && run.asset_hp <= 0.0 {
                    lost_by_asset = true;
                }
            }
        }
        assert_eq!(run.outcome(), Some(false), "a lone scout held a protect mission");
        assert!(lost_by_asset || run.squad_alive() == 0, "the loss came from nowhere");
    }

    #[test]
    fn control_banks_only_uncontested_seconds() {
        let (i, m) = MISSIONS.iter().enumerate().find(|(_, m)| m.objective == Objective::Control).unwrap();
        let mut run = MissionRun::new(m, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, i as u32 * energon::PER_MISSION);
        let mut rose_contested = false;
        let mut last = 0.0f32;
        while run.outcome().is_none() && run.t < 1200.0 {
            let contested_before = run.fight.as_ref().map(|f| f.b.alive() > 0).unwrap_or(false);
            let ev = run.step(0.1);
            let cleared_this_tick = ev.iter().any(|e| matches!(e, CampEvent::WaveCleared { .. }));
            let still_contested = run.fight.as_ref().map(|f| f.b.alive() > 0).unwrap_or(false);
            // the rule: a tick that BEGAN contested and did not clear the
            // wave must not bank hold. A tick that cleared mid-step may.
            if contested_before && still_contested && !cleared_this_tick && run.hold_pct > last + 1e-6 {
                rose_contested = true;
            }
            last = run.hold_pct;
        }
        assert_eq!(run.outcome(), Some(true));
        assert!(!rose_contested, "the zone filled while the field was contested");
    }

    #[test]
    fn the_log_roundtrips_and_gates_the_wardens() {
        let mut log = CampaignLog::from_line("the_pluto_line, bogus ,the_pluto_line");
        assert_eq!(log.done(), 1);
        assert_eq!(log.next().unwrap().id, "the_scavenger_king");
        for m in MISSIONS {
            log.record(m.id);
        }
        assert!(log.raids_unlocked());
        assert!(log.next().is_none());
        let back = CampaignLog::from_line(&log.line());
        assert_eq!(back.done(), MISSIONS.len());
        // linearity: next is always the FIRST gap, not the last clear + 1
        let mut gap = CampaignLog::default();
        gap.record(MISSIONS[2].id);
        assert_eq!(gap.next().unwrap().id, MISSIONS[0].id);
    }
}
