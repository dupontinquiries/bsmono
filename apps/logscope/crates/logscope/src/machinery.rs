//! machinery.rs — the engine's working parts, made visible in whatever world you are standing in.
//!
//! Every measure-then-choose loop in this engine already explains itself in English: `Verdict`,
//! `ExpertResidency`, `PrefetchAccuracy`, `ShapeDisagreement`, `Reframe`, `PatchError::retry`. All of
//! that goes to stderr and dies there, which is why the last several days have been spent extracting
//! it from logs by hand.
//!
//! This is the content layer for surfacing it. Two parts:
//!
//! 1. **`Strip`** — a rotating line sharing space with the token preview, carrying the single most
//!    actionable thing right now rather than a dashboard of numbers nobody reads.
//! 2. **`Machine`** — the same state as a physical apparatus, themed per scene, so the landmark in
//!    each world is the *same machine* wearing different clothes.
//!
//! ## Why one thing at a time
//!
//! A strip showing six metrics is a strip nobody reads. It shows the worst offender and rotates, so
//! the thing on screen is always the thing worth knowing — and when everything is healthy it says so
//! in one short line rather than going blank, because blank reads as broken.

/// What the engine is currently doing wrong, in priority order.
///
/// Ordering is by what a reader could act on, not by magnitude. A 99.9% sync figure and a 46% cache
/// hit rate are both true; only one of them has a next step.
#[derive(Clone, PartialEq, Debug)]
pub enum Concern {
    /// The CPU is waiting on the GPU for most of the step.
    SyncBound { pct: u32 },
    /// A model was refused or clamped for memory.
    MemoryClamped { asked_gib: u32, held_gib: u32 },
    /// Expert streaming is doing most of the work.
    ExpertChurn { hit_pct: u32, pinned: u32, total: u32 },
    /// A tuner wanted to switch and was held.
    TunerHeld { seconds_left: u32 },
    /// Nothing is wrong.
    Healthy { tok_s: f32 },
}

impl Concern {
    /// One line, short enough to share a strip with a token preview.
    pub fn line(&self) -> String {
        match self {
            Concern::SyncBound { pct } =>
                format!("⏳ {pct}% of each step is the CPU waiting — the GPU is idle, not slow"),
            Concern::MemoryClamped { asked_gib, held_gib } =>
                format!("▣ budget {asked_gib} GiB clamped to {held_gib} — this device accepts a big \
allocation but will not hold it"),
            Concern::ExpertChurn { hit_pct, pinned, total } =>
                format!("◈ experts {hit_pct}% resident ({pinned}/{total} pinned) — the rest stream \
every token"),
            Concern::TunerHeld { seconds_left } =>
                format!("⧗ a faster setting is waiting {seconds_left}s to prove itself"),
            Concern::Healthy { tok_s } => format!("✦ running clean at {tok_s:.1} tok/s"),
        }
    }

    /// Lower sorts first. Actionability, not magnitude.
    fn rank(&self) -> u8 {
        match self {
            Concern::SyncBound { .. } => 0,
            Concern::MemoryClamped { .. } => 1,
            Concern::ExpertChurn { .. } => 2,
            Concern::TunerHeld { .. } => 3,
            Concern::Healthy { .. } => 9,
        }
    }
}

/// The rotating strip.
pub struct Strip { concerns: Vec<Concern>, at: usize }

impl Strip {
    pub fn new(mut concerns: Vec<Concern>, tok_s: f32) -> Self {
        concerns.sort_by_key(|c| c.rank());
        // Never empty. A blank strip reads as a broken strip, and "nothing is wrong" is information.
        if concerns.is_empty() { concerns.push(Concern::Healthy { tok_s }); }
        Strip { concerns, at: 0 }
    }

    /// The line to show now; advances on each call so the strip cycles.
    pub fn next(&mut self) -> String {
        let s = self.concerns[self.at % self.concerns.len()].line();
        self.at = self.at.wrapping_add(1);
        s
    }

    /// The single worst thing, for when there is only room for one.
    pub fn worst(&self) -> &Concern { &self.concerns[0] }
}

/// The apparatus that represents the engine in a given world.
///
/// Same machine everywhere — a thing that fetches, a thing that decides, a thing that waits — dressed
/// for the scene it stands in. The point is that state is legible at a glance: a mechanism visibly
/// stalled reads as stalled without anyone parsing a percentage.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Machine {
    /// What the landmark IS.
    pub landmark: &'static str,
    /// The part that moves when experts are fetched.
    pub fetch: &'static str,
    /// The part that moves when the router chooses.
    pub decide: &'static str,
    /// The machine at full song — everything nominal, and the state a viewer should most want to
    /// watch. Distinct from `fetch` on purpose: "working" and "working beautifully" are different
    /// pictures, and a landmark that cannot show the second has nothing to reward attention with.
    pub thriving: &'static str,
    /// What it looks like when the CPU is waiting — the state that matters most and is hardest to
    /// show as a number.
    pub stall: &'static str,
}

/// Themed per scene. Unknown scenes get a plain mechanism rather than nothing.
pub fn machine_for(scene: &str) -> Machine {
    match scene {
        "Old Library" | "Archive" | "Reading Room" => Machine {
            thriving: "every runner in motion at once, the escapement singing, chimes marking each completed turn",
            landmark: "a clocktower above the stacks, its face open to the works",
            fetch: "brass page-runners sliding volumes down the shelf-rails",
            decide: "the escapement ticking once per routed token",
            stall: "the pendulum hangs at the top of its arc and does not fall",
        },
        "Canopy" | "Forest" => Machine {
            thriving: "the whole trellis in bloom, sap running bright, every leaf tracking its bough in sequence",
            landmark: "a solarpunk trellis-engine grown into the branches",
            fetch: "sap-lines drawing bright fluid up from the roots",
            decide: "leaves turning to face whichever bough is called",
            stall: "the whole canopy holds still, no leaf turning, light unspent",
        },
        "Cloudscape" | "Mistfall" => Machine {
            thriving: "engines at full ahead, ballast cycling smoothly, the envelope taut and the wake clean",
            landmark: "a moored steampunk blimp, its gondola all exposed gearing",
            fetch: "ballast crates winched up through the cloud on cables",
            decide: "the rudder swinging to a new heading each token",
            stall: "propellers feathered, the envelope drifting without heading",
        },
        "Treehouse" => Machine {
            thriving: "beads flying across every runner, baskets crossing in both directions, the whole mill drumming",
            landmark: "an ewok-built abacus-mill of rope, bark and counterweight",
            fetch: "woven baskets hauled hand-over-hand along the span",
            decide: "carved beads clacking across their runners to mark the choice",
            stall: "every rope slack, the beads resting mid-runner",
        },
        "Folding@home" => Machine {
            thriving: "the chain folding cleanly domain by domain, each pocket closing into place with a settling shimmer",
            landmark: "a protein folding in a lattice of glass, one domain per layer",
            fetch: "side-chains swinging into place as their residues are called",
            decide: "the backbone kinking to admit the next fold",
            stall: "the chain frozen half-folded, hydrophobic core still open",
        },
        "Mansion Garden" => Machine {
            thriving: "every engine on the terrace running together, the Antikythera at speed, the chariot true, the astrolabe wheeling through its houses",
            landmark: "a museum terrace of analog engines — an Antikythera of golden \
gears beside a Chinese south-pointing chariot and an astrolabe on its stand",
            fetch: "the chariot's differential holding its pointer true as trays are wheeled in",
            decide: "the Antikythera's crown gear stepping one tooth per routed token",
            stall: "every dial stopped mid-figure, the crank turning nothing",
        },
        "Atlantis" => Machine {
            thriving: "the tide running strong, vanes wide, amphorae arriving on every swell in procession",
            landmark: "a tide-engine of bronze and coral in the flooded plaza",
            fetch: "current-vanes drawing sealed amphorae in on the swell",
            decide: "a shoal of fish wheeling as one toward the chosen gate",
            stall: "the water flat, vanes hanging, nothing carried in or out",
        },
        "Fractal Spires" | "Recursive Citadel" => Machine {
            thriving: "every depth turning at once, each ring in phase with its own smaller copy",
            landmark: "a self-similar orrery, each arm a smaller copy of the whole",
            fetch: "sub-orreries unfolding to surface the arm being read",
            decide: "the outermost ring selecting which copy of itself to enter",
            stall: "the recursion halted at one depth, inner rings unmoving",
        },
        "Slate" | "Alley" => Machine {
            thriving: "every indicator green, the loader cycling without pause, relays snapping cleanly",
            landmark: "a service panel open on the wall, guts exposed and labelled",
            fetch: "cartridges racking in and out of the loader",
            decide: "a relay board latching the next path closed",
            stall: "every indicator amber, the loader holding its cartridge",
        },
        // A scene with no theme gets the machine plainly rather than nothing: an unnamed mechanism
        // still shows state, and a blank landmark teaches the reader that the feature is broken.
        _ => Machine {
            thriving: "every part moving in time, nothing waiting",
            landmark: "an open mechanism on a plinth",
            fetch: "carriers moving parts in from the store",
            decide: "a selector arm settling on the next path",
            stall: "all motion stopped, the drive still turning",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_shows_the_most_ACTIONABLE_concern_first() {
        // Not the largest number. A 99.9% sync figure and a 46% hit rate are both true and only one
        // has a next step.
        let s = Strip::new(vec![
            Concern::ExpertChurn { hit_pct: 46, pinned: 6, total: 16 },
            Concern::SyncBound { pct: 99 },
        ], 1.4);
        assert!(matches!(s.worst(), Concern::SyncBound { .. }));
    }

    #[test]
    fn it_ROTATES_rather_than_showing_everything_at_once() {
        // A strip with six metrics on it is a strip nobody reads.
        let mut s = Strip::new(vec![
            Concern::SyncBound { pct: 99 },
            Concern::ExpertChurn { hit_pct: 46, pinned: 6, total: 16 },
        ], 1.4);
        let (a, b, c) = (s.next(), s.next(), s.next());
        assert_ne!(a, b);
        assert_eq!(a, c, "and cycles back round");
    }

    #[test]
    fn a_HEALTHY_engine_says_so_rather_than_going_blank() {
        // Blank reads as broken, and "nothing is wrong" is information.
        let mut s = Strip::new(vec![], 12.5);
        assert!(s.next().contains("12.5 tok/s"));
    }

    #[test]
    fn every_scene_gets_a_machine_including_unknown_ones() {
        for scene in ["Old Library", "Canopy", "Cloudscape", "Treehouse", "Folding@home",
                      "Mansion Garden", "Atlantis", "Fractal Spires", "Slate", "Somewhere New"] {
            let m = machine_for(scene);
            assert!(!m.landmark.is_empty() && !m.stall.is_empty(), "{scene}");
        }
    }

    #[test]
    fn the_STALL_state_is_described_for_every_theme() {
        // The state that matters most and is hardest to read as a number — 99.9% sync means nothing
        // at a glance, and a pendulum stopped at the top of its arc means everything.
        for scene in ["Old Library", "Canopy", "Cloudscape", "Treehouse", "Mansion Garden"] {
            let m = machine_for(scene);
            assert_ne!(m.stall, m.fetch, "{scene}: stalled must LOOK different from working");
        }
    }

    #[test]
    fn themes_are_actually_DIFFERENT_and_not_one_text_reused() {
        let a = machine_for("Old Library");
        let b = machine_for("Canopy");
        let c = machine_for("Treehouse");
        assert_ne!(a.landmark, b.landmark);
        assert_ne!(b.landmark, c.landmark);
        assert!(machine_for("Mansion Garden").landmark.contains("Antikythera"));
    }
}

/// How the strip shares its space with the token preview.
///
/// The preview shows what the model just wrote; the strip shows what the engine is doing. Both want
/// the same row, and the split is not even: **the preview is what the person came for.**
///
/// So the strip takes a short slot on a long cycle, and — the part that matters — it takes a LONGER
/// one when something is actually wrong. A rotation that gives a stall the same three seconds as
/// "running clean" is a rotation that hides the stall in the noise of its own schedule.
pub struct Alternator {
    strip: Strip,
    elapsed_ms: u64,
}

/// The preview's slot. Long, because it is the primary content.
pub const PREVIEW_MS: u64 = 5_000;

/// The strip's slot when nothing is wrong: brief, easily missed, and that is correct — there is
/// nothing to act on.
pub const STRIP_QUIET_MS: u64 = 1_500;

/// The strip's slot when something is. Long enough to read twice, because the first read of
/// "99% of each step is the CPU waiting" is usually not the one that lands.
pub const STRIP_ALERT_MS: u64 = 4_000;

/// What should occupy the row right now.
#[derive(Clone, PartialEq, Debug)]
pub enum Slot {
    /// Show the token preview.
    Preview,
    /// Show this line from the strip.
    Status(String),
}

impl Alternator {
    pub fn new(strip: Strip) -> Self { Alternator { strip, elapsed_ms: 0 } }

    fn strip_ms(&self) -> u64 {
        if matches!(self.strip.worst(), Concern::Healthy { .. }) { STRIP_QUIET_MS } else { STRIP_ALERT_MS }
    }

    /// Advance by a frame and say what to draw.
    ///
    /// Takes a delta rather than reading a clock so the behaviour is testable without waiting, which
    /// is the only way a five-second cycle gets tested at all.
    pub fn tick(&mut self, dt_ms: u64) -> Slot {
        let cycle = PREVIEW_MS + self.strip_ms();
        self.elapsed_ms = (self.elapsed_ms + dt_ms) % cycle.max(1);
        if self.elapsed_ms < PREVIEW_MS {
            Slot::Preview
        } else {
            Slot::Status(self.strip.next())
        }
    }

    /// Skip straight to the status line — for when the worst concern has just changed and waiting
    /// five seconds to mention it would be absurd.
    pub fn interrupt(&mut self) -> Slot {
        self.elapsed_ms = PREVIEW_MS;
        Slot::Status(self.strip.next())
    }
}

#[cfg(test)]
mod alternator_tests {
    use super::*;

    fn alt(c: Vec<Concern>) -> Alternator { Alternator::new(Strip::new(c, 1.4)) }

    #[test]
    fn the_PREVIEW_holds_the_row_for_most_of_the_cycle() {
        // It is what the person came for; the strip is an aside.
        let mut a = alt(vec![Concern::SyncBound { pct: 99 }]);
        assert_eq!(a.tick(0), Slot::Preview);
        assert_eq!(a.tick(4_900), Slot::Preview);
        assert!(matches!(a.tick(200), Slot::Status(_)));
    }

    #[test]
    fn a_REAL_concern_gets_a_longer_slot_than_a_healthy_one() {
        // A rotation giving a stall the same time as "running clean" hides the stall in the noise of
        // its own schedule.
        let mut bad = alt(vec![Concern::SyncBound { pct: 99 }]);
        let mut good = alt(vec![]);
        bad.tick(PREVIEW_MS); good.tick(PREVIEW_MS);
        // After the quiet slot would have expired, the healthy strip is back on preview and the
        // alerting one is still showing.
        good.tick(STRIP_QUIET_MS);
        assert_eq!(good.tick(1), Slot::Preview);
        bad.tick(STRIP_QUIET_MS);
        assert!(matches!(bad.tick(1), Slot::Status(_)), "an alert holds the row longer");
    }

    #[test]
    fn it_CYCLES_rather_than_running_out() {
        let mut a = alt(vec![Concern::SyncBound { pct: 99 }]);
        let mut saw_preview_again = false;
        for _ in 0..40 {
            if matches!(a.tick(500), Slot::Preview) && a.elapsed_ms < 500 { saw_preview_again = true; }
        }
        assert!(saw_preview_again, "the row must return to the preview");
    }

    #[test]
    fn INTERRUPT_shows_the_status_without_waiting() {
        // When the worst concern has just changed, waiting five seconds to mention it is absurd.
        let mut a = alt(vec![Concern::MemoryClamped { asked_gib: 48, held_gib: 31 }]);
        match a.interrupt() {
            Slot::Status(s) => assert!(s.contains("clamped")),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn a_healthy_engine_still_takes_its_turn() {
        // Briefly, and it must still appear: a row that only ever shows the preview cannot tell you
        // the engine is fine, which is information when you are wondering why it feels slow.
        let mut a = alt(vec![]);
        a.tick(PREVIEW_MS);
        assert!(matches!(a.tick(1), Slot::Status(_)));
    }
}

/// Which part of the machine should be moving right now.
///
/// `Machine` describes three states in prose and nothing said which one applies. A landmark needs
/// that: the clocktower has to know whether its pendulum swings or hangs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion { Thriving, Fetching, Deciding, Stalled }

impl Motion {
    /// The description to render, from the scene's machine.
    pub fn describe(self, m: &Machine) -> &'static str {
        match self {
            Motion::Thriving => m.thriving,
            Motion::Fetching => m.fetch,
            Motion::Deciding => m.decide,
            Motion::Stalled => m.stall,
        }
    }

    /// How fast the mechanism should animate, as a multiplier on its resting rate.
    ///
    /// **Stalled is 0.0, not slow.** A mechanism creeping along reads as "working hard"; one that has
    /// visibly stopped reads as stopped. That difference is the whole reason to draw this rather than
    /// print a percentage, and softening it to a crawl would give back exactly what the visual buys.
    pub fn rate(self) -> f32 {
        match self {
            // Faster than merely working, because a landmark is something you CHOOSE to look at and
            // a healthy engine should be the best-looking thing it does — not the absence of a
            // problem. The strip and the landmark want opposite rules here: the strip stays quiet
            // when all is well because a diagnostic that always fires is ignored, and the landmark
            // does its finest work for exactly the same reason it is worth glancing at.
            Motion::Thriving => 1.4,
            Motion::Fetching => 1.0,
            Motion::Deciding => 0.6,
            Motion::Stalled => 0.0,
        }
    }
}

/// What the machine should be doing, given what the engine is doing.
pub fn motion_for(c: &Concern) -> Motion {
    match c {
        // The stall is the state worth showing, and it outranks everything: a machine that fetched
        // busily while the CPU waited would be telling the opposite of the truth.
        Concern::SyncBound { .. } => Motion::Stalled,
        // A clamped budget is not a stall — work continues, just less of it fits. Showing it as
        // stopped would be alarming about the wrong thing.
        Concern::MemoryClamped { .. } => Motion::Fetching,
        Concern::ExpertChurn { .. } => Motion::Fetching,
        Concern::TunerHeld { .. } => Motion::Deciding,
        // NOT Deciding. The landmark's job when nothing is wrong is to be worth looking at, which
        // is the opposite of the strip's job in the same situation.
        Concern::Healthy { .. } => Motion::Thriving,
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    #[test]
    fn a_sync_bound_engine_shows_a_STOPPED_machine() {
        assert_eq!(motion_for(&Concern::SyncBound { pct: 99 }), Motion::Stalled);
        assert_eq!(Motion::Stalled.rate(), 0.0, "stopped, not slow");
    }

    #[test]
    fn stalled_is_ZERO_rather_than_a_crawl() {
        // A mechanism creeping along reads as "working hard"; one that has visibly stopped reads as
        // stopped. Softening it gives back exactly what drawing this instead of a percentage buys.
        assert!(Motion::Stalled.rate() < Motion::Deciding.rate());
        assert_eq!(Motion::Stalled.rate(), 0.0);
    }

    #[test]
    fn a_CLAMPED_budget_is_not_shown_as_a_stall() {
        // Work continues, just less of it fits. Showing it stopped would alarm about the wrong thing.
        assert_eq!(motion_for(&Concern::MemoryClamped { asked_gib: 48, held_gib: 31 }), Motion::Fetching);
    }

    #[test]
    fn expert_churn_shows_the_FETCH_parts_moving() {
        // Which is literally what is happening: the stream is doing the work.
        assert_eq!(motion_for(&Concern::ExpertChurn { hit_pct: 46, pinned: 6, total: 16 }),
                   Motion::Fetching);
    }

    #[test]
    fn every_motion_has_a_description_in_every_theme() {
        for scene in ["Old Library", "Canopy", "Cloudscape", "Treehouse", "Folding@home",
                      "Mansion Garden", "Atlantis", "Somewhere New"] {
            let m = machine_for(scene);
            for mo in [Motion::Fetching, Motion::Deciding, Motion::Stalled] {
                assert!(!mo.describe(&m).is_empty(), "{scene} {mo:?}");
            }
        }
    }

    #[test]
    fn the_clocktower_hangs_when_the_engine_waits() {
        // The end-to-end claim, in one assertion: a real Scout run makes the pendulum stop.
        let m = machine_for("Old Library");
        let motion = motion_for(&Concern::SyncBound { pct: 99 });
        assert!(motion.describe(&m).contains("does not fall"));
    }
}

#[cfg(test)]
mod audience_tests {
    use super::*;

    /// **The strip and the landmark want OPPOSITE rules when nothing is wrong**, and I built the
    /// landmark inheriting the strip's.
    ///
    /// The strip interrupts: it shares a row with content the reader came for, so staying quiet when
    /// healthy is correct — a diagnostic that always fires is one nobody reads.
    ///
    /// The landmark is chosen: someone looked at it deliberately, so healthy has to be its most
    /// watchable state. A mechanism that only does something interesting when broken teaches you to
    /// hope for failure, and gives you nothing on the days it works.
    #[test]
    fn a_healthy_engine_makes_the_landmark_do_its_BEST_work() {
        let m = motion_for(&Concern::Healthy { tok_s: 24.0 });
        assert_eq!(m, Motion::Thriving);
        assert!(m.rate() > Motion::Fetching.rate(), "thriving must out-move merely working");
    }

    #[test]
    fn thriving_reads_DIFFERENTLY_from_working_in_every_theme() {
        // "Working" and "working beautifully" are different pictures. A landmark that cannot show the
        // second has nothing to reward attention with.
        for scene in ["Old Library", "Canopy", "Cloudscape", "Treehouse", "Folding@home",
                      "Mansion Garden", "Atlantis", "Fractal Spires", "Slate", "Elsewhere"] {
            let m = machine_for(scene);
            assert_ne!(m.thriving, m.fetch, "{scene}");
            assert_ne!(m.thriving, m.stall, "{scene}");
            assert!(!m.thriving.is_empty(), "{scene}");
        }
    }

    #[test]
    fn the_four_states_are_ORDERED_by_how_much_moves() {
        // Stalled reads as stopped, thriving as fullest — and the two middle states sit between, so a
        // viewer can rank what they are seeing without a legend.
        assert!(Motion::Thriving.rate() > Motion::Fetching.rate());
        assert!(Motion::Fetching.rate() > Motion::Deciding.rate());
        assert!(Motion::Deciding.rate() > Motion::Stalled.rate());
        assert_eq!(Motion::Stalled.rate(), 0.0);
    }

    #[test]
    fn the_STRIP_still_stays_quiet_on_the_same_state() {
        // The other half of the distinction: the same Healthy concern that makes the clocktower sing
        // gives the strip one short line and a brief slot.
        let mut s = Strip::new(vec![], 24.0);
        assert!(s.next().contains("running clean"));
        let mut a = Alternator::new(Strip::new(vec![], 24.0));
        a.tick(PREVIEW_MS);
        a.tick(STRIP_QUIET_MS);
        assert_eq!(a.tick(1), Slot::Preview, "the healthy strip yields the row quickly");
    }
}
