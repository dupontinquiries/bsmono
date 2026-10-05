//! THE RAID, FLOWN — the warden ladder as a run of drops, not a call.
//!
//! `raidgen` plans a raid (three phases, four encounters each), `raid::run`
//! resolves one for the planner, and `raidrun` is the C engine's encounter
//! layer ported as a pure per-tick machine that nobody was driving from a
//! seat. This is the seat: the Wardeck's `Order::Raid` lands here, and the
//! flow walks the plan the way the C engine played it —
//!
//!   Brief -> Falling -> Assault -> (Between -> Brief -> ...) -> Debrief
//!
//! A PHASE SWAPS THE MAP AND THE CLOCK RESTARTS, which means a raid is
//! several dropship sequences: every phase opens with its own foothold
//! question (the raid's sites are `raidgen`'s own, not derived here), its
//! own sky and window (`raid::window_s` with the arena's REAL orbit), and
//! its own pass out of `dropship::launch`.
//!
//! What the assault actually is: `raidrun::PhaseRun` stepped with unit
//! positions. AIR nodes are claimed by BEING there, SYNC plates by
//! standing on them, GROUND fields slam and chip anything flying —
//! position-driven, no combat model required. The two that need a fight —
//! quota encounters and the anchor's champion — run through THE KILL
//! BRIDGE below, which is labelled for what it is.
//!
//! Debts, by name:
//! * **The kill bridge.** No troops walk the arena yet. The player's fire
//!   near a quota beacon converts damage into kills at one grunt-hp per
//!   kill (`skirmish::fighter("grunt")` — a real number, an abstract use),
//!   and the champion is a damage pool at `CHAMPION_GRUNTS` grunts. When
//!   raid troops are real entities, this section deletes.
//! * **The squad holds the seats.** SYNC wants more units than one;
//!   `PhaseRun::seats` says where companions should stand, and two squad
//!   ghosts stand exactly there. That is the whole squad AI, and it is
//!   labelled as such.
//! * **The plan is leaked.** `RaidRun` borrows its `RaidPlan`; storing
//!   both across frames needs `&'static`, so `begin` leaks one small plan
//!   per raid started. Bounded, ugly, honest — retiring it means giving
//!   `raidrun` an owning mode.
//! * **A scattered landing spends the window.** The penalty formula is the
//!   sortie's (`stations::landing_penalty`); here it burns weather-window
//!   seconds instead of squad power, because the window IS this mode's
//!   power. Derived, and the exchange rate is a judgment call.

use glam::Vec3;

use crate::drop::{Drop, Landing, Mode};
use crate::dropship;
use crate::foothold::{self, Approach, Plan, Site};
use crate::bigbody::BigBody;
use crate::raidcon::{Arena, Boss};
use crate::raidgen::{phase_raid, plan_for, strhash, EncKind, RaidPlan};
use crate::raidrun::{Beacon, Event, RaidRun, Unit};
use crate::sections;
use crate::skirmish::{fighter, OPEN_AT};
use crate::stations;
use crate::survival::{Body, Weather};
use crate::weapons::Element;

/// Kills are attributed to the nearest undone encounter inside this many
/// metres of the shooter. A GUESS standing in for the C engine's zone
/// volumes, which did not survive the port.
const CREDIT_R: f32 = 30.0;
/// The champion, priced in grunts. A GUESS: hero tier is ~3 grunts by
/// `skirmish`'s own constant, a champion sits above a hero.
const CHAMPION_GRUNTS: f32 = 9.0;
/// Surface gravity for an arena with no route hook, m/s². A GUESS pending
/// orbit-derived gravity; arenas with `via` use their destination's real
/// figure.
const ORPHAN_ARENA_G: f32 = 9.81 * 0.8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Brief,
    Falling,
    Assault,
    /// a phase cleared and the next arena is waiting on the shell to load
    /// its map — Enter flies the next drop
    Between,
    Debrief { won: bool },
}

pub struct RaidFlow {
    pub boss: &'static Boss,
    /// leaked — see the module header
    pub plan: &'static RaidPlan,
    run: RaidRun<'static>,
    pub phase_i: usize,
    pub stage: Stage,
    pub drop: Drop,
    pub hull: Option<BigBody>,
    pub landing: Option<Landing>,
    /// this phase's sky, window and footholds
    pub weather: Weather,
    pub window_s: f32,
    pub sites: Vec<Site>,
    pub plans: Vec<Plan>,
    pub site_sel: usize,
    lz_dir: Vec3,
    pub g: f32,
    /// the kill bridge: damage pooled per encounter, and the champion's
    pools: Vec<f32>,
    champ_pool: f32,
    champ_done: bool,
    foe_hp: f32,
    /// does a win bank a shard? True for the warden ladder's own signals;
    /// false for forged echoes (`raidforge`), which fly the same and count
    /// for nothing — the seal stays about the nineteen real ones.
    scored: bool,
    /// seconds spent fighting off an open LZ, for the hull's wear bill
    lz_secs: f32,
    pub wear: f32,
    pub feed: Vec<String>,
    toasts: Vec<String>,
    aa_applied: f32,
    aa_note_t: f32,
    last_gy: f32,
}

impl RaidFlow {
    /// Accept the Wardeck's order. The ladder gate lives in the console
    /// (`stations::apply`); this trusts it and builds the plan.
    pub fn begin(boss_id: &str) -> Result<RaidFlow, &'static str> {
        let boss = crate::raidcon::boss(boss_id).ok_or("no such warden")?;
        // deterministic per boss: the same signal plans the same raid
        let plan: &'static RaidPlan = Box::leak(Box::new(plan_for(boss, 3, strhash(boss.id))));
        Self::from_plan(plan, true)
    }

    /// Fly a forged echo: any well-formed plan, through the same flow,
    /// banking nothing. The plan is leaked like the ladder's own — see the
    /// module header for that debt.
    pub fn begin_echo(plan: RaidPlan) -> Result<RaidFlow, &'static str> {
        if plan.phases.is_empty() {
            return Err("an echo with no phases is silence");
        }
        let plan: &'static RaidPlan = Box::leak(Box::new(plan));
        Self::from_plan(plan, false)
    }

    fn from_plan(plan: &'static RaidPlan, scored: bool) -> Result<RaidFlow, &'static str> {
        let boss = plan.boss;
        let run = RaidRun::new(plan);
        let foe_hp = fighter("grunt", OPEN_AT).map(|f| f.hp_max).unwrap_or(120.0);
        let mut f = RaidFlow {
            boss,
            plan,
            run,
            scored,
            phase_i: 0,
            stage: Stage::Brief,
            drop: Drop::new(Vec3::ZERO, false),
            hull: None,
            landing: None,
            weather: Weather::Clear,
            window_s: f32::INFINITY,
            sites: Vec::new(),
            plans: Vec::new(),
            site_sel: 0,
            lz_dir: Vec3::NEG_Z,
            g: ORPHAN_ARENA_G,
            pools: Vec::new(),
            champ_pool: 0.0,
            champ_done: false,
            foe_hp,
            lz_secs: 0.0,
            wear: stations::ship_wear(),
            feed: Vec::new(),
            toasts: Vec::new(),
            aa_applied: 0.0,
            aa_note_t: 0.0,
            last_gy: 0.0,
        };
        f.brief_phase();
        Ok(f)
    }

    pub fn arena(&self) -> &'static Arena {
        self.plan.phases[self.phase_i].arena
    }
    pub fn next_arena(&self) -> &'static Arena {
        self.plan.phases[(self.phase_i + 1).min(self.plan.phases.len() - 1)].arena
    }
    pub fn falling(&self) -> bool {
        self.stage == Stage::Falling
    }
    pub fn assaulting(&self) -> bool {
        self.stage == Stage::Assault
    }

    /// Set up the CURRENT phase's brief: the raid's own sites, the arena's
    /// own sky, the real orbit's window.
    fn brief_phase(&mut self) {
        let pr = phase_raid(self.plan, self.phase_i);
        let a = self.arena();
        self.weather = pr.weather;
        self.window_s = crate::raid::window_s(&a.orbit, pr.weather);
        self.plans = pr
            .sites
            .iter()
            .map(|s| {
                foothold::assess(
                    Body::Machine,
                    s,
                    pr.weather,
                    self.window_s,
                    crate::sortie::SQUAD_SPEED_MS,
                )
            })
            .collect();
        self.site_sel = foothold::choose(
            Body::Machine,
            &pr.sites,
            pr.weather,
            self.window_s,
            crate::sortie::SQUAD_SPEED_MS,
        )
        .and_then(|p| self.plans.iter().position(|q| q.site.approach == p.site.approach))
        .unwrap_or(0);
        self.sites = pr.sites;
        let h = strhash(self.boss.id) ^ (self.phase_i as u64);
        let ang = (h % 6283) as f32 / 1000.0;
        self.lz_dir = Vec3::new(ang.cos(), 0.0, ang.sin());
        self.g = a
            .via
            .and_then(|v| crate::patrol::ROUTE.iter().find(|d| d.id == v))
            .map(|d| crate::mapsim::ground_for(d).g)
            .unwrap_or(ORPHAN_ARENA_G);
        let n = self.plan.phases[self.phase_i].enc.len();
        self.pools = vec![0.0; n];
        self.champ_pool = self.foe_hp * CHAMPION_GRUNTS;
        self.champ_done = false;
        self.aa_applied = 0.0;
        self.aa_note_t = 0.0;
        self.landing = None;
        self.stage = Stage::Brief;
        self.push_toast(format!(
            "phase {}/{} — {} · {}",
            self.phase_i + 1,
            self.plan.phases.len(),
            self.plan.phases[self.phase_i].name,
            a.name
        ));
    }

    /// The shell reloaded the map for the next arena; walk the plan
    /// forward and brief it.
    pub fn brief_next(&mut self) {
        if self.stage != Stage::Between {
            return;
        }
        let _ = self.run.advance();
        self.phase_i += 1;
        self.brief_phase();
    }

    pub fn site_cycle(&mut self, d: i32) {
        if self.stage != Stage::Brief || self.sites.is_empty() {
            return;
        }
        let n = self.sites.len() as i32;
        self.site_sel = ((self.site_sel as i32 + d).rem_euclid(n)) as usize;
    }

    pub fn lz_now(&self) -> Vec3 {
        self.lz_dir * self.sites[self.site_sel].at.distance_m
    }

    /// Out the door onto this phase's ground. Warden arenas shoot back at
    /// anything dropping on what they built; open ground is open.
    pub fn launch(&mut self, ground_y: f32) -> bool {
        if self.stage != Stage::Brief {
            return false;
        }
        let plan = self.plans[self.site_sel];
        if !plan.viable {
            self.push_toast(format!("{} — {}", plan.site.approach.label(), plan.note));
            return false;
        }
        let defended = plan.site.approach == Approach::SeizeFortress;
        let lz = self.lz_now();
        let h = strhash(self.boss.id) ^ (self.phase_i as u64);
        match dropship::launch(lz, defended, self.wear, ground_y, h) {
            Ok((hull, drop)) => {
                self.drop = drop;
                self.hull = Some(hull);
                self.stage = Stage::Falling;
                self.push_toast(format!(
                    "out the door — {} it is",
                    plan.site.approach.label()
                ));
                true
            }
            Err(e) => {
                self.push_toast(e.to_string());
                false
            }
        }
    }

    pub fn fold(&mut self) {
        if self.stage == Stage::Falling {
            let alt = self.drop.pos.y - self.last_gy;
            self.drop.transform(alt);
            self.push_toast(
                match self.drop.mode {
                    Mode::Robot => "limbs out — slow, steerable, visible",
                    Mode::Vehicle => "folded — fast, committed",
                }
                .to_string(),
            );
        }
    }

    /// One fixed step of the descent. The assault ticks separately
    /// (`tick`) because it needs the player's position, not a stick.
    pub fn update(&mut self, dt: f32, steer: Vec3, ground_y: f32, body: &mut sections::Body) {
        self.last_gy = ground_y;
        if self.stage != Stage::Falling {
            return;
        }
        let aa0 = self.drop.aa_so_far();
        let landed = self.drop.update(dt, steer, self.g, ground_y);
        let flak = self.drop.aa_so_far() - aa0;
        if let Some(t) = dropship::charge_flak(
            flak,
            dt,
            &mut self.aa_applied,
            &mut self.aa_note_t,
            body,
            &mut self.feed,
        ) {
            self.push_toast(t.to_string());
        }
        if let Some(l) = landed {
            self.touchdown(l, body);
        }
    }

    fn touchdown(&mut self, l: Landing, body: &mut sections::Body) {
        self.hull = None;
        let penalty = stations::landing_penalty(&l);
        self.push_toast(dropship::landing_line(&l, penalty));
        if let Some(t) = dropship::charge_landing(&l, body, &mut self.feed) {
            self.push_toast(t.to_string());
        }
        // A SCATTERED LANDING SPENDS THE WINDOW: regrouping costs sky, and
        // sky is what a raid runs on. Same penalty formula as the sortie,
        // different currency — see the module header.
        let window = self.window_s * (1.0 - penalty);
        let sheltered = self.plans[self.site_sel].site.approach == Approach::SeizeFortress;
        self.run.start_phase(window, sheltered);
        self.landing = Some(l);
        self.stage = Stage::Assault;
    }

    /// One assault tick. `player` drives the mechanics; two squad ghosts
    /// hold the SYNC seats (the labelled squad AI). Returns the PHYSICAL
    /// events — slams, chips — for the shell to apply to the car and the
    /// body; narration stays here.
    pub fn tick(&mut self, dt: f32, player: Vec3, ground_y: f32) -> Vec<Event> {
        if self.stage != Stage::Assault {
            return Vec::new();
        }
        if self.plans[self.site_sel].site.approach == Approach::ClearLz {
            self.lz_secs += dt;
        }
        let Some(pr) = self.run.phase_run() else { return Vec::new() };
        let seats = pr.seats([player.x, player.z]);
        let units = [
            Unit { pos: [player.x, player.y, player.z] },
            Unit { pos: [seats[0][0], ground_y, seats[0][1]] },
            Unit { pos: [seats[1][0], ground_y, seats[1][1]] },
        ];
        let events = pr.step(dt, &units);
        let mut out = Vec::new();
        for e in events {
            match e {
                Event::Slam { unit: 0, .. } | Event::Chip { unit: 0, .. } => out.push(e),
                // the ghosts absorb their own slams and chips silently —
                // they are positions, not bodies, until the squad is real
                Event::Slam { .. } | Event::Chip { .. } => {}
                Event::Grapple { .. } => out.push(e),
                Event::NodeClaimed { enc } => self.feed.push(format!("node {} claimed", enc + 1)),
                Event::SyncHeld { .. } => {}
                Event::Secured { enc } => {
                    self.push_toast(format!("encounter {} secured", enc + 1))
                }
                Event::AnchorDown { .. } => self.push_toast("the anchor falls".into()),
                Event::PhaseCleared => {
                    if self.phase_i + 1 < self.plan.phases.len() {
                        self.stage = Stage::Between;
                        self.push_toast("phase cleared — the next arena is waiting".into());
                    } else {
                        self.stage = Stage::Debrief { won: true };
                        self.push_toast(format!("{} — the shard is yours", self.boss.name));
                    }
                }
                Event::WindowClosed => {
                    self.stage = Stage::Debrief { won: false };
                    self.push_toast("the sky closed first".into());
                }
            }
        }
        if let Some(pr) = self.run.phase_run() {
            if pr.failed() && !matches!(self.stage, Stage::Debrief { .. }) {
                self.stage = Stage::Debrief { won: false };
            }
        }
        if self.feed.len() > 64 {
            let cut = self.feed.len() - 64;
            self.feed.drain(..cut);
        }
        out
    }

    /// THE KILL BRIDGE — see the module header. The shell's fire arrives
    /// here with the same converted damage the sortie's engagement gets;
    /// it pools against the nearest undone encounter in reach. `elem` is
    /// the Watch's attuned element at the moment of the hit — `None`
    /// while mid-attune, same as every other advantage check in the
    /// game — and runs through the warden's own `element` on the same
    /// ring the regular bots use, so a matched attunement finally means
    /// something against a boss instead of the ring staying flat.
    pub fn strike(&mut self, dmg: f32, elem: Element, at: Vec3) {
        if self.stage != Stage::Assault {
            return;
        }
        let dmg = dmg * elem.advantage(self.boss.element);
        let spec = &self.plan.phases[self.phase_i];
        let Some(pr) = self.run.phase_run() else { return };
        let mut best: Option<(f32, usize)> = None;
        for (i, e) in spec.enc.iter().enumerate() {
            if pr.enc_done(i) {
                continue;
            }
            let d2 = (at.x - e.x).powi(2) + (at.z - e.z).powi(2);
            if d2 < CREDIT_R * CREDIT_R && best.map(|b| d2 < b.0).unwrap_or(true) {
                best = Some((d2, i));
            }
        }
        let Some((_, i)) = best else { return };
        if matches!(spec.enc[i].kind, EncKind::Anchor) {
            if !self.champ_done {
                self.champ_pool -= dmg;
                if self.champ_pool <= 0.0 {
                    self.champ_done = true;
                    pr.champion_fell();
                    self.feed.push("the champion falls".into());
                }
            }
        } else {
            self.pools[i] += dmg;
            while self.pools[i] >= self.foe_hp {
                self.pools[i] -= self.foe_hp;
                pr.credit_kill(i);
                self.feed.push(format!("hostile down at encounter {}", i + 1));
            }
        }
    }

    /// Live encounter beacons for the shell to draw — `PhaseRun::beacons`'s
    /// own doc calls this "for the shell to draw," but nothing in the
    /// shell ever called it; the Assault stage had no in-world visual at
    /// any encounter site, only the HUD text panel. Empty outside Assault
    /// or before the plan's first phase starts.
    pub fn live_beacons(&self) -> Vec<Beacon> {
        self.run.phase_ref().map(|pr| pr.beacons()).unwrap_or_default()
    }

    /// A grapple toward the nearest live AIR node, if the line reaches.
    pub fn grapple(&mut self, player: Vec3) -> Option<[f32; 3]> {
        if self.stage != Stage::Assault {
            return None;
        }
        let pr = self.run.phase_run()?;
        let units = [Unit { pos: [player.x, player.y, player.z] }];
        match pr.request_grapple(0, &units) {
            Some(Event::Grapple { vel, .. }) => Some(vel),
            _ => None,
        }
    }

    /// Close the debrief: a won raid banks the shard through the one raid
    /// persistence path; the pinned hull's wear bill lands either way.
    pub fn commit(&self) -> String {
        let Stage::Debrief { won } = self.stage else {
            return "raid abandoned — no result recorded".into();
        };
        let mut tail = String::new();
        if self.lz_secs > 0.0 {
            let w = crate::raid::lz_wear(self.lz_secs);
            stations::record_ship_wear(w);
            tail = format!(" · hull wore +{w:.1} on the deck");
        }
        if !won {
            return format!("{} holds — nothing recorded{tail}", self.boss.name);
        }
        if !self.scored {
            return format!("the echo is laid to rest — banks nothing{tail}");
        }
        // the flown team is machine-bodied (`Body::Machine` above): no suited chi
        let banked = stations::record_raid_outcome(self.boss.id, false);
        let rewards = banked.lines().join(" · ");
        let tail = if rewards.is_empty() { tail } else { format!(" · {rewards}{tail}") };
        format!(
            "{} — SHARD RECOVERED{}{tail}",
            self.boss.name,
            if banked.fresh { "" } else { "; already banked" }
        )
    }

    fn push_toast(&mut self, s: String) {
        self.feed.push(s.clone());
        self.toasts.push(s);
    }
    pub fn pop_toast(&mut self) -> Option<String> {
        if self.toasts.is_empty() {
            None
        } else {
            Some(self.toasts.remove(0))
        }
    }

    /// `grapple_key` labels whatever `Action::Grapple` is actually bound
    /// to — the row used to hardcode "G", which went stale the moment
    /// that stopped being the default (see `input::Bindings::classic`).
    pub fn hud_rows(&self, grapple_key: &str) -> Vec<String> {
        let mut rows = Vec::new();
        match self.stage {
            Stage::Brief => {
                rows.push(format!(
                    "RAID — {} · phase {}/{}",
                    self.boss.name,
                    self.phase_i + 1,
                    self.plan.phases.len()
                ));
                let window = if self.window_s.is_finite() {
                    format!("window {:.0}s", self.window_s)
                } else {
                    "open sky".into()
                };
                rows.push(format!("{} · {} ({window})", self.arena().name, self.weather.label()));
                if self.wear > 0.1 {
                    rows.push(format!("hull wear {:.1} — expect a dirty pass", self.wear));
                }
                for (i, p) in self.plans.iter().enumerate() {
                    let mark = if i == self.site_sel { ">" } else { " " };
                    let verdict = if !p.viable {
                        p.note.to_string()
                    } else if p.margin_s.is_finite() {
                        format!("margin {:.0}s", p.margin_s)
                    } else {
                        "open sky".into()
                    };
                    rows.push(format!(
                        "{mark} {} — take {:.0}s · {}",
                        p.site.approach.label(),
                        p.site.take_s,
                        verdict
                    ));
                }
                rows.push("ENTER launch · UP/DOWN site · ESC stand down".into());
            }
            Stage::Falling => {
                let alt = (self.drop.pos.y - self.last_gy).max(0.0);
                rows.push(format!(
                    "DROP · {}",
                    match self.drop.mode {
                        Mode::Vehicle => "folded",
                        Mode::Robot => "limbs out",
                    }
                ));
                rows.push(format!("alt {alt:.0} m · sink {:.0} m/s", -self.drop.vel.y));
                rows.push("steer to the mark · JUMP folds".into());
            }
            Stage::Assault => {
                rows.push(format!(
                    "ASSAULT — phase {}/{}",
                    self.phase_i + 1,
                    self.plan.phases.len()
                ));
                let spec = &self.plan.phases[self.phase_i];
                if let Some(pr) = self.run.phase_ref() {
                    if pr.left_s.is_finite() {
                        rows.push(format!("window {:.0}s left", pr.left_s.max(0.0)));
                    }
                    for (i, e) in spec.enc.iter().enumerate() {
                        let state = if pr.enc_done(i) {
                            "done".to_string()
                        } else if matches!(e.kind, EncKind::Anchor) {
                            "the anchor".to_string()
                        } else {
                            format!("{} down", pr.kills(i))
                        };
                        rows.push(format!("{:?} {} — {}", e.mech, i + 1, state));
                    }
                }
                rows.push(format!("{grapple_key} grapples at a node · ESC abandons"));
            }
            Stage::Between => {
                rows.push("PHASE CLEARED".into());
                rows.push(format!("next: {}", self.next_arena().name));
                rows.push("ENTER flies the next drop · ESC abandons".into());
            }
            Stage::Debrief { won } => {
                rows.push(if won {
                    format!("{} RECOVERED", self.boss.name)
                } else {
                    "THE RAID FAILED".into()
                });
                rows.push("ENTER to log it and stand down".into());
            }
        }
        for line in self.feed.iter().rev().take(4).rev() {
            rows.push(format!("· {line}"));
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raidcon::BOSSES;

    fn first_boss() -> &'static str {
        BOSSES[0].id
    }

    fn fly_down(f: &mut RaidFlow) -> sections::Body {
        let mut b = sections::Body::default();
        assert!(f.launch(0.0), "the chosen site refused the launch");
        for _ in 0..(60 * 180) {
            let alt = f.drop.pos.y;
            if alt < 120.0 && f.drop.mode == Mode::Vehicle {
                f.fold();
            }
            let to = (f.drop.lz - f.drop.pos) * Vec3::new(1.0, 0.0, 1.0);
            f.update(1.0 / 60.0, (to * 0.05).clamp_length_max(1.0), 0.0, &mut b);
            if f.stage != Stage::Falling {
                return b;
            }
        }
        panic!("the drop never landed");
    }

    #[test]
    fn the_wardeck_order_reaches_an_assault_through_the_air() {
        let mut f = RaidFlow::begin(first_boss()).unwrap();
        assert_eq!(f.stage, Stage::Brief);
        assert_eq!(f.plans.len(), f.sites.len());
        assert!(!f.plans.is_empty(), "a phase briefed with no footholds");
        fly_down(&mut f);
        assert_eq!(f.stage, Stage::Assault, "touchdown did not start the phase");
        assert!(f.run.phase_run().is_some(), "no live PhaseRun after touchdown");
    }

    #[test]
    fn fire_near_a_quota_beacon_secures_it() {
        let mut f = RaidFlow::begin(first_boss()).unwrap();
        fly_down(&mut f);
        // find a quota encounter and stand on it with overwhelming fire
        let spec = &f.plan.phases[0];
        let Some((i, e)) = spec
            .enc
            .iter()
            .enumerate()
            .find(|(_, e)| matches!(e.kind, EncKind::Standard))
        else {
            return; // a phase of pure anchors would be its own generator bug
        };
        let at = Vec3::new(e.x, 0.0, e.z);
        for _ in 0..200 {
            f.strike(1.0e5, Element::None, at);
            f.tick(0.05, at, 0.0);
            if f.run.phase_run().map(|p| p.enc_done(i)).unwrap_or(false) {
                return;
            }
            if !matches!(f.stage, Stage::Assault) {
                return; // cleared past it — even better
            }
        }
        panic!("overwhelming fire never secured the quota encounter");
    }

    #[test]
    fn the_ladder_is_deterministic() {
        let a = RaidFlow::begin(first_boss()).unwrap();
        let b = RaidFlow::begin(first_boss()).unwrap();
        assert_eq!(a.lz_now(), b.lz_now());
        assert_eq!(a.plan.phases.len(), b.plan.phases.len());
        assert_eq!(a.plan.phases[0].arena.id, b.plan.phases[0].arena.id);
    }
}
