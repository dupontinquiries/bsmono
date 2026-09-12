//! Crash on purpose before the driver crashes for you.
//!
//! Observed on Asahi (Apple silicon): single frames taking 11.2 seconds
//! of GPU time, `main_us` 9.7s of it. The firmware watchdog kills the
//! GPU, and on this driver that takes the WHOLE DEVICE down --
//! `kwin_wayland`, `plasmashell` and every other client start failing
//! with ENODEV, the screen freezes with cursor-shaped smears where
//! damage rects never get redrawn, and only a reboot recovers it.
//!
//! The engine already measured this. `gpu_spike` fires above 100ms and
//! we were logging eleven-second frames and submitting another one.
//! Measuring a fatal condition and then proceeding into it is the
//! actual defect; the slow frame is only the cause.
//!
//! WHAT THIS DOES
//!
//! Watches frame GPU time and escalates:
//!
//!   under the soft limit    nothing
//!   over it once            shed load: the cheapest thing first
//!   over it again           shed more
//!   out of things to shed   ABORT, with a message naming the pass
//!
//! Aborting is the point. A process that exits with "the GPU took 9.7s
//! on the main pass, killing the frame before the driver kills the
//! device" costs the player one session. Letting the submission through
//! costs them every application they had open and a reboot.
//!
//! THE THRESHOLDS
//!
//! Driver watchdogs are typically 2 seconds and are not knowable from
//! inside the process, so the soft limit is well below any of them. The
//! equivalent bug in llama.cpp on AMD APUs was the same shape --
//! batching a hundred nodes into one submit against a 2000ms
//! `lockup_timeout` -- and the fix there was to submit less per batch,
//! with no measurable slowdown. Less work per submission is the cure;
//! this is the seatbelt for when it is not enough.

/// What can be given up, cheapest first.
///
/// Order matters and is a judgement: a player would rather lose
/// shadows than draw distance, and would rather lose draw distance
/// than have their machine locked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shed {
    /// Nothing given up yet.
    Nothing,
    /// Shadow pass off. Usually a fifth of the time and the least
    /// missed.
    Shadows,
    /// Volumetric/froxel lighting off.
    Volumetrics,
    /// Render distance cut hard.
    Distance,
    /// Everything sheddable is gone and it is STILL over.
    Exhausted,
}

impl Shed {
    pub fn next(self) -> Shed {
        match self {
            Shed::Nothing => Shed::Shadows,
            Shed::Shadows => Shed::Volumetrics,
            Shed::Volumetrics => Shed::Distance,
            Shed::Distance | Shed::Exhausted => Shed::Exhausted,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Shed::Nothing => "nothing", Shed::Shadows => "shadows",
            Shed::Volumetrics => "volumetric lighting", Shed::Distance => "render distance",
            Shed::Exhausted => "everything sheddable",
        }
    }
}

/// What the guard decided this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Carry on.
    Fine,
    /// Give this up before the next frame.
    ShedNow { level: Shed, why: String },
    /// Stop. Submitting again risks taking the device down.
    Abort { why: String },
}

/// Per-pass GPU microseconds for one frame, as `gpu_timer` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameCost {
    pub total_us: u64,
    pub main_us: u64,
    pub shadow_us: u64,
    pub froxel_us: u64,
    pub cull_us: u64,
}

impl FrameCost {
    /// Which pass is to blame, and what share it took.
    pub fn worst(&self) -> (&'static str, u64) {
        let mut w = ("main", self.main_us);
        for (n, v) in [("shadow", self.shadow_us), ("froxel", self.froxel_us),
                       ("cull", self.cull_us)] {
            if v > w.1 { w = (n, v); }
        }
        w
    }
}

#[derive(Debug, Clone)]
pub struct Guard {
    /// Over this, start shedding. Well under any driver watchdog.
    pub soft_us: u64,
    /// Over this, do not submit another frame at this cost.
    pub hard_us: u64,
    /// How many frames over the soft limit before shedding, so one
    /// slow frame during a load spike does not strip the renderer.
    pub patience: u32,
    over: u32,
    shed: Shed,
    /// Frames under the soft limit since the last shed, for restoring.
    calm: u32,
}

impl Default for Guard {
    fn default() -> Self {
        // 700ms soft: a third of a typical 2s watchdog, which leaves
        // room for the NEXT frame to be worse than this one before
        // anything is done about it.
        //
        // 1.5s hard: still under the watchdog, but close enough that
        // one more frame of growth would reach it.
        Guard { soft_us: 700_000, hard_us: 1_500_000, patience: 2,
                over: 0, shed: Shed::Nothing, calm: 0 }
    }
}

impl Guard {
    pub fn shed_level(&self) -> Shed { self.shed }

    /// Look at a finished frame and decide.
    pub fn frame(&mut self, cost: FrameCost) -> Verdict {
        let (pass, pass_us) = cost.worst();
        // HARD LIMIT IS IMMEDIATE. There is no patience here: a frame
        // this expensive means the next one may not return at all, and
        // waiting to be sure costs the whole device.
        if cost.total_us >= self.hard_us {
            if self.shed == Shed::Exhausted {
                return Verdict::Abort { why: format!(
                    "the GPU took {:.1}s on this frame ({pass} pass {:.1}s) with everything \
                     sheddable already off. Stopping before the driver's watchdog kills the \
                     device -- on this system that takes down every other application too.",
                    cost.total_us as f64 / 1e6, pass_us as f64 / 1e6) };
            }
            self.shed = self.shed.next();
            self.over = 0;
            self.calm = 0;
            return Verdict::ShedNow { level: self.shed, why: format!(
                "{:.1}s frame ({pass} pass {:.1}s) is close to the driver watchdog; \
                 dropping {}", cost.total_us as f64 / 1e6, pass_us as f64 / 1e6,
                self.shed.name()) };
        }
        if cost.total_us >= self.soft_us {
            self.calm = 0;
            self.over += 1;
            if self.over > self.patience {
                self.over = 0;
                if self.shed == Shed::Exhausted {
                    // over the SOFT limit with nothing left is not
                    // fatal: it is playable, badly. Say so once rather
                    // than killing a session that still works.
                    return Verdict::Fine;
                }
                self.shed = self.shed.next();
                return Verdict::ShedNow { level: self.shed, why: format!(
                    "{} frames over {:.0}ms ({pass} pass worst); dropping {}",
                    self.patience + 1, self.soft_us as f64 / 1000.0, self.shed.name()) };
            }
            return Verdict::Fine;
        }
        // calm again: give something back, slowly. Restoring fast means
        // oscillating between shedding and restoring, which looks worse
        // than staying shed.
        self.over = 0;
        self.calm += 1;
        if self.calm > 240 && self.shed != Shed::Nothing {
            self.calm = 0;
            self.shed = match self.shed {
                Shed::Exhausted => Shed::Distance,
                Shed::Distance => Shed::Volumetrics,
                Shed::Volumetrics => Shed::Shadows,
                _ => Shed::Nothing,
            };
        }
        Verdict::Fine
    }

    /// A frame that never came back. The driver has almost certainly
    /// already reset by the time this is called, but saying so is the
    /// difference between a bug report that can be acted on and "it
    /// froze".
    pub fn lost_device(&self, waited_ms: u64) -> String {
        format!("the GPU stopped responding after {waited_ms}ms. This is a driver-level \
                 device loss: every GPU application on the machine is affected and a \
                 reboot is usually needed. Load shed at the time: {}.", self.shed.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(total_ms: u64, main_ms: u64) -> FrameCost {
        FrameCost { total_us: total_ms * 1000, main_us: main_ms * 1000,
                    shadow_us: (total_ms - main_ms) * 1000, froxel_us: 100, cull_us: 50 }
    }

    #[test]
    fn a_frame_near_the_watchdog_sheds_immediately_without_patience() {
        // there is no waiting to be sure: a frame this expensive means
        // the next may not return at all
        let mut g = Guard::default();
        match g.frame(cost(1800, 1500)) {
            Verdict::ShedNow { level, why } => {
                assert_eq!(level, Shed::Shadows, "the cheapest thing first");
                assert!(why.contains("1.8s") && why.contains("main pass"), "{why}");
                assert!(why.contains("watchdog"), "{why}");
            }
            o => panic!("{o:?}"),
        }
        // the observed frame: 11.2s total, 9.7s main
        let mut g2 = Guard::default();
        assert!(matches!(g2.frame(cost(11200, 9700)), Verdict::ShedNow { .. }));
    }

    #[test]
    fn one_slow_frame_does_not_strip_the_renderer() {
        // a load spike is not a watchdog risk, and shedding on the
        // first slow frame would make every chunk-load stutter
        // permanent
        let mut g = Guard::default();
        assert_eq!(g.frame(cost(900, 700)), Verdict::Fine);
        assert_eq!(g.frame(cost(900, 700)), Verdict::Fine);
        assert_eq!(g.shed_level(), Shed::Nothing, "nothing given up yet");
        // sustained is different
        assert!(matches!(g.frame(cost(900, 700)), Verdict::ShedNow { .. }));
    }

    #[test]
    fn it_aborts_rather_than_submitting_a_frame_that_may_kill_the_device() {
        // a process that exits costs the player one session; letting
        // the submission through costs every application they had open
        let mut g = Guard::default();
        // burn through everything sheddable
        for _ in 0..4 { let _ = g.frame(cost(3000, 2500)); }
        assert_eq!(g.shed_level(), Shed::Exhausted);
        match g.frame(cost(9000, 8000)) {
            Verdict::Abort { why } => {
                assert!(why.contains("9.0s") && why.contains("main pass"), "{why}");
                assert!(why.contains("everything sheddable already off"), "{why}");
                assert!(why.contains("every other application"),
                        "it says what is at stake: {why}");
            }
            o => panic!("expected an abort: {o:?}"),
        }
    }

    #[test]
    fn being_merely_slow_with_nothing_left_is_not_fatal() {
        // over the SOFT limit with nothing left is playable, badly.
        // Killing a session that still works would be worse than the
        // stutter.
        let mut g = Guard::default();
        for _ in 0..4 { let _ = g.frame(cost(3000, 2500)); }
        assert_eq!(g.shed_level(), Shed::Exhausted);
        for _ in 0..6 {
            assert_eq!(g.frame(cost(800, 600)), Verdict::Fine, "slow but survivable");
        }
    }

    #[test]
    fn the_blame_lands_on_the_right_pass() {
        let shadow_heavy = FrameCost { total_us: 2_000_000, main_us: 300_000,
                                       shadow_us: 1_600_000, froxel_us: 50_000, cull_us: 100 };
        assert_eq!(shadow_heavy.worst().0, "shadow");
        let observed = FrameCost { total_us: 11_162_700, main_us: 9_683_400,
                                   shadow_us: 1_346_100, froxel_us: 133_200, cull_us: 31 };
        assert_eq!(observed.worst().0, "main", "9.7s of 11.2s was the main pass");
        let mut g = Guard::default();
        match g.frame(observed) {
            Verdict::ShedNow { why, .. } => assert!(why.contains("main pass 9.7s"), "{why}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn what_is_given_up_is_given_back_slowly() {
        // restoring fast means oscillating between shedding and
        // restoring, which looks worse than staying shed
        let mut g = Guard::default();
        let _ = g.frame(cost(2000, 1800));
        assert_eq!(g.shed_level(), Shed::Shadows);
        for _ in 0..200 { assert_eq!(g.frame(cost(8, 5)), Verdict::Fine); }
        assert_eq!(g.shed_level(), Shed::Shadows, "not yet -- 200 calm frames is not enough");
        for _ in 0..60 { let _ = g.frame(cost(8, 5)); }
        assert_eq!(g.shed_level(), Shed::Nothing, "after a few seconds of calm, given back");
    }

    #[test]
    fn a_lost_device_is_reported_as_what_it_is() {
        let g = Guard::default();
        let m = g.lost_device(4200);
        assert!(m.contains("4200ms") && m.contains("reboot"), "{m}");
        assert!(m.contains("every GPU application"), "it says the scope: {m}");
    }
}
