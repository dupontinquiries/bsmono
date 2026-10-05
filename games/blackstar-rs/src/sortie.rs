//! THE SORTIE, FLOWN — a mission as something you do, not a call you make.
//!
//! `stations::run_order` resolves a sortie in one call: autopilot flies the
//! drop, `MissionRun::resolve` decides the fight, a toast reports what you
//! were not present for. Every piece of that is a real machine with a `step`
//! — which is exactly the renderer-boundary debt `docs/PRIORITIES.md`
//! describes. This module is the flow that hands those machines to the
//! player, one stage at a time:
//!
//!   Brief -> Falling -> Engage -> Debrief
//!
//! * **Brief** — the order accepted, the destination loaded, and THE
//!   FOOTHOLD QUESTION asked at runtime for the first time: two candidate
//!   sites (`sites_for`, derived like `raidgen::phase_raid` derives its
//!   raid sites), each priced by `foothold::assess` against the
//!   destination's sky. A clear world makes the open LZ obviously right;
//!   weather makes the fortress the play — the trade `foothold.rs` was
//!   built around, finally asked where the player can answer it.
//! * **Falling** — `drop::Drop`, steered by the stick and folded by the
//!   jump button. The landing you fly is priced by the SAME formula the
//!   resolved path uses (`stations::landing_penalty`).
//! * **Engage** — `campaign::MissionRun`, stepped at the frame rate. The
//!   player's fire reaches the fight through `strike`: real weapon numbers
//!   (impulse, cooldown, element) through the fight's own shield and
//!   evasion rules — see `fire_weapon` in the shell for the conversion.
//! * **Debrief** — the outcome, and only then the save, through
//!   `stations::record_outcome`, the ONE persistence path.
//!
//! WHAT IS STILL CANNED, by name, so nobody mistakes it for wired:
//!
//! * The approach HULL. The throw out the door now comes from
//!   `Frame::carry_at` on a real `BigBody` flying the pass — velocity plus
//!   the rotational term of its bank — so the mechanism is the live one.
//!   The hull itself is still canned (nobody owns a flying ship yet): a
//!   belly-bay box on a deterministic line, banking into its escape turn,
//!   held in `hull` and not yet rendered. Hull WEAR dirties the bank —
//!   a ship that sat pinned on hostile decks flies a rougher pass until
//!   it gets yard time at a dock (`stations::ship_wear`).
//! * The LZ bearing and the site distances hang off the WORLD ORIGIN,
//!   because the ship parks there and nothing else on the map has an
//!   address yet — the known-wrong `docs/PRIORITIES.md` already lists.
//! * `weather_for` maps the route table's one-bit weather onto a dust
//!   storm. Derived stopgap with its name on it.
//! * Docked destinations refuse the flow and fall back to the resolved
//!   path: you berth at a station, you do not drop on it.

use glam::Vec3;

use crate::bigbody::BigBody;
use crate::campaign::{CampEvent, MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD};
use crate::content::Mission;
use crate::drop::{Drop, Landing, Mode};
use crate::field::Field;
use crate::foothold::{self, Approach, Plan, Site};
use crate::patrol::{Destination, Faction, ROUTE};
use crate::dropship;
use crate::raidgen::strhash;
use crate::sections;
use crate::stations;
use crate::survival::{Body, Bunker, Weather};
use crate::weapons::{Element, Register};

/// The squad's overland speed for the foothold clock, m/s. The same
/// Machine figure `raidsim` defaults to — if that number moves, move
/// both toward a shared home rather than letting them drift. `pub` because
/// the raid flow paces its footholds with the same legs.
pub const SQUAD_SPEED_MS: f32 = 14.0;

/// Ground clearance for a standing body when the shell has not said: the
/// sandbox car's `half_h`. A GUESS in metres, which `docs/START_HERE.md`
/// warns about — the shell overrides it with the live value
/// (`set_ride_h`) so this only matters to headless callers.
pub const DEFAULT_RIDE_H: f32 = 0.19;

/// Where the flow is. `Debrief` keeps the outcome so the shell can close
/// the sortie without re-asking the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Brief,
    Falling,
    Engage,
    Debrief { won: bool },
}

pub struct Sortie {
    pub mission: &'static Mission,
    pub dest: &'static Destination,
    /// the nightfall week, when this sortie is one — the card itself is
    /// re-derived where needed (`nightfall::card` is deterministic per
    /// week), so this struct does not have to know the card's type story
    pub week: Option<u64>,
    pub stage: Stage,
    pub drop: Drop,
    /// built at TOUCHDOWN, not at launch — the landing penalty has to exist
    /// before the run does, or `arrived_badly` becomes a patch instead of a
    /// constructor argument
    pub run: Option<MissionRun>,
    pub landing: Option<Landing>,
    /// gravity at the destination, m/s² (`mapsim::ground_for`)
    pub g: f32,
    /// the sky over the LZ, and what it allows
    pub weather: Weather,
    pub window_s: f32,
    /// the foothold question: candidate sites, their assessments, and
    /// which one the cursor is on. `plans[i]` prices `sites[i]`.
    pub sites: Vec<Site>,
    pub plans: Vec<Plan>,
    pub site_sel: usize,
    /// The cutscene cursor: which of the stage's slides (`Mission::intro` on the brief,
    /// `Mission::outro` on a won debrief) is showing; `slides().len()` once they are done.
    pub slide: usize,
    /// unit bearing from the origin to the LZ, hashed off the mission id;
    /// the chosen site's distance scales it (`lz_now`)
    lz_dir: Vec3,
    /// does the chosen approach leave the squad fighting in the open under
    /// weather? Feeds `MissionRun`'s exposure dial at touchdown.
    site_exposure: bool,
    /// the flyover, live from launch to touchdown. Not rendered yet; held
    /// so the renderer seam is a draw call away rather than a redesign.
    pub hull: Option<BigBody>,
    /// hull wear read at accept (`stations::ship_wear`) — a pinned ship's
    /// past, dirtying this pass's bank until yard time clears it
    pub wear: f32,
    /// flak already charged to the body this descent, and the note throttle
    aa_applied: f32,
    aa_note_t: f32,
    /// event ticker, newest last; the HUD shows the tail
    pub feed: Vec<String>,
    /// lines the shell should toast, drained by `pop_toast`
    toasts: Vec<String>,
    /// valence at accept time — resolved once, same as the resolved path
    have: u32,
    /// ground height under the drop last step, for the HUD's prediction
    last_gy: f32,
    /// THE WAVE, IN THE WORLD. Built at touchdown around the objective
    /// site; mustered on every `WaveStart`, mirrored from the fight every
    /// tick, stepped by the shell with the player's position. See
    /// `field.rs` for the seam.
    pub field: Field,
    /// ground clearance for a standing body, handed in at accept so the
    /// field can be rebuilt at touchdown without asking the shell again
    ride_h: f32,
}

/// Is this order one the flow can fly? Returns the mission id and the
/// nightfall week; `None` means "not ours" — either not a sortie at all, or
/// a docked destination, which stays on the resolved path.
pub fn order_flyable(o: &stations::Order) -> Option<(&'static str, Option<u64>)> {
    let (id, week) = match o {
        stations::Order::Sortie(id) => (*id, None),
        stations::Order::Nightfall(w) => (crate::nightfall::card(*w).mission.id, Some(*w)),
        _ => return None,
    };
    let m = crate::content::MISSIONS.iter().find(|m| m.id == id)?;
    let d = ROUTE.iter().find(|d| d.id == m.dest)?;
    if stations::is_dock(d) {
        return None;
    }
    Some((m.id, week))
}

/// The sky over a destination, as a `survival::Weather`. DERIVED STOPGAP
/// with its name on it: the route table only says whether air moves
/// (`Destination.weather` gates dunes, per its own doc), not what the air
/// does — so a weather world gets the dust storm the dune gate implies,
/// and flares, waves and thermal fronts stay raid-arena weather until the
/// route table can say otherwise.
pub fn weather_for(d: &Destination) -> Weather {
    if d.weather {
        Weather::DustStorm
    } else {
        Weather::Clear
    }
}

/// The two ways onto a mission's ground, derived from the film the way
/// `raidgen::phase_raid` derives its sites from the boss tier: open ground
/// close and quick and naked, the fortress far and slow and shielded.
/// Nothing here is authored per mission — a mission's foothold is a
/// property of how deep into the campaign it sits, until content wants to
/// name exceptions.
pub fn sites_for(m: &Mission) -> Vec<Site> {
    let t = m.film as f32;
    vec![
        Site {
            approach: Approach::ClearLz,
            at: Bunker { distance_m: 180.0 + 30.0 * t, shielded: false },
            garrison: 0.3 + 0.1 * t,
            take_s: 45.0 + 15.0 * t,
        },
        Site {
            approach: Approach::SeizeFortress,
            at: Bunker { distance_m: 420.0 + 60.0 * t, shielded: true },
            garrison: 0.8 + 0.2 * t,
            take_s: 120.0 + 30.0 * t,
        },
    ]
}

impl Sortie {
    /// Accept the order. Loads the save once for the valence standing —
    /// the same read `run_order` does, at the same moment (accept), so the
    /// two paths price the engagement identically — and asks the foothold
    /// question while there is still a choice to make.
    pub fn begin(mission_id: &str, week: Option<u64>) -> Result<Sortie, &'static str> {
        let m = crate::content::MISSIONS
            .iter()
            .find(|m| m.id == mission_id)
            .ok_or("no such mission")?;
        let d = ROUTE
            .iter()
            .find(|d| d.id == m.dest)
            .ok_or("mission names a destination the route does not")?;
        if stations::is_dock(d) {
            return Err("docked destinations berth; they do not drop");
        }
        let (sv, _binds, _cfg) = crate::save::load();
        let camp = crate::campaign::CampaignLog::from_line(&sv.missions);
        let raids = crate::raidlog::RaidLog::from_line(&sv.wardens);
        let nf = crate::nightfall::NightfallLog::from_line(&sv.nightfalls);
        let have = crate::valence::valence(&camp, &raids, nf.count());
        let g = crate::mapsim::ground_for(d).g;

        // THE FOOTHOLD QUESTION, at runtime at last. Assess both sites
        // against this sky; preselect what doctrine would pick
        // (`foothold::choose`), so Enter-without-thinking is the textbook
        // answer and the arrows are for disagreeing with it.
        let weather = weather_for(d);
        let window_s = crate::raid::window_base_s(weather);
        let sites = sites_for(m);
        let plans: Vec<Plan> = sites
            .iter()
            .map(|s| foothold::assess(Body::Machine, s, weather, window_s, SQUAD_SPEED_MS))
            .collect();
        let site_sel = foothold::choose(Body::Machine, &sites, weather, window_s, SQUAD_SPEED_MS)
            .and_then(|p| plans.iter().position(|q| q.site.approach == p.site.approach))
            .unwrap_or(0);

        let h = strhash(m.id);
        let ang = (h % 6283) as f32 / 1000.0;
        let lz_dir = Vec3::new(ang.cos(), 0.0, ang.sin());

        Ok(Sortie {
            mission: m,
            dest: d,
            week,
            stage: Stage::Brief,
            // a placeholder until launch rebuilds it for the CHOSEN site —
            // the brief needs a Drop to exist, not to be right yet
            drop: Drop::new(lz_dir * sites[site_sel].at.distance_m, false),
            run: None,
            landing: None,
            g,
            weather,
            window_s,
            sites,
            plans,
            site_sel,
            slide: 0,
            lz_dir,
            site_exposure: false,
            hull: None,
            wear: stations::ship_wear(),
            aa_applied: 0.0,
            aa_note_t: 0.0,
            feed: Vec::new(),
            toasts: Vec::new(),
            have,
            last_gy: 0.0,
            field: Field::for_sortie(Vec3::ZERO, lz_dir, m.id, DEFAULT_RIDE_H),
            ride_h: DEFAULT_RIDE_H,
        })
    }

    /// The shell's ground clearance for a standing body, so the field
    /// seats foes the way the player is seated. Set before launch; the
    /// field is rebuilt with it at touchdown.
    pub fn set_ride_h(&mut self, h: f32) {
        self.ride_h = h.max(0.01);
    }

    /// The cutscene slides for this stage. `content::Mission` has carried three intro slides
    /// and an outro for every mission since the campaign was authored -- "shown over a dim
    /// backdrop before and after the mission, advanced or skipped", per its own doc -- and
    /// the only thing that ever printed them was the headless `campsim`. The brief shows the
    /// intro, a won debrief shows the outro; a lost one has nothing to say but the loss.
    pub fn slides(&self) -> &'static [&'static str] {
        match self.stage {
            Stage::Brief => self.mission.intro,
            Stage::Debrief { won: true } => self.mission.outro,
            _ => &[],
        }
    }

    /// The slide on screen, or `None` when the stage's own panel is.
    pub fn slide_now(&self) -> Option<&'static str> {
        self.slides().get(self.slide).copied()
    }

    /// Enter during the slides: the next one. `true` when a slide was consumed, so the caller
    /// does nothing else with the key; `false` once the stage's panel is up.
    pub fn next_slide(&mut self) -> bool {
        if self.slide < self.slides().len() {
            self.slide += 1;
            true
        } else {
            false
        }
    }

    /// Esc during the slides: straight to the stage's panel. `true` if there was anything to
    /// skip -- otherwise Esc keeps its stage meaning (stand down / log it).
    pub fn skip_slides(&mut self) -> bool {
        let had = self.slide < self.slides().len();
        self.slide = self.slides().len();
        had
    }

    pub fn falling(&self) -> bool {
        self.stage == Stage::Falling
    }
    pub fn engaged(&self) -> bool {
        self.stage == Stage::Engage
    }

    /// Walk the brief's cursor over the sites.
    pub fn site_cycle(&mut self, d: i32) {
        if self.stage != Stage::Brief || self.sites.is_empty() {
            return;
        }
        let n = self.sites.len() as i32;
        self.site_sel = ((self.site_sel as i32 + d).rem_euclid(n)) as usize;
    }

    /// Where the CHOSEN site puts the mark.
    pub fn lz_now(&self) -> Vec3 {
        self.lz_dir * self.sites[self.site_sel].at.distance_m
    }

    /// Out the door, onto the chosen site — refused if its plan is not
    /// viable, with the plan's own note, because a launch button that
    /// flies you into a closed window is worse than one that says why not.
    /// Returns whether the launch happened.
    pub fn launch(&mut self, ground_y: f32) -> bool {
        if self.stage != Stage::Brief {
            return false;
        }
        let plan = self.plans[self.site_sel];
        if !plan.viable {
            self.push_toast(format!("{} — {}", plan.site.approach.label(), plan.note));
            return false;
        }
        // WHAT THE APPROACH BUYS AND COSTS, wired to the machines that
        // already price such things:
        //  * open ground is undefended — nobody garrisons empty rock, and
        //    AA is fortress kit — but taking it under weather leaves the
        //    squad fighting in the open: MissionRun's exposure dial (no
        //    field repair between waves), the cost foothold always claimed.
        //  * the fortress shoots at you on the way down (its AA is why it
        //    is worth having) and shelters you after: no exposure,
        //    whatever the sky is doing.
        let approach = plan.site.approach;
        let defended = match approach {
            Approach::ClearLz => false,
            Approach::SeizeFortress => self.dest.hostiles != Faction::None,
        };
        self.site_exposure =
            approach.exposed_during_take() && !matches!(self.weather, Weather::Clear);
        let lz = self.lz_now();

        // THE PASS, from the shared dropship module — the same flyover a
        // raid phase flies, wear-dirtied bank and all.
        match dropship::launch(lz, defended, self.wear, ground_y, strhash(self.mission.id)) {
            Ok((hull, drop)) => {
                self.drop = drop;
                self.hull = Some(hull);
            }
            Err(e) => {
                self.push_toast(e.to_string());
                return false;
            }
        }
        self.stage = Stage::Falling;
        self.push_toast(format!(
            "out the door over {} — {} it is",
            self.dest.name,
            approach.label()
        ));
        true
    }

    /// The fold, on the jump edge. The drop machine owns the hero-window
    /// arithmetic; this just needs the altitude it happened at.
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

    /// One fixed step of whichever machine is live. `steer` is the lateral
    /// stick; `ground_y` the world's height under the drop; `body` is the
    /// player's own sections, because the descent now CHARGES its costs —
    /// flak lands low as it is taken, the impact lands in the legs.
    pub fn update(&mut self, dt: f32, steer: Vec3, ground_y: f32, body: &mut sections::Body) {
        self.last_gy = ground_y;
        match self.stage {
            Stage::Falling => {
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
            Stage::Engage => self.tick_engage(dt),
            _ => {}
        }
    }

    fn touchdown(&mut self, l: Landing, body: &mut sections::Body) {
        self.hull = None; // the pass flew on
        let penalty = stations::landing_penalty(&l);
        self.push_toast(dropship::landing_line(&l, penalty));
        if let Some(t) = dropship::charge_landing(&l, body, &mut self.feed) {
            self.push_toast(t.to_string());
        }
        if l.aa_taken > 0.0 || l.self_dmg > 0.0 {
            self.feed.push(format!(
                "descent cost: {:.0} flak, {:.0} impact",
                l.aa_taken, l.self_dmg
            ));
        }
        if self.site_exposure {
            self.feed.push("in the open under weather — no field repair out here".into());
        }
        let squad: Vec<&str> = DEFAULT_SQUAD.to_vec();
        let mut run = MissionRun::new(self.mission, &squad, DEFAULT_DOCTRINE, self.have);
        if penalty > 0.0 {
            run = run.arrived_badly(penalty);
        }
        // the nightfall's dials and the foothold's compose: the burn is
        // the card's alone, exposure is EITHER saying you fight unrepaired
        let (burn, card_exposure) = match self.week {
            Some(w) => {
                let c = crate::nightfall::card(w);
                (Some(c.burn), c.exposure)
            }
            None => (None, false),
        };
        run = run.with_modifiers(burn, card_exposure || self.site_exposure);
        self.landing = Some(l);
        self.run = Some(run);
        // the wave will stand SITE_AHEAD_M past the LZ along the drop
        // bearing; the first `WaveStart` musters it there
        let lz = self.lz_now();
        self.field = Field::for_sortie(
            Vec3::new(lz.x, self.last_gy, lz.z),
            self.lz_dir,
            self.mission.id,
            self.ride_h,
        );
        self.stage = Stage::Engage;
    }

    /// Step the engagement.
    pub fn tick_engage(&mut self, dt: f32) {
        let Some(run) = self.run.as_mut() else { return };
        for ev in run.step(dt) {
            match ev {
                CampEvent::WaveStart { wave, .. } => {
                    self.field.muster(wave, run.foes(), self.mission.champion);
                }
                CampEvent::WaveCleared { .. } => self.field.clear(),
                _ => {}
            }
            Self::note(&mut self.feed, &mut self.toasts, ev);
        }
        // the fight's own casualties reach the world through the mirror
        self.field.sync(run.foes());
        if let Some(won) = run.outcome() {
            self.stage = Stage::Debrief { won };
            self.slide = 0;
        }
        if self.feed.len() > 64 {
            let cut = self.feed.len() - 64;
            self.feed.drain(..cut);
        }
    }

    /// A world hit on the hostile standing in `slot` (`field.rs`). The
    /// element ring is resolved against THAT body; the fight applies its
    /// own shield and evasion; the agent is woken so a foe shot from
    /// cover turns to face you. This is the aimed door — the shell uses
    /// it for every hit that lands on a hostile bot.
    pub fn strike_slot(&mut self, slot: usize, dmg: f32, shooter: Element, reg: Register) {
        if self.stage != Stage::Engage {
            return;
        }
        let Some(run) = self.run.as_mut() else { return };
        // exposure is the field's: caught mid-fold, or on the end of a tow
        let towed = self.field.towed(slot);
        let dmg = dmg * self.field.foe(slot).map(|f| f.exposure(towed)).unwrap_or(1.0);
        let ring = run.foe_element(slot).map(|d| shooter.advantage(d)).unwrap_or(1.0);
        if let Some(ev) = run.player_strike_slot(slot, dmg, ring) {
            Self::note(&mut self.feed, &mut self.toasts, ev);
        }
        // the register's answer: stagger, suppression, a stalled fold
        self.field.hit(slot, dmg * ring, reg);
    }

    /// The player is down on the field: the sortie is lost, whatever the
    /// squad ghosts were doing. Nothing is recorded until the debrief.
    pub fn player_down(&mut self) {
        if self.stage != Stage::Engage {
            return;
        }
        let Some(run) = self.run.as_mut() else { return };
        if let Some(ev) = run.concede() {
            Self::note(&mut self.feed, &mut self.toasts, ev);
            self.toasts.push("you are down — the sortie is lost".into());
        }
        self.field.clear();
        self.stage = Stage::Debrief { won: false };
        self.slide = 0;
    }

    /// The player's shot, into the fight, AIM-AGNOSTIC: lands on the
    /// hostile leading the line whatever the shell was pointing at. This
    /// was the shell's whole bridge before `field.rs` gave hostiles
    /// positions; the shell now uses `strike_slot`. Kept for headless
    /// callers and the tests that pin the conversion. `dmg` is the
    /// shell's conversion of the weapon's REAL numbers (see
    /// `fire_weapon`); attuning fires as `Element::None` and takes
    /// whatever the advantage table says that is worth.
    pub fn strike(&mut self, dmg: f32, shooter: Element) {
        if self.stage != Stage::Engage {
            return;
        }
        let Some(run) = self.run.as_mut() else { return };
        let ring = run
            .lead_foe_element()
            .map(|d| shooter.advantage(d))
            .unwrap_or(1.0);
        if let Some(ev) = run.player_strike(dmg, ring) {
            Self::note(&mut self.feed, &mut self.toasts, ev);
        }
    }

    /// One event, one line, following `campsim`'s narration so the flown
    /// mission and the terminal sim describe the same fight the same way.
    fn note(feed: &mut Vec<String>, toasts: &mut Vec<String>, ev: CampEvent) {
        match ev {
            CampEvent::WaveStart { wave, of, size } => {
                let s = format!("wave {wave}/{of} — {size} inbound");
                toasts.push(s.clone());
                feed.push(s);
            }
            CampEvent::EnemyDown { wave_slot } => feed.push(format!("hostile {} down", wave_slot + 1)),
            CampEvent::SquadDown { slot } => {
                let name = DEFAULT_SQUAD.get(slot).copied().unwrap_or("?");
                let s = format!("we lost {name}");
                toasts.push(s.clone());
                feed.push(s);
            }
            CampEvent::WaveCleared { wave } => {
                let s = format!("wave {wave} cleared");
                toasts.push(s.clone());
                feed.push(s);
            }
            CampEvent::StructureHit { hp_left } => feed.push(format!("charge set — {hp_left:.0} to breach")),
            CampEvent::AssetHit { hp_left } => feed.push(format!("the asset is taking fire — {hp_left:.0}")),
            CampEvent::Hold { .. } => {} // the meter is the HUD's, not the ticker's
            CampEvent::Won => toasts.push("OBJECTIVE COMPLETE".into()),
            CampEvent::Lost => toasts.push("the squad did not hold".into()),
        }
    }

    /// Close out the debrief. A win commits through the one persistence
    /// path; a loss writes nothing, same as the resolved path. Returns the
    /// closing line.
    pub fn commit(&self) -> String {
        let Stage::Debrief { won } = self.stage else {
            return "sortie abandoned — no result recorded".into();
        };
        // THE PINNED SHIP WEARS, win or lose — foothold always said an LZ
        // approach pins the hull, `raid::lz_wear` prices a second on the
        // deck, and the engagement's own clock says how many there were.
        // Charged at debrief either way, because the ship sat there
        // whatever the squad did; only walking away mid-fight skips it.
        let mut tail = String::new();
        if self.plans[self.site_sel].site.approach == Approach::ClearLz {
            if let Some(r) = self.run.as_ref() {
                let w = crate::raid::lz_wear(r.t);
                stations::record_ship_wear(w);
                tail = format!(" · hull wore +{w:.1} on the deck");
            }
        }
        if !won {
            return format!("{} — lost. nothing recorded{tail}", self.mission.id);
        }
        let banked = stations::record_outcome(self.mission.id, self.week);
        let what = if self.week.is_some() { "nightfall" } else { "sortie" };
        let rewards = banked.lines().join(" · ");
        let tail = if rewards.is_empty() { tail } else { format!(" · {rewards}{tail}") };
        format!(
            "{} — {what} CLEARED{}{tail}",
            self.mission.id,
            if banked.fresh { "" } else { "; already on the board" }
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

    /// The HUD's lines for the current stage. Computed here so the HUD
    /// stays a renderer — the flow knows what matters, the panel only knows
    /// where text goes.
    pub fn hud_rows(&self) -> Vec<String> {
        let mut rows = Vec::new();
        if let Some(text) = self.slide_now() {
            // a slide owns the panel: the mission's name, the prose, and how to move on
            let n = self.slides().len();
            rows.push(format!(
                "{} — {}/{}",
                match self.stage {
                    Stage::Brief => "SORTIE",
                    _ => "DEBRIEF",
                },
                self.slide + 1,
                n
            ));
            rows.push(String::new());
            rows.extend(wrap(text, SLIDE_COLS));
            rows.push(String::new());
            rows.push(if self.slide + 1 < n { "ENTER next · ESC skip".into() } else { "ENTER continue".into() });
            return rows;
        }
        match self.stage {
            Stage::Brief => {
                rows.push(format!("SORTIE — {}", self.mission.id));
                let window = if self.window_s.is_finite() {
                    format!("window {:.0}s", self.window_s)
                } else {
                    "open sky".into()
                };
                rows.push(format!(
                    "{} · {:?} · {} ({window})",
                    self.dest.name,
                    self.mission.objective,
                    self.weather.label()
                ));
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
                        "{mark} {} — {:.0} m · take {:.0}s · {}",
                        p.site.approach.label(),
                        p.site.at.distance_m,
                        p.site.take_s,
                        verdict
                    ));
                }
                rows.push("ENTER launch · UP/DOWN site · ESC stand down".into());
            }
            Stage::Falling => {
                let alt = (self.drop.pos.y - self.last_gy).max(0.0);
                let miss = {
                    let p = self.drop.predicted(self.g, self.last_gy);
                    (Vec3::new(p.x, 0.0, p.z) - Vec3::new(self.drop.lz.x, 0.0, self.drop.lz.z))
                        .length()
                };
                rows.push(format!(
                    "DROP · {}",
                    match self.drop.mode {
                        Mode::Vehicle => "folded",
                        Mode::Robot => "limbs out",
                    }
                ));
                rows.push(format!("alt {alt:.0} m · sink {:.0} m/s", -self.drop.vel.y));
                rows.push(format!("predicted miss {miss:.0} m"));
                rows.push("steer to the mark · JUMP folds".into());
            }
            Stage::Engage => {
                if let Some(r) = self.run.as_ref() {
                    rows.push(format!("ENGAGED — {}", self.mission.id));
                    rows.push(format!(
                        "wave {}/{} · squad {} · foes {}",
                        r.wave_now(),
                        r.waves_total(),
                        r.squad_alive(),
                        r.foes_alive()
                    ));
                    let (label, frac) = r.meter();
                    rows.push(format!("{label} {:.0}%", frac * 100.0));
                    rows.push("your fire lands on the line · ESC abandons".into());
                }
            }
            Stage::Debrief { won } => {
                rows.push(if won { "OBJECTIVE COMPLETE".into() } else { "MISSION LOST".into() });
                rows.push(format!(
                    "{} · {}",
                    self.plans[self.site_sel].site.approach.label(),
                    match self.landing {
                        Some(l) => format!("{:.0} m off the mark", l.miss_distance),
                        None => "no landing recorded".into(),
                    }
                ));
                rows.push("ENTER to log it and stand down".into());
            }
        }
        // the tail of the ticker rides under the stage block
        for line in self.feed.iter().rev().take(4).rev() {
            rows.push(format!("· {line}"));
        }
        rows
    }
}

/// Characters per slide line. The sortie panel is 0.48 of the screen wide at the row glyph
/// size, which fits about this many; a slide is up to ~100 characters, so most wrap to two.
pub const SLIDE_COLS: usize = 52;

/// Word-wrap `text` to lines of at most `cols` characters (a single word longer than `cols`
/// stands alone on its line rather than being cut).
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let wl = word.chars().count();
        let cl = cur.chars().count();
        if cl > 0 && cl + 1 + wl > cols {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Abandoning a sortie on the field takes TWO presses of Esc within [`Abandon::WINDOW`]
/// seconds. One press used to drop the whole run with "nothing recorded" -- the same key that
/// backs out of every menu, pressed once by reflex, mid-fight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Abandon {
    armed_at: Option<f32>,
}

impl Abandon {
    pub const WINDOW: f32 = 3.0;

    /// A press at `now`: `true` when it confirms (an arming press within the window came
    /// before it); `false` arms (or re-arms, if the last one has expired).
    pub fn press(&mut self, now: f32) -> bool {
        match self.armed_at {
            Some(t) if now - t <= Self::WINDOW && now >= t => {
                self.armed_at = None;
                true
            }
            _ => {
                self.armed_at = Some(now);
                false
            }
        }
    }

    /// Still waiting for the confirming press at `now`?
    pub fn armed(&self, now: f32) -> bool {
        matches!(self.armed_at, Some(t) if now - t <= Self::WINDOW && now >= t)
    }

    pub fn disarm(&mut self) {
        self.armed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoning_takes_a_second_press_inside_the_window() {
        let mut a = Abandon::default();
        assert!(!a.press(10.0), "the first press arms");
        assert!(a.armed(11.0));
        assert!(a.press(12.5), "the second, inside the window, confirms");
        assert!(!a.armed(12.6), "and nothing is armed afterwards");
        assert!(!a.press(20.0));
        assert!(!a.press(20.0 + Abandon::WINDOW + 0.1), "too late: this press re-arms instead");
        assert!(a.armed(23.2));
        a.disarm();
        assert!(!a.armed(23.3));
        assert!(!a.press(30.0));
        assert!(!a.press(29.0), "a clock that went backwards does not confirm");
    }

    /// Autopilot a begun sortie to touchdown, competently — fold high,
    /// steer at the mark — so tests can reach Engage without a screen.
    /// Returns the body that flew it, for tests that care what it cost.
    fn fly_down(s: &mut Sortie) -> sections::Body {
        let mut b = sections::Body::default();
        assert!(s.launch(0.0), "the chosen site refused the launch");
        assert_eq!(s.stage, Stage::Falling);
        for _ in 0..(60 * 120) {
            let alt = s.drop.pos.y;
            if alt < 120.0 && s.drop.mode == Mode::Vehicle {
                s.fold();
            }
            let to = (s.drop.lz - s.drop.pos) * Vec3::new(1.0, 0.0, 1.0);
            s.update(1.0 / 60.0, (to * 0.05).clamp_length_max(1.0), 0.0, &mut b);
            if s.stage != Stage::Falling {
                return b;
            }
        }
        panic!("the drop never landed");
    }

    fn hp_total(b: &sections::Body) -> f32 {
        use sections::Section::*;
        [Head, Torso, ArmL, ArmR, LegL, LegR].iter().map(|x| b.hp_of(*x)).sum()
    }

    fn first_mission() -> &'static str {
        // any mission on a non-dock destination; the route has plenty
        crate::content::MISSIONS
            .iter()
            .find(|m| {
                ROUTE
                    .iter()
                    .find(|d| d.id == m.dest)
                    .map(|d| !stations::is_dock(d))
                    .unwrap_or(false)
            })
            .expect("no flyable mission in content")
            .id
    }

    #[test]
    fn the_flow_reaches_the_fight_through_the_air() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        assert_eq!(s.stage, Stage::Brief);
        fly_down(&mut s);
        assert_eq!(s.stage, Stage::Engage, "touchdown did not open the engagement");
        assert!(s.landing.is_some());
        assert!(s.run.is_some(), "the run must exist only after the landing priced it");
    }

    #[test]
    fn a_docked_destination_refuses_the_drop() {
        // outer_rim is DeepSpace in the route table; you berth, you do not fall
        if let Some(m) = crate::content::MISSIONS.iter().find(|m| {
            ROUTE.iter().find(|d| d.id == m.dest).map(|d| stations::is_dock(d)).unwrap_or(false)
        }) {
            assert!(Sortie::begin(m.id, None).is_err());
            assert!(order_flyable(&stations::Order::Sortie(m.id)).is_none());
        }
    }

    #[test]
    fn the_brief_asks_the_foothold_question() {
        let s = Sortie::begin(first_mission(), None).unwrap();
        assert_eq!(s.sites.len(), 2, "two ways onto the ground");
        assert_eq!(s.plans.len(), s.sites.len());
        // the preselection is doctrine's answer: a viable plan, when one exists
        if s.plans.iter().any(|p| p.viable) {
            assert!(s.plans[s.site_sel].viable, "the cursor starts on a refusal");
        }
        // and the two sites genuinely differ where the drop cares
        assert!(s.sites[0].at.distance_m < s.sites[1].at.distance_m);
        assert!(!s.sites[0].at.shielded && s.sites[1].at.shielded);
    }

    #[test]
    fn choosing_the_other_site_moves_the_mark() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        s.site_sel = 0;
        let near = s.lz_now();
        s.site_cycle(1);
        let far = s.lz_now();
        assert!(far.length() > near.length(), "the fortress is not further out");
        // same bearing, different reach: the choice moves the mark, not the map
        assert!(near.normalize_or_zero().dot(far.normalize_or_zero()) > 0.99);
    }

    #[test]
    fn the_engagement_ends_and_the_flow_notices() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        fly_down(&mut s);
        // drive the fight to a decision the way the shell would, capped the
        // way `resolve` caps — a mission that cannot end is a bug
        for _ in 0..(600.0 / 0.1) as usize {
            s.tick_engage(0.1);
            if matches!(s.stage, Stage::Debrief { .. }) {
                return;
            }
        }
        panic!("the engagement never reached a debrief");
    }

    #[test]
    fn the_player_can_actually_kill_something() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        fly_down(&mut s);
        // one tick to muster the first wave, then lean on the trigger with
        // absurd force. A shielded lead foe eats the first hit on the
        // shield — the rules apply to us too — so allow a few strikes.
        s.tick_engage(0.05);
        let before = s.run.as_ref().unwrap().foes_alive();
        assert!(before > 0, "the first wave never mustered");
        for _ in 0..4 {
            s.strike(1.0e6, Element::None);
        }
        assert!(
            s.run.as_ref().unwrap().foes_alive() < before,
            "overwhelming fire through the real rules killed nobody"
        );
    }

    #[test]
    fn the_wave_stands_in_the_world_and_an_aimed_hit_lands_on_that_body() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        fly_down(&mut s);
        assert!(s.field.foes.is_empty(), "foes stood up before the first wave mustered");
        s.tick_engage(0.05);
        let run = s.run.as_ref().unwrap();
        assert_eq!(s.field.alive(), run.foes_alive(), "the field and the fight disagree on who is standing");
        assert!(s.field.alive() > 0);
        // they muster past the LZ along the drop bearing, not on top of you
        let lz = s.lz_now();
        for f in &s.field.foes {
            let d = (f.pos() - lz) * Vec3::new(1.0, 0.0, 1.0);
            assert!(d.length() > 20.0, "foe {} mustered {:.0} m from the LZ", f.slot, d.length());
        }
        // an aimed hit on slot 1 costs slot 1, and only slot 1
        let hp_before: Vec<f32> = run.foes().iter().map(|f| f.hp).collect();
        let target = s.field.foes.iter().map(|f| f.slot).nth(1).unwrap_or(0);
        for _ in 0..4 {
            s.strike_slot(target, 40.0, Element::None, Register::Medium);
        }
        let run = s.run.as_ref().unwrap();
        let hp_after: Vec<f32> = run.foes().iter().map(|f| f.hp).collect();
        assert!(hp_after[target] < hp_before[target], "the aimed body took nothing");
        for (i, (a, b)) in hp_before.iter().zip(hp_after.iter()).enumerate() {
            if i != target {
                assert!((a - b).abs() < 1e-3, "slot {i} was hit by fire aimed at slot {target}");
            }
        }
        // the woken foe is on the field, awake
        assert_ne!(s.field.foe(target).unwrap().agent.state, crate::enemy::State::Idle);
        // and a kill through the aimed door comes off the field on the next mirror
        for _ in 0..40 {
            s.strike_slot(target, 1.0e6, Element::None, Register::Medium);
        }
        s.tick_engage(0.0);
        assert!(!s.field.foe(target).map(|f| f.alive()).unwrap_or(false), "killed in the fight, still standing in the world");
        assert_eq!(s.field.alive(), s.run.as_ref().unwrap().foes_alive());
    }

    #[test]
    fn going_down_loses_the_sortie_and_clears_the_field() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        fly_down(&mut s);
        s.tick_engage(0.05);
        assert!(s.field.alive() > 0);
        s.player_down();
        assert_eq!(s.stage, Stage::Debrief { won: false });
        assert!(s.field.foes.is_empty(), "the wave outlived the sortie");
        // idempotent, and the fight agrees it is over
        s.player_down();
        assert_eq!(s.run.as_ref().unwrap().outcome(), Some(false));
        assert!(s.feed.iter().any(|l| l.contains("down")) || !s.feed.is_empty());
    }

    #[test]
    fn a_cleared_wave_leaves_nobody_standing() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        fly_down(&mut s);
        s.tick_engage(0.05);
        let n = s.run.as_ref().unwrap().foes().len();
        for slot in 0..n {
            for _ in 0..40 {
                s.strike_slot(slot, 1.0e6, Element::None, Register::Medium);
            }
        }
        s.tick_engage(0.0);
        assert_eq!(s.field.alive(), 0, "the wave is dead in the fight and alive in the world");
        // the breather runs, then the next wave musters fresh bodies
        let mut mustered = false;
        for _ in 0..800 {
            s.tick_engage(1.0 / 60.0);
            if s.field.alive() > 0 {
                mustered = true;
                break;
            }
            if s.stage != Stage::Engage {
                break;
            }
        }
        assert!(mustered || s.stage != Stage::Engage, "no second wave and no ending either");
    }

    #[test]
    fn a_hard_landing_hurts_the_legs() {
        // vehicle terminal scales with sqrt(g): below ~0.25 g it never
        // beats SAFE_IMPACT and an unfolded landing is genuinely free, so
        // the test needs a world heavy enough for the mistake to exist
        let Some(m) = crate::content::MISSIONS.iter().find(|m| {
            ROUTE
                .iter()
                .find(|d| d.id == m.dest)
                .map(|d| !stations::is_dock(d) && d.gravity > 0.3)
                .unwrap_or(false)
        }) else {
            return;
        };
        let mut s = Sortie::begin(m.id, None).unwrap();
        s.site_sel = 0; // open ground: undefended, so every point of damage is the impact's
        assert!(s.launch(0.0));
        let mut b = sections::Body::default();
        for _ in 0..(60 * 120) {
            // never fold: arrive at vehicle terminal and wear it
            s.update(1.0 / 60.0, Vec3::ZERO, 0.0, &mut b);
            if s.stage != Stage::Falling {
                break;
            }
        }
        assert_eq!(s.stage, Stage::Engage, "never landed");
        let l = s.landing.unwrap();
        assert!(l.self_dmg > 0.0, "terminal velocity in vehicle mode was free");
        let fresh = sections::Body::default();
        use sections::Section::{LegL, LegR};
        assert!(
            b.hp_of(LegL) + b.hp_of(LegR) < fresh.hp_of(LegL) + fresh.hp_of(LegR),
            "the impact never reached the legs"
        );
    }

    #[test]
    fn a_fortress_drop_shoots_back_and_it_lands_on_the_body() {
        // needs a mission on a hostile world; if content has none, there is
        // nothing to assert
        let Some(m) = crate::content::MISSIONS.iter().find(|m| {
            ROUTE
                .iter()
                .find(|d| d.id == m.dest)
                .map(|d| !stations::is_dock(d) && d.hostiles != Faction::None)
                .unwrap_or(false)
        }) else {
            return;
        };
        let mut s = Sortie::begin(m.id, None).unwrap();
        s.site_sel = s
            .plans
            .iter()
            .position(|p| p.site.approach == Approach::SeizeFortress)
            .expect("no fortress site");
        if !s.launch(0.0) {
            return; // the window refused the fortress: the plan gate working is its own test
        }
        s.fold(); // limbs out at altitude: the slow, visible, expensive way down
        let mut b = sections::Body::default();
        for _ in 0..(60 * 180) {
            let to = (s.drop.lz - s.drop.pos) * Vec3::new(1.0, 0.0, 1.0);
            s.update(1.0 / 60.0, (to * 0.05).clamp_length_max(1.0), 0.0, &mut b);
            if s.stage != Stage::Falling {
                break;
            }
        }
        let l = s.landing.expect("never landed");
        assert!(l.aa_taken > 0.0, "a defended LZ never fired");
        assert!(
            hp_total(&b) < hp_total(&sections::Body::default()),
            "the flak never reached the body"
        );
    }

    #[test]
    fn the_lz_is_the_same_place_twice() {
        let mut a = Sortie::begin(first_mission(), None).unwrap();
        let mut b = Sortie::begin(first_mission(), None).unwrap();
        a.site_sel = 0;
        b.site_sel = 0;
        assert_eq!(a.lz_now(), b.lz_now());
        assert!(a.lz_now().length() > 100.0, "an LZ at the origin is not a navigation task");
    }

    /// The brief opens on the mission's first intro slide, Enter walks them, and the brief
    /// proper (sites, launch line) is only drawn once they are done. Every authored mission
    /// has intro slides, so a brief that opened on the sites would mean the prose is lost.
    #[test]
    fn the_brief_opens_on_the_intro_slides_and_enter_walks_them() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        let intro = s.mission.intro;
        assert!(intro.len() >= 2, "the fixture mission has slides to walk");
        assert_eq!(s.slide_now(), Some(intro[0]));
        let rows = s.hud_rows();
        assert!(rows[0].starts_with("SORTIE") && rows[0].ends_with(&format!("1/{}", intro.len())), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains(intro[0].split_whitespace().next().unwrap())), "the slide text is in the panel: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains("ENTER launch")), "the launch line waits behind the slides: {rows:?}");
        assert!(rows.iter().any(|r| r == "ENTER next · ESC skip"));
        for i in 1..intro.len() {
            assert!(s.next_slide());
            assert_eq!(s.slide_now(), Some(intro[i]));
        }
        assert!(s.hud_rows().iter().any(|r| r == "ENTER continue"), "the last slide says so");
        assert!(s.next_slide(), "the last slide is consumed by Enter too");
        assert_eq!(s.slide_now(), None);
        assert!(!s.next_slide(), "and then Enter is the launch key");
        let rows = s.hud_rows();
        assert_eq!(rows[0], format!("SORTIE — {}", s.mission.id));
        assert!(rows.iter().any(|r| r.contains("ENTER launch")), "{rows:?}");
        assert!(!s.skip_slides(), "nothing left to skip: Esc stands down");
    }

    /// Esc on a slide skips the rest rather than standing the sortie down.
    #[test]
    fn esc_skips_the_slides_once_and_then_means_stand_down() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        assert!(s.skip_slides());
        assert_eq!(s.slide_now(), None);
        assert!(!s.skip_slides());
    }

    /// The outro belongs to a WON debrief; a lost sortie has only the loss to show, and the
    /// cursor starts over when the stage changes.
    #[test]
    fn the_outro_shows_on_a_win_and_not_on_a_loss() {
        let mut s = Sortie::begin(first_mission(), None).unwrap();
        s.skip_slides();
        s.stage = Stage::Debrief { won: false };
        s.slide = 0;
        assert_eq!(s.slides(), &[] as &[&str]);
        assert_eq!(s.slide_now(), None);
        assert!(s.hud_rows().iter().any(|r| r == "MISSION LOST"));
        s.stage = Stage::Debrief { won: true };
        assert_eq!(s.slides(), s.mission.outro);
        assert_eq!(s.slide_now(), Some(s.mission.outro[0]));
        let rows = s.hud_rows();
        assert!(rows[0].starts_with("DEBRIEF"), "{rows:?}");
        assert!(!rows.iter().any(|r| r == "OBJECTIVE COMPLETE"), "the summary waits behind the slides");
        s.skip_slides();
        assert!(s.hud_rows().iter().any(|r| r == "OBJECTIVE COMPLETE"));
        // a stage change starts the cursor over: the player going down mid-fight, with a
        // stale cursor from the brief, lands on a debrief whose cursor is at zero
        s.stage = Stage::Engage;
        s.run = Some(MissionRun::new(s.mission, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, s.have));
        s.slide = 7;
        s.player_down();
        assert_eq!(s.stage, Stage::Debrief { won: false });
        assert_eq!(s.slide, 0);
    }

    /// Slides wrap at word boundaries to the panel's width; every authored slide fits.
    #[test]
    fn slides_wrap_at_words_and_every_authored_slide_fits_the_panel() {
        let lines = wrap("the system's edge. an old soldier in an older suit walks the wall he helped build.", 52);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 52), "{lines:?}");
        assert_eq!(lines.join(" "), "the system's edge. an old soldier in an older suit walks the wall he helped build.");
        assert_eq!(wrap("", 10), Vec::<String>::new());
        assert_eq!(wrap("supercalifragilistic", 5), vec!["supercalifragilistic".to_string()], "a long word stands alone, uncut");
        let mut slides = 0;
        for m in crate::content::MISSIONS {
            for t in m.intro.iter().chain(m.outro.iter()) {
                slides += 1;
                let w = wrap(t, SLIDE_COLS);
                assert!((1..=3).contains(&w.len()), "{}: a slide is one to three panel lines, not {}: {w:?}", m.id, w.len());
            }
        }
        assert!(slides >= 60, "the scan read the campaign: {slides} slides");
    }
}
