//! FLOW — the one path everything goes through.
//!
//! Before this file, every mode had its own idea of how a player
//! arrives: `overload.rs` used to open on its own `Boot`/`MainMenu`
//! (removed — see its module doc), and `campaign.rs`/`raid.rs`/
//! `patrol.rs` have never had an arrival sequence at all, just a
//! runnable state once something else decides to start one. This
//! module is that missing "something else": ONE ordered `Stage`,
//! shared by every destination, so launching the game and launching
//! OVERLOAD or a raid are the same kind of event at different depths,
//! not two unrelated systems that happen to share a binary.
//!
//! ```text
//! Boot -> Loading -> (first launch ever?)
//!                       yes -> IntroCutscene -> Campaign
//!                       no  -> CharacterHub -> { Campaign, Overload, Raid, Patrol }
//! ```
//!
//! **First-time vs returning** reads `Save::seen_intro`, set once the
//! intro finishes (`finish_intro`) and persisted like everything else
//! in `save.rs`. A first-time player is funneled straight into
//! Campaign — no hub, no mode choice, matching "first timers go into
//! campaign" — everyone after that lands on the hub and picks.
//!
//! **What each leaf actually is, honestly:**
//! * `Overload` wraps a real `overload::Overload` — the full inner
//!   flow (region select through a live `CircuitMatch`) already exists
//!   and is fully tested; this is the first time anything actually
//!   constructs one from a real launch path instead of a bare
//!   `Overload::new()` in a test.
//! * `Patrol` wraps a real `patrol::Patrol` — cheap to construct
//!   (`Patrol::default()`/`Patrol::new` already exist) and immediately
//!   playable headlessly.
//! * `Campaign` wraps a real `campaign::MissionRun` — the mission is
//!   `CampaignLog::next_mission()` (lowest `Mission::order` not yet in
//!   `Save::missions`, wrapping to the first once everything's
//!   cleared), squad/doctrine are `campaign::DEFAULT_SQUAD`/
//!   `DEFAULT_DOCTRINE` — the same fallback `sortie.rs`, `stations.rs`
//!   and this file's own tests already use anywhere a real squad
//!   picker hasn't been built. Not invented for this module.
//! * `Raid` is still a marker only: `raid::Run` needs a chosen BOSS,
//!   and there's no equivalent "next boss" ordering to fall back on —
//!   raids are Destiny-style repeatable endgame content, not a
//!   sequence, so picking one silently would be a real design
//!   decision this file isn't making. Reaching `Stage::Raid` is real
//!   and tested; which boss loads under it is the next, separate
//!   piece of work.

use crate::campaign::{CampaignLog, MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD};
use crate::overload::Overload;
use crate::patrol::Patrol;
use crate::save::Save;

/// (guess) — long enough to read a logo, short enough nobody minds.
pub const BOOT_S: f32 = 1.5;
/// (guess) — long enough to be a beat, not a wall.
pub const INTRO_S: f32 = 8.0;
/// (guess) — matches `overload::LOADING_S`'s own reasoning: costs
/// nothing here, easy to retune from one place.
pub const LOADING_S: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HubChoice {
    Campaign,
    Overload,
    Raid,
    Patrol,
}

pub enum Stage {
    Boot { t: f32 },
    Loading { t: f32 },
    IntroCutscene { t: f32 },
    CharacterHub { sel: HubChoice },
    Campaign(MissionRun),
    Overload(Overload),
    /// marker only — see the module doc's "what each leaf actually is"
    Raid,
    Patrol(Patrol),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    BootDone,
    LoadingDone,
    /// fires only on a first-ever launch, when the intro starts
    IntroStarted,
    IntroDone,
    /// fires only for a returning player, when the hub opens
    HubOpened,
    Entered(HubChoice),
}

pub struct Flow {
    pub stage: Stage,
    events: Vec<Event>,
}

fn start_campaign(save: &Save) -> MissionRun {
    let log = CampaignLog::from_line(&save.missions);
    let mission = log.next_mission();
    MissionRun::new(mission, &DEFAULT_SQUAD, DEFAULT_DOCTRINE, 0)
}

impl Flow {
    pub fn new() -> Flow {
        Flow { stage: Stage::Boot { t: 0.0 }, events: Vec::new() }
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Time-based advance for `Boot`/`Loading`/`IntroCutscene`; every
    /// other stage waits on an explicit call (`confirm_hub`,
    /// `finish_intro`). `save` is read only at the `Loading -> ...`
    /// branch, to decide first-time vs returning — nothing here writes
    /// to it; setting `seen_intro` is `finish_intro`'s job, done
    /// exactly once.
    pub fn update(&mut self, dt: f32, save: &Save) {
        match &mut self.stage {
            Stage::Boot { t } => {
                *t += dt;
                if *t >= BOOT_S {
                    self.events.push(Event::BootDone);
                    self.stage = Stage::Loading { t: 0.0 };
                }
            }
            Stage::Loading { t } => {
                *t += dt;
                if *t >= LOADING_S {
                    self.events.push(Event::LoadingDone);
                    if save.seen_intro {
                        self.events.push(Event::HubOpened);
                        self.stage = Stage::CharacterHub { sel: HubChoice::Campaign };
                    } else {
                        self.events.push(Event::IntroStarted);
                        self.stage = Stage::IntroCutscene { t: 0.0 };
                    }
                }
            }
            Stage::IntroCutscene { t } => {
                *t += dt;
                // does NOT auto-advance past INTRO_S on its own -- see
                // `finish_intro`, which is the actual transition and is
                // what sets `seen_intro`. `t` past `INTRO_S` is a valid,
                // observable "ready to skip/finish" state for a shell to
                // poll (`intro_can_finish`), not an automatic cut.
            }
            _ => {}
        }
    }

    /// `0..1` through the current timed stage, for a loading bar or a
    /// cutscene scrub — `1.0` outside a timed stage.
    pub fn progress(&self) -> f32 {
        match self.stage {
            Stage::Boot { t } => (t / BOOT_S).clamp(0.0, 1.0),
            Stage::Loading { t } => (t / LOADING_S).clamp(0.0, 1.0),
            Stage::IntroCutscene { t } => (t / INTRO_S).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    pub fn intro_can_finish(&self) -> bool {
        matches!(self.stage, Stage::IntroCutscene { t } if t >= INTRO_S)
    }

    /// `IntroCutscene -> Campaign`, and the ONLY place `seen_intro`
    /// becomes true. No-op before `intro_can_finish()` — a shell can
    /// still offer a "skip" input, but that input should itself wait
    /// for the same condition rather than calling this early, so a
    /// skip and a natural finish are indistinguishable here.
    pub fn finish_intro(&mut self, save: &mut Save) {
        if self.intro_can_finish() {
            let run = start_campaign(save);
            save.seen_intro = true;
            self.events.push(Event::IntroDone);
            self.stage = Stage::Campaign(run);
            self.events.push(Event::Entered(HubChoice::Campaign));
        }
    }

    /// Which hub row is highlighted, or `None` outside `CharacterHub`.
    pub fn stage_sel(&self) -> Option<HubChoice> {
        match self.stage {
            Stage::CharacterHub { sel } => Some(sel),
            _ => None,
        }
    }

    /// Move the hub cursor. No-op outside `CharacterHub`.
    pub fn hub_cycle(&mut self, d: i32) {
        if let Stage::CharacterHub { sel } = self.stage {
            let order = [HubChoice::Campaign, HubChoice::Overload, HubChoice::Raid, HubChoice::Patrol];
            let i = order.iter().position(|c| *c == sel).unwrap_or(0) as i32;
            let n = order.len() as i32;
            let next = ((i + d).rem_euclid(n)) as usize;
            self.stage = Stage::CharacterHub { sel: order[next] };
        }
    }

    /// Confirm the hub's current selection, entering that mode. No-op
    /// outside `CharacterHub`.
    pub fn confirm_hub(&mut self, seed: u64, save: &Save) {
        if let Stage::CharacterHub { sel } = self.stage {
            self.events.push(Event::Entered(sel));
            self.stage = match sel {
                HubChoice::Campaign => Stage::Campaign(start_campaign(save)),
                HubChoice::Overload => Stage::Overload(Overload::new()),
                HubChoice::Raid => Stage::Raid,
                HubChoice::Patrol => Stage::Patrol(Patrol::new(0, seed)),
            };
        }
    }

    /// Leave whatever mode is running and return to the hub. No-op if
    /// already at or before the hub (nothing to leave `Boot`/`Loading`/
    /// `IntroCutscene` back INTO — the hub only exists once a player
    /// has already seen the intro).
    pub fn return_to_hub(&mut self) {
        if matches!(self.stage, Stage::Campaign(_) | Stage::Overload(_) | Stage::Raid | Stage::Patrol(_)) {
            self.stage = Stage::CharacterHub { sel: HubChoice::Campaign };
        }
    }
}

impl Default for Flow {
    fn default() -> Flow {
        Flow::new()
    }
}

fn stage_name(s: &Stage) -> &'static str {
    match s {
        Stage::Boot { .. } => "Boot",
        Stage::Loading { .. } => "Loading",
        Stage::IntroCutscene { .. } => "IntroCutscene",
        Stage::CharacterHub { .. } => "CharacterHub",
        Stage::Campaign(_) => "Campaign",
        Stage::Overload(_) => "Overload",
        Stage::Raid => "Raid",
        Stage::Patrol(_) => "Patrol",
    }
}

/// `--flow`: walks both branches (first launch, and a returning
/// player entering each hub choice) once, printing every stage — so
/// the shared path can be read without a screen.
pub fn run() {
    println!("FLOW — first launch\n");
    let mut save = Save::default();
    let mut f = Flow::new();
    println!("{}", stage_name(&f.stage));
    for _ in 0..2000 {
        f.update(0.01, &save);
        for e in f.drain_events() {
            println!("  event: {e:?}");
        }
        if f.intro_can_finish() {
            break;
        }
    }
    println!("{}", stage_name(&f.stage));
    f.finish_intro(&mut save);
    println!("{}  (seen_intro={})", stage_name(&f.stage), save.seen_intro);

    println!("\nFLOW — returning player, each hub choice\n");
    for choice in [HubChoice::Campaign, HubChoice::Overload, HubChoice::Raid, HubChoice::Patrol] {
        let mut f2 = Flow::new();
        for _ in 0..2000 {
            f2.update(0.01, &save);
            if matches!(f2.stage, Stage::CharacterHub { .. }) {
                break;
            }
        }
        while f2.stage_sel() != Some(choice) {
            f2.hub_cycle(1);
        }
        f2.confirm_hub(42, &save);
        println!("  {choice:?} -> {}", stage_name(&f2.stage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_to_loading_done(f: &mut Flow, save: &Save) {
        for _ in 0..2000 {
            f.update(0.01, save);
            if !matches!(f.stage, Stage::Boot { .. } | Stage::Loading { .. }) {
                break;
            }
        }
    }

    #[test]
    fn first_time_launch_goes_boot_loading_intro_campaign() {
        let save = Save::default(); // seen_intro: false
        let mut f = Flow::new();
        assert!(matches!(f.stage, Stage::Boot { .. }));
        run_to_loading_done(&mut f, &save);
        assert!(matches!(f.stage, Stage::IntroCutscene { .. }), "first launch must hit the intro");
    }

    #[test]
    fn returning_player_skips_the_intro_straight_to_the_hub() {
        let mut save = Save::default();
        save.seen_intro = true;
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        assert!(matches!(f.stage, Stage::CharacterHub { .. }), "returning player must reach the hub directly");
    }

    #[test]
    fn intro_cannot_be_finished_before_its_own_duration() {
        let save = Save::default();
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        assert!(matches!(f.stage, Stage::IntroCutscene { .. }));
        assert!(!f.intro_can_finish());
        let mut save2 = save.clone();
        f.finish_intro(&mut save2); // must be a no-op: too early
        assert!(matches!(f.stage, Stage::IntroCutscene { .. }));
        assert!(!save2.seen_intro);
    }

    #[test]
    fn finishing_the_intro_sets_seen_intro_and_enters_campaign_directly() {
        let save = Save::default();
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        let mut save2 = save.clone();
        for _ in 0..2000 {
            f.update(0.01, &save2);
            if f.intro_can_finish() {
                break;
            }
        }
        f.finish_intro(&mut save2);
        assert!(save2.seen_intro);
        assert_eq!(stage_name(&f.stage), "Campaign");
        // first-timers go straight to Campaign -- no hub stop
        assert!(!matches!(f.stage, Stage::CharacterHub { .. }));
    }

    #[test]
    fn campaign_starts_on_the_real_next_mission_for_this_save() {
        let mut save = Save::default();
        save.seen_intro = true;
        let expect = CampaignLog::from_line(&save.missions).next_mission();
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        f.confirm_hub(1, &save); // hub opens on Campaign already selected
        match &f.stage {
            Stage::Campaign(run) => assert_eq!(run.mission.id, expect.id),
            other => panic!("expected Campaign, got {}", stage_name(other)),
        }
    }

    #[test]
    fn a_returning_player_with_progress_gets_the_next_uncleared_mission() {
        let mut save = Save::default();
        save.seen_intro = true;
        let first = CampaignLog::from_line("").next_mission();
        save.missions = first.id.to_string();
        let expect = CampaignLog::from_line(&save.missions).next_mission();
        assert_ne!(expect.id, first.id, "test setup: this save must have real progress");

        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        f.confirm_hub(1, &save);
        match &f.stage {
            Stage::Campaign(run) => assert_eq!(run.mission.id, expect.id),
            other => panic!("expected Campaign, got {}", stage_name(other)),
        }
    }

    #[test]
    fn hub_cycle_wraps_both_directions() {
        let mut save = Save::default();
        save.seen_intro = true;
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        assert_eq!(f.stage_sel(), Some(HubChoice::Campaign));
        f.hub_cycle(-1);
        assert_eq!(f.stage_sel(), Some(HubChoice::Patrol));
        f.hub_cycle(1);
        assert_eq!(f.stage_sel(), Some(HubChoice::Campaign));
    }

    #[test]
    fn confirm_hub_enters_overload_with_a_real_overload_instance() {
        let mut save = Save::default();
        save.seen_intro = true;
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        f.hub_cycle(1); // -> Overload
        f.confirm_hub(7, &save);
        match &f.stage {
            Stage::Overload(o) => assert!(matches!(o.stage, crate::overload::Stage::RegionSelect { .. })),
            other => panic!("expected Overload, got {}", stage_name(other)),
        }
    }

    #[test]
    fn confirm_hub_enters_patrol_with_a_real_patrol_instance() {
        let mut save = Save::default();
        save.seen_intro = true;
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        f.hub_cycle(3); // -> Patrol
        f.confirm_hub(99, &save);
        assert!(matches!(f.stage, Stage::Patrol(_)));
    }

    #[test]
    fn confirm_hub_only_fires_from_the_hub() {
        let save = Save::default(); // first-time: never reaches the hub
        let mut f = Flow::new();
        run_to_loading_done(&mut f, &save);
        assert!(matches!(f.stage, Stage::IntroCutscene { .. }));
        f.confirm_hub(1, &save); // no-op: not in the hub
        assert!(matches!(f.stage, Stage::IntroCutscene { .. }));
    }

    #[test]
    fn return_to_hub_works_from_every_mode_and_nowhere_else() {
        let mut save = Save::default();
        save.seen_intro = true;

        for choice in [HubChoice::Campaign, HubChoice::Overload, HubChoice::Raid, HubChoice::Patrol] {
            let mut f = Flow::new();
            run_to_loading_done(&mut f, &save);
            while f.stage_sel() != Some(choice) {
                f.hub_cycle(1);
            }
            f.confirm_hub(1, &save);
            f.return_to_hub();
            assert!(matches!(f.stage, Stage::CharacterHub { .. }), "{choice:?}");
        }

        // and NOT from Boot/Loading/IntroCutscene
        let mut f = Flow::new();
        f.return_to_hub();
        assert!(matches!(f.stage, Stage::Boot { .. }));
    }

    #[test]
    fn events_fire_in_order_for_a_first_time_launch() {
        let save = Save::default();
        let mut f = Flow::new();
        let mut all = Vec::new();
        for _ in 0..2000 {
            f.update(0.01, &save);
            all.extend(f.drain_events());
            if f.intro_can_finish() {
                break;
            }
        }
        let mut save2 = save.clone();
        f.finish_intro(&mut save2);
        all.extend(f.drain_events());
        assert_eq!(
            all,
            vec![
                Event::BootDone,
                Event::LoadingDone,
                Event::IntroStarted,
                Event::IntroDone,
                Event::Entered(HubChoice::Campaign)
            ]
        );
    }

    #[test]
    fn events_fire_in_order_for_a_returning_player() {
        let mut save = Save::default();
        save.seen_intro = true;
        let mut f = Flow::new();
        let mut all = Vec::new();
        for _ in 0..2000 {
            f.update(0.01, &save);
            all.extend(f.drain_events());
            if matches!(f.stage, Stage::CharacterHub { .. }) {
                break;
            }
        }
        f.confirm_hub(1, &save);
        all.extend(f.drain_events());
        assert_eq!(
            all,
            vec![Event::BootDone, Event::LoadingDone, Event::HubOpened, Event::Entered(HubChoice::Campaign)]
        );
    }
}
