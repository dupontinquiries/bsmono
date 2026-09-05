//! courtesy — how much of the machine a long job is allowed to take.
//!
//! An upscale runs for minutes and will use every core and all the GPU it
//! is given. On a dedicated box that is correct. On the machine someone is
//! also *using*, it is why the pointer stutters and video tears while a job
//! runs — and "sickening" is the right word, because dropped compositor
//! frames at irregular intervals are far worse to look at than a steady
//! lower frame rate.
//!
//! # The number that matters is the frame budget, not the core count
//!
//! A compositor needs to produce a frame every [`FRAME_BUDGET_MS`]
//! (16.7 ms at 60 Hz). Leaving a core free does not help if the job holds
//! that core in one uninterruptible 400 ms chunk — the desktop still misses
//! twenty-four frames in a row, which reads as a freeze. Conversely a job
//! that uses every core but yields inside the budget is barely noticeable.
//!
//! So this module decides two things and the SLICE is the important one:
//!
//! * how many workers, and at what `nice` — coarse, set once;
//! * how long a worker may run before it yields — fine, and what actually
//!   determines whether the desktop stays smooth.
//!
//! Pure: it returns numbers. Applying them (spawning that many threads,
//! calling `setpriority`, sleeping between slices) is the caller's, so the
//! policy can be tested without a machine to be rude to.

/// One compositor frame at 60 Hz. The unit everything here is measured in,
/// because it is the unit the person watching perceives.
pub const FRAME_BUDGET_MS: f32 = 16.7;

/// How much of the machine the job may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Politeness {
    /// The machine is in use. Stay out of the way even at real cost to
    /// throughput.
    Background,
    /// The default: fast, but never the reason a frame is missed.
    Balanced,
    /// Nobody is watching — a render farm, a headless box, an overnight
    /// run. Take everything.
    Foreground,
}

impl Politeness {
    pub fn label(self) -> &'static str {
        match self {
            Politeness::Background => "background (stay out of the way)",
            Politeness::Balanced => "balanced",
            Politeness::Foreground => "foreground (take the machine)",
        }
    }
}

/// Workers to run on a machine with `cores`.
///
/// Background and Balanced always leave at least one core, INCLUDING on a
/// two-core machine — that is exactly where the desktop suffers most, and
/// "leave 25%" rounds to zero there. Foreground takes the lot.
pub fn worker_threads(cores: usize, p: Politeness) -> usize {
    let c = cores.max(1);
    match p {
        Politeness::Foreground => c,
        Politeness::Balanced => (c.saturating_sub(1)).max(1),
        // Half, and never more than cores-1.
        Politeness::Background => (c / 2).clamp(1, c.saturating_sub(1).max(1)),
    }
}

/// `nice` value for the job's threads and its ffmpeg children.
///
/// Positive is lower priority on POSIX. 10 is the conventional "batch work"
/// level and 19 the floor; Foreground stays at 0 rather than going negative,
/// because raising priority above the desktop needs privileges and wanting
/// the CPU is not a reason to outrank the session.
pub fn nice_value(p: Politeness) -> i32 {
    match p {
        Politeness::Background => 15,
        Politeness::Balanced => 5,
        Politeness::Foreground => 0,
    }
}

/// How long a worker may run before yielding, and how long to yield for.
///
/// The work slice is kept under one frame budget for the polite modes so a
/// worker can never be the reason a compositor frame is late. Foreground
/// uses a long slice because throughput is the only goal there.
pub fn slice(p: Politeness) -> (f32, f32) {
    match p {
        // Half a frame of work, half a frame of yield: the desktop always
        // has a gap inside every frame to be scheduled in.
        Politeness::Background => (FRAME_BUDGET_MS * 0.5, FRAME_BUDGET_MS * 0.5),
        // Most of a frame, a small gap. Enough for the compositor to get in
        // without giving up much throughput.
        Politeness::Balanced => (FRAME_BUDGET_MS * 0.8, FRAME_BUDGET_MS * 0.2),
        Politeness::Foreground => (250.0, 0.0),
    }
}

/// The fraction of wall-clock the job actually gets, given its slice. What
/// throughput the politeness costs, so the choice can be stated honestly in
/// a UI rather than implied.
pub fn duty_cycle(p: Politeness) -> f32 {
    let (work, rest) = slice(p);
    if work + rest <= 0.0 { return 1.0; }
    work / (work + rest)
}

/// `-threads` for a spawned ffmpeg. ffmpeg defaults to every core and is
/// usually the loudest neighbour in the whole pipeline, so it gets the same
/// budget as the workers rather than being left to help itself.
pub fn ffmpeg_threads(cores: usize, p: Politeness) -> usize {
    worker_threads(cores, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Politeness; 3] =
        [Politeness::Background, Politeness::Balanced, Politeness::Foreground];

    #[test]
    fn a_polite_job_never_holds_a_worker_past_one_compositor_frame() {
        // The property the whole module exists for: a slice longer than a
        // frame means a missed frame, and missed frames at irregular
        // intervals are what reads as stutter.
        for p in [Politeness::Background, Politeness::Balanced] {
            let (work, rest) = slice(p);
            assert!(work < FRAME_BUDGET_MS, "{p:?} works {work}ms, a frame is {FRAME_BUDGET_MS}");
            assert!(rest > 0.0, "{p:?} never yields");
            assert!(work + rest <= FRAME_BUDGET_MS + 0.1, "{p:?} cycle overruns a frame");
        }
        // Foreground is allowed to be rude; that is what it is for.
        assert!(slice(Politeness::Foreground).0 > FRAME_BUDGET_MS);
        assert_eq!(slice(Politeness::Foreground).1, 0.0);
    }

    #[test]
    fn a_core_is_always_left_for_the_desktop_even_on_a_two_core_machine() {
        // Where "leave 25%" quietly rounds to zero and the machine locks up.
        for cores in 1..=32 {
            for p in [Politeness::Background, Politeness::Balanced] {
                let w = worker_threads(cores, p);
                assert!(w >= 1, "{cores} cores, {p:?}: no workers at all");
                if cores > 1 {
                    assert!(w < cores, "{cores} cores, {p:?}: took all {w}");
                }
            }
            assert_eq!(worker_threads(cores, Politeness::Foreground), cores);
        }
        // A machine that reports zero cores still gets one worker.
        assert_eq!(worker_threads(0, Politeness::Balanced), 1);
    }

    #[test]
    fn politeness_is_ordered_in_every_dimension() {
        let (bg, bal, fg) = (Politeness::Background, Politeness::Balanced, Politeness::Foreground);
        // More polite: fewer threads, higher nice, smaller duty cycle.
        assert!(worker_threads(16, bg) <= worker_threads(16, bal));
        assert!(worker_threads(16, bal) <= worker_threads(16, fg));
        assert!(nice_value(bg) > nice_value(bal) && nice_value(bal) > nice_value(fg));
        assert!(duty_cycle(bg) < duty_cycle(bal) && duty_cycle(bal) < duty_cycle(fg));
        // nice stays inside what POSIX allows without privileges.
        for p in ALL {
            assert!((0..=19).contains(&nice_value(p)), "{p:?} nice out of range");
        }
    }

    #[test]
    fn the_throughput_cost_is_statable_rather_than_hidden() {
        // A UI should be able to say "this runs at 80% speed", so the number
        // has to be real and not a vibe.
        assert!((duty_cycle(Politeness::Background) - 0.5).abs() < 0.01);
        assert!((duty_cycle(Politeness::Balanced) - 0.8).abs() < 0.01);
        assert_eq!(duty_cycle(Politeness::Foreground), 1.0);
    }

    #[test]
    fn ffmpeg_gets_the_same_budget_as_the_workers() {
        // Left alone it takes every core and undoes the whole policy.
        for cores in [2usize, 4, 8, 16] {
            for p in ALL {
                assert_eq!(ffmpeg_threads(cores, p), worker_threads(cores, p));
            }
        }
    }
}
