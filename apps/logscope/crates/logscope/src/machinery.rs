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
#[derive(Clone, PartialEq, Debug)]
pub struct Machine {
    /// What the landmark IS.
    pub landmark: &'static str,
    /// The part that moves when experts are fetched.
    pub fetch: &'static str,
    /// The part that moves when the router chooses.
    pub decide: &'static str,
    /// Proportions, so the theme is visible in the GEOMETRY and not only in the prose.
    ///
    /// `add_landmark` took Parts and Placement and never saw a Machine, so fifteen themes existed as
    /// descriptions a renderer could not use — every scene drew the same box. These are the three
    /// numbers that make a clocktower read as a clocktower and an abacus-mill as an abacus-mill.
    ///
    /// **Tall-and-narrow versus wide-and-low is most of the silhouette**, which is all you get at the
    /// distance a landmark stands.
    pub shape: Shape,
    /// The machine at full song — everything nominal, and the state a viewer should most want to
    /// watch. Distinct from `fetch` on purpose: "working" and "working beautifully" are different
    /// pictures, and a landmark that cannot show the second has nothing to reward attention with.
    pub thriving: &'static str,
    /// What it looks like when the CPU is waiting — the state that matters most and is hardest to
    /// show as a number.
    pub stall: &'static str,
}

/// The silhouette of a themed machine.
///
/// Deliberately three numbers rather than a mesh: a scene builds ONE mechanism and proportions it, so
/// adding a theme is a table entry rather than new geometry. Fifteen bespoke models would each need
/// their own stall state, and getting that wrong in one of them is worse than a plainer landmark
/// everywhere.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shape {
    /// Height of the upright, in plinth-widths. A clocktower is tall; a workbench is not.
    pub rise: f32,
    /// Width of the store relative to the plinth. Wide reads as a rack or a runner of beads.
    pub spread: f32,
    /// Size of the drive relative to the upright. A blimp's propellers are large; an escapement is
    /// small and fast.
    pub drive: f32,
}

/// Themed per scene. Unknown scenes get a plain mechanism rather than nothing.
pub fn machine_for(scene: &str) -> Machine {
    match scene {
        "Old Library" | "Archive" | "Reading Room" => Machine {
            shape: Shape { rise: 2.4, spread: 0.8, drive: 0.5 },
            thriving: "every runner in motion at once, the escapement singing, chimes marking each completed turn",
            landmark: "a clocktower above the stacks, its face open to the works",
            fetch: "brass page-runners sliding volumes down the shelf-rails",
            decide: "the escapement ticking once per routed token",
            stall: "the pendulum hangs at the top of its arc and does not fall",
        },
        "Canopy" | "Forest" => Machine {
            shape: Shape { rise: 1.6, spread: 1.3, drive: 0.7 },
            thriving: "the whole trellis in bloom, sap running bright, every leaf tracking its bough in sequence",
            landmark: "a solarpunk trellis-engine grown into the branches",
            fetch: "sap-lines drawing bright fluid up from the roots",
            decide: "leaves turning to face whichever bough is called",
            stall: "the whole canopy holds still, no leaf turning, light unspent",
        },
        "Cloudscape" | "Mistfall" => Machine {
            shape: Shape { rise: 1.2, spread: 1.1, drive: 1.4 },
            thriving: "engines at full ahead, ballast cycling smoothly, the envelope taut and the wake clean",
            landmark: "a moored steampunk blimp, its gondola all exposed gearing",
            fetch: "ballast crates winched up through the cloud on cables",
            decide: "the rudder swinging to a new heading each token",
            stall: "propellers feathered, the envelope drifting without heading",
        },
        "Treehouse" => Machine {
            shape: Shape { rise: 0.8, spread: 1.6, drive: 0.6 },
            thriving: "beads flying across every runner, baskets crossing in both directions, the whole mill drumming",
            landmark: "an ewok-built abacus-mill of rope, bark and counterweight",
            fetch: "woven baskets hauled hand-over-hand along the span",
            decide: "carved beads clacking across their runners to mark the choice",
            stall: "every rope slack, the beads resting mid-runner",
        },
        "Folding@home" => Machine {
            shape: Shape { rise: 1.4, spread: 1.4, drive: 0.4 },
            thriving: "the chain folding cleanly domain by domain, each pocket closing into place with a settling shimmer",
            landmark: "a protein folding in a lattice of glass, one domain per layer",
            fetch: "side-chains swinging into place as their residues are called",
            decide: "the backbone kinking to admit the next fold",
            stall: "the chain frozen half-folded, hydrophobic core still open",
        },
        "Mansion Garden" => Machine {
            shape: Shape { rise: 1.0, spread: 1.5, drive: 1.1 },
            thriving: "every engine on the terrace running together, the Antikythera at speed, the chariot true, the astrolabe wheeling through its houses",
            landmark: "a museum terrace of analog engines — an Antikythera of golden \
gears beside a Chinese south-pointing chariot and an astrolabe on its stand",
            fetch: "the chariot's differential holding its pointer true as trays are wheeled in",
            decide: "the Antikythera's crown gear stepping one tooth per routed token",
            stall: "every dial stopped mid-figure, the crank turning nothing",
        },
        "Atlantis" => Machine {
            shape: Shape { rise: 1.3, spread: 1.2, drive: 1.0 },
            thriving: "the tide running strong, vanes wide, amphorae arriving on every swell in procession",
            landmark: "a tide-engine of bronze and coral in the flooded plaza",
            fetch: "current-vanes drawing sealed amphorae in on the swell",
            decide: "a shoal of fish wheeling as one toward the chosen gate",
            stall: "the water flat, vanes hanging, nothing carried in or out",
        },
        "Fractal Spires" | "Recursive Citadel" => Machine {
            shape: Shape { rise: 2.8, spread: 0.6, drive: 0.8 },
            thriving: "every depth turning at once, each ring in phase with its own smaller copy",
            landmark: "a self-similar orrery, each arm a smaller copy of the whole",
            fetch: "sub-orreries unfolding to surface the arm being read",
            decide: "the outermost ring selecting which copy of itself to enter",
            stall: "the recursion halted at one depth, inner rings unmoving",
        },
        "Slate" | "Alley" => Machine {
            shape: Shape { rise: 1.0, spread: 1.0, drive: 0.7 },
            thriving: "every indicator green, the loader cycling without pause, relays snapping cleanly",
            landmark: "a service panel open on the wall, guts exposed and labelled",
            fetch: "cartridges racking in and out of the loader",
            decide: "a relay board latching the next path closed",
            stall: "every indicator amber, the loader holding its cartridge",
        },
        // A scene with no theme gets the machine plainly rather than nothing: an unnamed mechanism
        // still shows state, and a blank landmark teaches the reader that the feature is broken.
        _ => Machine {
            shape: Shape { rise: 1.0, spread: 1.0, drive: 1.0 },
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

/// The machine's moving parts as NUMBERS, for a renderer.
///
/// `Machine` describes the apparatus in prose, which is what a reader needs and nothing a renderer
/// can use: "beads clacking across their runners" does not say there are sixteen beads and six of
/// them are home. This is that.
///
/// **Structure is theme-independent and naming is not.** Every theme is the same three mechanisms —
/// a store with some slots filled, a selector pointing somewhere, and a drive turning at some rate —
/// so a scene implements one layout and dresses it, rather than each scene inventing its own.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Parts {
    /// Total slots in the store — experts, shelves, beads on the runner.
    pub slots: u32,
    /// How many are currently resident. The gap between this and `slots` is what streams.
    pub filled: u32,
    /// Which slot the selector points at, if the engine has chosen one.
    pub selected: Option<u32>,
    /// Drive speed, 0.0 when stalled. Straight from `Motion::rate`.
    pub drive: f32,
    /// 0..1, how much of the work is landing without a fetch. Drives how bright or settled the
    /// mechanism looks, separately from how fast it moves.
    pub smoothness: f32,
}

impl Parts {
    /// Build from what the engine reports.
    ///
    /// `hit_rate` and the pin counts are already computed for the strip; this is the same state in a
    /// form a scene can draw, so the landmark and the status line cannot disagree.
    pub fn from_state(pinned: u32, total: u32, selected: Option<u32>, motion: Motion, hit_rate: f32)
        -> Parts
    {
        Parts {
            // A dense model has no store, and drawing an empty rack would read as a broken cache
            // rather than as a model that has no experts. One slot, filled, is the honest picture.
            slots: if total == 0 { 1 } else { total },
            filled: if total == 0 { 1 } else { pinned.min(total) },
            // A stalled machine's selector points nowhere: it is not mid-choice, it is waiting.
            selected: if motion == Motion::Stalled { None } else { selected },
            drive: motion.rate(),
            smoothness: hit_rate.clamp(0.0, 1.0),
        }
    }

    /// Slots that are not resident — the ones a scene should show arriving and leaving.
    pub fn streaming(&self) -> u32 { self.slots.saturating_sub(self.filled) }

    /// Is there anything to animate? A machine with every slot filled and nothing streaming should
    /// look settled rather than busy, however fast the drive turns.
    pub fn has_traffic(&self) -> bool { self.streaming() > 0 && self.drive > 0.0 }
}

#[cfg(test)]
mod parts_tests {
    use super::*;

    #[test]
    fn a_real_scout_state_becomes_six_of_sixteen_and_a_STOPPED_drive() {
        let p = Parts::from_state(6, 16, Some(3), Motion::Stalled, 0.46);
        assert_eq!((p.slots, p.filled, p.streaming()), (16, 6, 10));
        assert_eq!(p.drive, 0.0);
        assert!(!p.has_traffic(), "a stopped drive has no traffic however many slots stream");
    }

    #[test]
    fn a_STALLED_machine_points_its_selector_NOWHERE() {
        // It is not mid-choice, it is waiting — and a selector frozen on a slot reads as deliberation
        // rather than as a stall, which is the one impression this must not give.
        assert_eq!(Parts::from_state(6, 16, Some(3), Motion::Stalled, 0.46).selected, None);
        assert_eq!(Parts::from_state(6, 16, Some(3), Motion::Fetching, 0.46).selected, Some(3));
    }

    #[test]
    fn a_DENSE_model_shows_one_filled_slot_not_an_empty_rack() {
        // An empty store would read as a broken cache rather than as a model with no experts.
        let p = Parts::from_state(0, 0, None, Motion::Thriving, 1.0);
        assert_eq!((p.slots, p.filled, p.streaming()), (1, 1, 0));
        assert!(!p.has_traffic());
    }

    #[test]
    fn a_FULLY_PINNED_machine_looks_settled_rather_than_busy() {
        // Nothing streams, so however fast the drive turns there is no traffic to draw — and showing
        // carriers moving in from an empty queue would be inventing work.
        let p = Parts::from_state(16, 16, Some(1), Motion::Thriving, 0.99);
        assert_eq!(p.streaming(), 0);
        assert!(!p.has_traffic());
        assert!(p.drive > 1.0, "still turning fast, just not fetching");
    }

    #[test]
    fn SMOOTHNESS_is_separate_from_speed() {
        // Two different facts: how fast it moves and how much of the work lands without a fetch. A
        // fast machine missing constantly should look frantic, not healthy, and one number cannot
        // carry both.
        let churny = Parts::from_state(2, 16, Some(0), Motion::Fetching, 0.15);
        let clean = Parts::from_state(2, 16, Some(0), Motion::Fetching, 0.95);
        assert_eq!(churny.drive, clean.drive);
        assert!(churny.smoothness < clean.smoothness);
    }

    #[test]
    fn pinned_beyond_the_total_is_CLAMPED_rather_than_overflowing() {
        // A bad count from the engine should not make a scene draw more beads than it has runners.
        let p = Parts::from_state(99, 16, None, Motion::Fetching, 0.5);
        assert_eq!(p.filled, 16);
        assert_eq!(p.streaming(), 0);
    }
}

/// Where the landmark stands, and how big.
///
/// Scenes are generated, so a landmark cannot be authored into each one by hand — and dropping it at
/// the origin puts it through the chat slab, which is the one object the person is actually reading.
///
/// ## Consistent bearing, varying distance
///
/// The landmark sits at **the same bearing in every scene** so a viewer learns where to look once
/// rather than fifteen times. What varies is distance and height, from the scene's own extent: a
/// cathedral-scale library wants its clocktower further out and taller than a treehouse wants its
/// abacus-mill, or the same apparatus reads as a toy in one and a monolith in the other.
///
/// ## Behind and to the left
///
/// The slab occupies the centre and the composer the bottom. Behind-left is the largest area that is
/// neither, and it is where peripheral vision already goes — the point is that a stopped mechanism is
/// noticed without being looked at.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    /// World position, right-handed, +y up, camera looking down -z.
    pub pos: [f32; 3],
    /// Uniform scale, so the landmark subtends roughly the same angle in every scene.
    pub scale: f32,
    /// Radians to rotate about +y so the working face turns toward the viewer. A mechanism seen
    /// edge-on shows nothing, which would waste the whole idea.
    pub yaw: f32,
}

/// Fraction of the scene's radius at which the landmark stands.
///
/// Far enough to clear the slab, near enough to read the parts. Closer than this and a tall landmark
/// crops at the top of the frame in the smaller scenes.
const BEARING_DISTANCE: f32 = 0.62;

/// Bearing in radians: back-left of the viewer.
const BEARING_YAW: f32 = 2.36;   // ~135 degrees

pub fn placement(scene_radius: f32, scene_height: f32) -> Placement {
    // A degenerate extent must not produce a landmark at the origin, which is exactly where the slab
    // is — a scene that failed to report its size should push the landmark out, not in.
    let r = if scene_radius > 1.0 { scene_radius } else { 12.0 };
    let d = r * BEARING_DISTANCE;
    Placement {
        pos: [d * BEARING_YAW.cos(), scene_height * 0.15, d * BEARING_YAW.sin()],
        // Scaled from the scene rather than fixed: the same apparatus reads as a toy in a cathedral
        // and a monolith in a treehouse if it does not grow with its surroundings.
        scale: (r / 12.0).clamp(0.6, 2.4),
        // Face the origin, where the viewer is. A mechanism seen edge-on shows nothing.
        yaw: BEARING_YAW + std::f32::consts::PI,
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    #[test]
    fn the_landmark_is_never_at_the_ORIGIN_where_the_slab_is() {
        for (r, h) in [(12.0, 8.0), (40.0, 30.0), (4.0, 3.0)] {
            let p = placement(r, h);
            let d = (p.pos[0] * p.pos[0] + p.pos[2] * p.pos[2]).sqrt();
            assert!(d > 2.0, "r={r}: landmark {d} from origin, through the reading surface");
        }
    }

    #[test]
    fn a_DEGENERATE_extent_still_pushes_it_out() {
        // A scene that failed to report its size must not collapse the landmark onto the slab —
        // falling back to a default radius is safe, falling back to zero is the worst case.
        let p = placement(0.0, 0.0);
        let d = (p.pos[0] * p.pos[0] + p.pos[2] * p.pos[2]).sqrt();
        assert!(d > 2.0, "got {d}");
    }

    #[test]
    fn the_BEARING_is_the_same_in_every_scene() {
        // A viewer learns where to look once, not fifteen times. Distance varies; direction does not.
        let a = placement(12.0, 8.0);
        let b = placement(40.0, 30.0);
        let ang = |p: &Placement| p.pos[2].atan2(p.pos[0]);
        assert!((ang(&a) - ang(&b)).abs() < 1e-4, "bearing must not drift with scene size");
    }

    #[test]
    fn it_stands_BEHIND_AND_LEFT_rather_than_in_front() {
        // The slab is centre and the composer is bottom; behind-left is the largest area that is
        // neither, and where peripheral vision already goes.
        let p = placement(20.0, 12.0);
        assert!(p.pos[0] < 0.0, "left");
        assert!(p.pos[2] > 0.0, "behind the viewer's forward axis");
    }

    #[test]
    fn SCALE_grows_with_the_scene_but_is_bounded() {
        // The same apparatus reads as a toy in a cathedral and a monolith in a treehouse if it does
        // not grow — and as absurd in either if it grows without limit.
        assert!(placement(40.0, 30.0).scale > placement(8.0, 6.0).scale);
        assert!(placement(400.0, 300.0).scale <= 2.4, "bounded above");
        assert!(placement(1.5, 1.0).scale >= 0.6, "and below");
    }

    #[test]
    fn the_working_face_turns_TOWARD_the_viewer() {
        // A mechanism seen edge-on shows nothing, which would waste the whole idea.
        let p = placement(20.0, 12.0);
        let facing = [p.yaw.cos(), p.yaw.sin()];
        // Pointing back at the origin means the dot with the outward radial is negative.
        let radial = [p.pos[0], p.pos[2]];
        let dot = facing[0] * radial[0] + facing[1] * radial[1];
        assert!(dot < 0.0, "the face must look inward, got {dot}");
    }
}

/// A peer machine's standing in the network.
///
/// **Three states look identical if drawn carelessly, and mean opposite things.** A machine that is
/// stalled is working and waiting; one that is idle is reachable with nothing to do; one that is
/// unreachable is telling you nothing at all. Drawing all three as "not moving" would make the
/// network view actively misleading — you would go and check a machine that is fine, and ignore one
/// that has fallen off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Standing {
    /// This device. Always drawn, always trusted.
    Local,
    /// Paired, reporting, and doing work.
    Working,
    /// Paired, reporting, nothing to do. **Not a fault** — a spare machine looks like this.
    Idle,
    /// Paired and not heard from within the freshness window.
    Silent { since_s: u32 },
    /// Known, and its last report was an error.
    Faulted,
}

impl Standing {
    /// How the mechanism should read.
    ///
    /// Idle and Silent must not look the same, which is the entire reason this enum exists: a
    /// mechanism at rest is a machine with nothing to do, and a mechanism that has gone DARK is one
    /// nobody has heard from. Rest is a posture; dark is an absence.
    pub fn appearance(self) -> &'static str {
        match self {
            Standing::Local => "lit and detailed, the one you are standing next to",
            Standing::Working => "lit, its parts moving in time with its own load",
            Standing::Idle => "lit and at rest — nothing to do, which is not the same as nothing working",
            Standing::Silent { .. } => "unlit and translucent, present on the map but not reporting",
            Standing::Faulted => "lit and askew, a part visibly out of place",
        }
    }

    /// Should the viewer be drawn to it?
    ///
    /// Idle is deliberately NOT attention-worthy. A network view that highlights every resting
    /// machine is one where the highlight means nothing.
    pub fn wants_attention(self) -> bool {
        matches!(self, Standing::Silent { .. } | Standing::Faulted)
    }
}

/// One machine in the network, positioned relative to the local one.
#[derive(Clone, PartialEq, Debug)]
pub struct Peer {
    /// The pairing code's first group — what a person reads aloud to identify a machine.
    pub short_id: String,
    pub standing: Standing,
    pub parts: Parts,
}

/// Arrange peers around the local machine.
///
/// A ring, at the same bearing rule as the solo landmark so the local machine stays where the eye
/// already goes and peers fan out from it. Returns the yaw for each, in the order given.
///
/// **Stable ordering matters more than optimal spacing**: a machine that moves position between
/// frames because another joined is a machine you cannot learn the location of, and the whole value
/// of a spatial view is that position carries identity.
pub fn ring_yaws(peers: usize) -> Vec<f32> {
    if peers == 0 { return Vec::new(); }
    // A lone peer belongs ON the bearing, not at the fan's left edge. My first version subtracted
    // half the span unconditionally, so one machine sat 80 degrees off from where a viewer has
    // learned to look — and the single-peer case is the common one on a two-machine setup.
    if peers == 1 { return vec![BEARING_YAW]; }
    let span = std::f32::consts::PI * 0.9;      // a fan behind-left, not a full circle
    let step = span / (peers - 1) as f32;
    (0..peers).map(|i| BEARING_YAW - span * 0.5 + step * i as f32).collect()
}

#[cfg(test)]
mod peer_tests {
    use super::*;

    #[test]
    fn IDLE_and_SILENT_do_not_look_the_same() {
        // The entire reason Standing exists. A mechanism at rest is a machine with nothing to do; one
        // gone dark is one nobody has heard from. Rest is a posture, dark is an absence.
        assert_ne!(Standing::Idle.appearance(), Standing::Silent { since_s: 60 }.appearance());
        assert!(Standing::Idle.appearance().contains("at rest"));
        assert!(Standing::Silent { since_s: 60 }.appearance().contains("not reporting"));
    }

    #[test]
    fn IDLE_does_not_ask_for_attention_and_SILENT_does() {
        // A network view that highlights every resting machine is one where the highlight means
        // nothing — and a spare box sitting ready is the most common state in a small cluster.
        assert!(!Standing::Idle.wants_attention());
        assert!(!Standing::Working.wants_attention());
        assert!(Standing::Silent { since_s: 30 }.wants_attention());
        assert!(Standing::Faulted.wants_attention());
    }

    #[test]
    fn a_STALLED_local_machine_is_still_Working() {
        // Standing is about the NETWORK; Motion is about the work. A local machine 99% blocked on
        // readbacks is present and busy, and conflating that with an unreachable peer would send
        // someone to check the network when the problem is a kernel.
        let p = Peer {
            short_id: "K7M2P".into(),
            standing: Standing::Local,
            parts: Parts::from_state(6, 16, None, Motion::Stalled, 0.46),
        };
        assert!(!p.standing.wants_attention(), "a stall is a Motion concern, not a Standing one");
        assert_eq!(p.parts.drive, 0.0);
    }

    #[test]
    fn the_ring_ORDER_is_stable_for_a_given_count() {
        // Position carries identity in a spatial view, so the same input must give the same layout.
        assert_eq!(ring_yaws(4), ring_yaws(4));
    }

    #[test]
    fn peers_fan_out_rather_than_encircling() {
        // A full circle puts machines behind the viewer where they cannot be seen without turning,
        // which defeats the point of glanceability.
        let y = ring_yaws(5);
        let spread = y.last().unwrap() - y.first().unwrap();
        assert!(spread < std::f32::consts::PI, "a fan, not a ring: {spread}");
        assert!(spread > 1.0, "and wide enough to tell them apart");
    }

    #[test]
    fn a_SINGLE_peer_sits_on_the_bearing_itself() {
        // With nothing to fan out from, the one peer belongs exactly where the eye already goes.
        let y = ring_yaws(1);
        assert_eq!(y.len(), 1);
        assert!((y[0] - BEARING_YAW).abs() < 1e-4);
    }

    #[test]
    fn no_peers_yields_no_positions_rather_than_one_at_the_origin() {
        assert!(ring_yaws(0).is_empty());
    }
}

/// What a peer's `Standing` means for its mechanism.
///
/// The genuinely new idea from a parallel implementation I built without noticing this module already
/// had one: **a stale peer must not animate its last reported motion.**
///
/// `Standing::Silent { since_s }` already carried the age, so the `RemoteMachine` type I added
/// alongside it was entirely redundant and is gone. `ring_yaws` already placed peers, so the second
/// layout is gone too. What is left is the rule, on the type that already existed.
impl Standing {
    /// The motion to draw, given what the peer last reported.
    ///
    /// **Silent does not fall through to the reported value.** A peer that was thriving when last
    /// heard from is not thriving now — it is unknown — and animating the old value claims a reading
    /// that did not happen, which is the failure mode every lying label in this codebase shares.
    ///
    /// Nor does it render as `Stalled`: "this machine is waiting" and "we do not know" are different
    /// facts, and if they look the same a network partition reads as every peer simultaneously
    /// seizing.
    pub fn motion(self, reported: Motion) -> Motion {
        match self {
            Standing::Local | Standing::Working => reported,
            Standing::Idle => Motion::Deciding,
            Standing::Silent { .. } | Standing::Faulted => Motion::Deciding,
        }
    }

    /// How strongly to draw it, 0..1, so the edge of the network is legible.
    ///
    /// A silent peer fades and a faulted one fades further, but neither reaches zero: **a landmark
    /// that vanishes reads as "I never had that peer"**, and knowing a machine dropped out is the
    /// more useful fact.
    pub fn presence(self) -> f32 {
        match self {
            Standing::Local | Standing::Working => 1.0,
            Standing::Idle => 0.8,
            Standing::Silent { .. } => 0.5,
            Standing::Faulted => 0.25,
        }
    }
}

#[cfg(test)]
mod standing_motion_tests {
    use super::*;

    #[test]
    fn a_SILENT_peer_does_not_animate_its_last_known_motion() {
        // The lying-label class, in a scene. A peer thriving when last heard from is not thriving
        // now — it is unknown, and animating the old value claims a reading that did not happen.
        assert_ne!(Standing::Silent { since_s: 45 }.motion(Motion::Thriving), Motion::Thriving);
    }

    #[test]
    fn a_silent_peer_looks_DIFFERENT_from_a_stalled_one() {
        // "This machine is waiting" and "we do not know" are different facts. If they render the
        // same, a network partition reads as every peer simultaneously seizing.
        assert_ne!(Standing::Silent { since_s: 45 }.motion(Motion::Stalled), Motion::Stalled);
    }

    #[test]
    fn a_FAULTED_peer_is_dimmed_rather_than_erased() {
        // A landmark that vanishes reads as "I never had that peer", and knowing a machine dropped
        // out is the more useful fact.
        assert!(Standing::Faulted.presence() > 0.0);
        assert!(Standing::Faulted.presence() < Standing::Silent { since_s: 40 }.presence());
    }

    #[test]
    fn the_LOCAL_machine_always_shows_what_it_reports() {
        // Its state is read directly rather than announced, so freshness does not apply.
        assert_eq!(Standing::Local.motion(Motion::Thriving), Motion::Thriving);
        assert_eq!(Standing::Local.presence(), 1.0);
    }

    #[test]
    fn IDLE_is_not_dimmed_like_a_fault() {
        // A spare machine is healthy. Fading it toward the faulted end would make every idle peer
        // look like a problem, which is the same mistake as highlighting every resting machine.
        assert!(Standing::Idle.presence() > Standing::Silent { since_s: 40 }.presence());
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn ONE_layout_remains_and_it_scales() {
        // I added a second (a fan beside the local machine) without noticing ring_yaws existed. Both
        // could not be right: a scene using one while a legend used the other would put a peer's
        // label on another peer's mechanism.
        //
        // The ring wins because it scales — eight machines fanned across a quarter turn overlap, and
        // eight on a ring stay separable.
        let ring = ring_yaws(8);
        let mut sorted = ring.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min_gap = sorted.windows(2).map(|w| w[1] - w[0]).fold(f32::MAX, f32::min);
        assert!(min_gap > 0.3, "eight machines stay separable, gap {min_gap}");
    }
}

/// A machine building, weathering, or coming apart.
///
/// Presence as a number fades a landmark evenly and says only *how much*. Weathering says **how
/// long** — vines and dust accumulate, and a mechanism half-swallowed by growth has visibly been
/// silent a while rather than merely being drawn faint.
///
/// ## The flapping problem
///
/// A peer on a marginal link disconnects and reconnects repeatedly. If each transition restarts, the
/// landmark strobes between assembling and collapsing and becomes the most distracting thing in the
/// scene — while telling you less than a steady dim would have.
///
/// **So a reversal RESUMES rather than restarts.** A machine caught halfway through crumbling and
/// then reconnected builds from where it stopped, and the visible result of a flapping peer is a
/// mechanism that hovers around half-built. That reads correctly: it is not working and it is not
/// gone, which is exactly the truth about a marginal link.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Weathering {
    /// 0.0 = fully crumbled, 1.0 = whole and new.
    pub built: f32,
    /// 0.0 = clean, 1.0 = fully overgrown. Independent of `built`: a whole machine can be dusty and
    /// a half-collapsed one can be freshly broken.
    pub age: f32,
}

/// Seconds for a peer to build up or come apart. Slow enough to be watchable, fast enough that the
/// scene settles before someone loses interest.
pub const WEATHER_TRANSITION_S: f32 = 3.0;

/// Seconds of silence before a machine is fully overgrown. Longer than the transition, because decay
/// is a story about duration and a mechanism that ages in three seconds tells no story.
pub const WEATHER_FULL_AGE_S: f32 = 90.0;

impl Weathering {
    /// A machine that has always been here.
    pub fn present() -> Self { Weathering { built: 1.0, age: 0.0 } }
    /// A machine not yet seen.
    pub fn absent() -> Self { Weathering { built: 0.0, age: 0.0 } }

    /// Advance by a frame.
    ///
    /// `connected` drives `built` toward 1 or 0; `silent_s` drives `age`. They are separate because a
    /// connected machine can still be idle and dusty, and a crumbling one is freshly broken rather
    /// than old.
    pub fn tick(&mut self, dt_s: f32, connected: bool, silent_s: f32) {
        let step = dt_s / WEATHER_TRANSITION_S.max(0.001);
        // Resume, not restart: no branch resets `built` to an endpoint, so a reversal simply changes
        // direction from wherever the transition had reached.
        self.built = if connected { (self.built + step).min(1.0) } else { (self.built - step).max(0.0) };
        // Age tracks silence directly rather than accumulating, so a peer that comes back is CLEAN
        // again — the vines were about how long it had been quiet, and it has not been quiet since.
        self.age = (silent_s / WEATHER_FULL_AGE_S.max(0.001)).clamp(0.0, 1.0);
    }

    /// Is the machine in transition? A scene can skip its expensive parts while a landmark is mostly
    /// rubble.
    pub fn transitioning(&self) -> bool { self.built > 0.001 && self.built < 0.999 }

    /// Fraction of the mechanism's parts to draw. A crumbling machine loses pieces rather than
    /// fading — **a half-transparent clocktower reads as a rendering bug; a half-collapsed one reads
    /// as a ruin.**
    pub fn parts_shown(&self, total: u32) -> u32 {
        ((total as f32 * self.built).round() as u32).min(total)
    }
}

#[cfg(test)]
mod weathering_tests {
    use super::*;

    #[test]
    fn a_connecting_machine_BUILDS_over_the_transition() {
        let mut w = Weathering::absent();
        w.tick(WEATHER_TRANSITION_S, true, 0.0);
        assert!(w.built >= 0.999, "got {}", w.built);
    }

    #[test]
    fn a_FLAPPING_peer_hovers_rather_than_strobing() {
        // The property that makes this usable. A reversal resumes from where the transition reached,
        // so a marginal link shows a machine hovering around half-built — which is the truth: not
        // working, not gone.
        let mut w = Weathering::present();
        for i in 0..12 {
            w.tick(0.5, i % 2 == 0, 0.0);
            assert!(w.built > 0.2 && w.built < 1.01, "strobed to {} at step {i}", w.built);
        }
    }

    #[test]
    fn a_reversal_RESUMES_from_where_it_stopped() {
        // Not from an endpoint. Restarting would make every brief drop cost a full rebuild.
        let mut w = Weathering::present();
        w.tick(1.0, false, 0.0);
        let mid = w.built;
        assert!(mid > 0.0 && mid < 1.0);
        w.tick(0.1, true, 0.0);
        assert!(w.built > mid && w.built < 1.0, "resumed from {mid} to {}", w.built);
    }

    #[test]
    fn AGE_tracks_silence_and_CLEARS_when_the_peer_returns() {
        // The vines were about how long it had been quiet, and it has not been quiet since. Letting
        // age accumulate would leave a healthy machine permanently overgrown from one old outage.
        let mut w = Weathering::present();
        w.tick(1.0, true, WEATHER_FULL_AGE_S);
        assert!(w.age >= 0.999);
        w.tick(1.0, true, 0.0);
        assert_eq!(w.age, 0.0, "a returned peer is clean again");
    }

    #[test]
    fn age_and_built_are_INDEPENDENT() {
        // A whole machine can be dusty and a half-collapsed one freshly broken. One number could not
        // say both, and conflating them would make every long-idle peer look like it was falling
        // apart.
        let mut w = Weathering::present();
        w.tick(0.1, true, WEATHER_FULL_AGE_S * 0.5);
        assert!(w.built >= 0.999, "whole");
        assert!(w.age > 0.4, "and dusty");
    }

    #[test]
    fn a_crumbling_machine_LOSES_PARTS_rather_than_fading() {
        // A half-transparent clocktower reads as a rendering bug; a half-collapsed one reads as a
        // ruin.
        let mut w = Weathering::present();
        w.tick(WEATHER_TRANSITION_S * 0.5, false, 0.0);
        let shown = w.parts_shown(20);
        assert!(shown > 0 && shown < 20, "got {shown} of 20");
    }

    #[test]
    fn a_fully_gone_machine_shows_NO_parts_and_is_not_transitioning() {
        let mut w = Weathering::present();
        w.tick(WEATHER_TRANSITION_S * 2.0, false, 0.0);
        assert_eq!(w.parts_shown(20), 0);
        assert!(!w.transitioning(), "settled, not mid-collapse");
    }
}

/// When a peer that dropped is allowed back into the work pool.
///
/// The weathering above smooths the PICTURE of a flapping peer. It does not stop the flapping from
/// costing anything, and smoothing a symptom is the wrong fix: a peer that keeps dropping is useless
/// for a long generation whatever its landmark looks like.
///
/// So it waits. And the wait is not arbitrary — **the rebuild animation and the join-back delay are
/// the same duration on purpose**. The tower finishing construction *is* the readiness signal, which
/// makes the animation load-bearing rather than decorative: what you see is what the scheduler
/// believes.
///
/// ## The delay depends on what a defection would cost
///
/// Mid-generation, a peer vanishing means its share of the work is lost and must be redone, so the
/// bar to readmit is high. Idle, readmitting early costs nothing and the rebuild time is free — it
/// overlaps with preloading, which has to happen anyway before that peer is useful.
///
/// A fixed delay would be wrong in both directions: too eager under load, too cautious at rest.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rejoin {
    /// Consecutive drops. A peer that has flapped repeatedly waits longer each time.
    pub drops: u32,
    /// Seconds since it came back.
    pub steady_s: f32,
}

/// Base wait when the local machine is idle: exactly the rebuild, so the delay is invisible.
pub const REJOIN_IDLE_S: f32 = WEATHER_TRANSITION_S;

/// Base wait mid-generation. Longer, because a defection here costs work already done.
pub const REJOIN_BUSY_S: f32 = 20.0;

/// Beyond this many drops the extra caution stops growing — a peer flapping this hard is a link
/// problem, and waiting ten minutes does not fix a link.
pub const REJOIN_MAX_DROPS: u32 = 4;

impl Rejoin {
    /// How long this peer must stay up before it gets work again.
    pub fn required_s(&self, busy: bool) -> f32 {
        let base = if busy { REJOIN_BUSY_S } else { REJOIN_IDLE_S };
        // Each drop doubles, to a ceiling. Linear would readmit a chronic flapper too often; growing
        // without bound would exile a peer that had one bad minute.
        base * (1u32 << self.drops.min(REJOIN_MAX_DROPS)) as f32
    }

    pub fn ready(&self, busy: bool) -> bool { self.steady_s >= self.required_s(busy) }

    /// **Is the wait free?** When idle, the rebuild has to happen anyway and preloading fills the
    /// same window, so nothing is lost by waiting — and a scene can say so rather than looking like
    /// it is withholding a working machine.
    pub fn wait_is_free(&self, busy: bool) -> bool {
        !busy && self.required_s(busy) <= WEATHER_TRANSITION_S * 2.0
    }
}

#[cfg(test)]
mod rejoin_tests {
    use super::*;

    #[test]
    fn an_IDLE_rejoin_costs_exactly_the_rebuild() {
        // The coupling that makes the animation load-bearing: the tower finishing construction IS
        // the readiness signal, so the delay is invisible and free.
        let r = Rejoin { drops: 0, steady_s: 0.0 };
        assert_eq!(r.required_s(false), WEATHER_TRANSITION_S);
        assert!(r.wait_is_free(false));
    }

    #[test]
    fn a_BUSY_rejoin_waits_much_longer() {
        // Mid-generation a defection costs work already done, so the bar to readmit is high.
        let r = Rejoin { drops: 0, steady_s: 0.0 };
        assert!(r.required_s(true) > r.required_s(false) * 5.0);
        assert!(!r.wait_is_free(true), "and the scene should not pretend it is free");
    }

    #[test]
    fn repeated_drops_DOUBLE_the_wait_up_to_a_ceiling() {
        // Linear would readmit a chronic flapper too often; unbounded would exile a peer that had one
        // bad minute. A peer flapping this hard is a link problem, and waiting ten minutes does not
        // fix a link.
        let w = |d| Rejoin { drops: d, steady_s: 0.0 }.required_s(false);
        assert!(w(1) > w(0) && w(2) > w(1));
        assert_eq!(w(REJOIN_MAX_DROPS), w(REJOIN_MAX_DROPS + 9), "capped");
    }

    #[test]
    fn a_peer_that_has_been_STEADY_is_readmitted() {
        let r = Rejoin { drops: 1, steady_s: 100.0 };
        assert!(r.ready(false));
        assert!(r.ready(true));
    }

    #[test]
    fn a_peer_that_just_returned_is_NOT_readmitted_mid_generation() {
        // The case the whole policy exists for: it looks available and has proved nothing.
        assert!(!Rejoin { drops: 0, steady_s: 2.0 }.ready(true));
    }

    #[test]
    fn the_idle_wait_ends_when_the_TOWER_finishes() {
        // Stated as an assertion rather than a comment, because the two constants living apart is
        // exactly how they would drift.
        let mut w = Weathering::absent();
        w.tick(REJOIN_IDLE_S, true, 0.0);
        assert!(w.built >= 0.999, "the rebuild completes exactly as the peer becomes ready");
        assert!(Rejoin { drops: 0, steady_s: REJOIN_IDLE_S }.ready(false));
    }
}
