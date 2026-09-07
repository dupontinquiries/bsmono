//! jobmetrics — what a run is actually costing, summarised for a pane.
//!
//! The app already collects a row per frame (analyse ms, upscale ms, and
//! how much of the frame had to be redrawn). It had nowhere to say so: the
//! numbers went into a log line every twenty-five frames and were otherwise
//! invisible, which is no use for the question people actually ask, which
//! is "is this going to take twenty minutes or two hours, and why".
//!
//! Pure, over the same `(analyse, upscale, redraw fraction)` triple the app
//! already has, so the pane is a view of existing data rather than a new
//! measurement path.
//!
//! # Averages over a whole run are the wrong number
//!
//! An upscale's cost is not stationary — a title card is nearly free and an
//! action sequence is not — so the mean over ten thousand frames tells you
//! about the beginning of the film. [`Summary::recent`] is a window over the
//! last [`WINDOW`] frames, which is what predicts the next minute.

/// Frames in the rolling window. Enough to smooth a scene change, short
/// enough to follow one.
pub const WINDOW: usize = 120;

/// One frame's cost, as the app already records it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub analyze_ms: f32,
    pub upscale_ms: f32,
    /// 0..1 — how much of the frame the network had to redraw. The cost
    /// driver: block matching is cheap, the network is not.
    pub fraction: f32,
    pub redraw: bool,
}

/// The pane's numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub frames: usize,
    /// Mean total ms per frame over the window.
    pub ms_per_frame: f32,
    pub analyze_ms: f32,
    pub upscale_ms: f32,
    /// Mean redrawn fraction — how much work the reuse is actually saving.
    pub fraction: f32,
    /// Full redraws in the window: scene cuts, or the matcher giving up.
    pub redraws: usize,
}

impl Summary {
    /// Frames per second at the current rate. `None` before anything has
    /// been measured, rather than a fabricated zero.
    pub fn fps(&self) -> Option<f32> {
        (self.ms_per_frame > 0.0).then(|| 1000.0 / self.ms_per_frame)
    }

    /// How long `remaining` frames will take at the current rate.
    ///
    /// From the WINDOW, not the whole run, which is the point: an estimate
    /// built on the title card is wrong for the rest of the film, and an
    /// ETA that keeps rising is worse than none.
    pub fn eta_secs(&self, remaining: usize) -> Option<f32> {
        (self.frames > 0 && self.ms_per_frame > 0.0)
            .then(|| remaining as f32 * self.ms_per_frame / 1000.0)
    }

    /// One line for a pane.
    pub fn line(&self) -> String {
        match self.fps() {
            None => "no frames yet".to_string(),
            Some(fps) => format!(
                "{:.1} fps · {:.0} ms/frame (A {:.0} · U {:.0}) · {:.0}% redrawn · {} full",
                fps, self.ms_per_frame, self.analyze_ms, self.upscale_ms,
                self.fraction * 100.0, self.redraws
            ),
        }
    }
}

/// Summarise the last [`WINDOW`] frames of `all`.
pub fn recent(all: &[Frame]) -> Summary {
    let w = &all[all.len().saturating_sub(WINDOW)..];
    let n = w.len();
    if n == 0 {
        return Summary { frames: 0, ms_per_frame: 0.0, analyze_ms: 0.0,
                         upscale_ms: 0.0, fraction: 0.0, redraws: 0 };
    }
    let inv = 1.0 / n as f32;
    let a: f32 = w.iter().map(|f| f.analyze_ms).sum::<f32>() * inv;
    let u: f32 = w.iter().map(|f| f.upscale_ms).sum::<f32>() * inv;
    Summary {
        frames: n,
        ms_per_frame: a + u,
        analyze_ms: a,
        upscale_ms: u,
        fraction: w.iter().map(|f| f.fraction).sum::<f32>() * inv,
        redraws: w.iter().filter(|f| f.redraw).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(a: f32, u: f32, frac: f32, redraw: bool) -> Frame {
        Frame { analyze_ms: a, upscale_ms: u, fraction: frac, redraw }
    }

    #[test]
    fn the_window_follows_the_recent_scene_not_the_whole_film() {
        // A cheap title card, then an expensive action scene. A mean over
        // everything would still be reporting the title card.
        let mut all: Vec<Frame> = (0..500).map(|_| f(1.0, 4.0, 0.05, false)).collect();
        all.extend((0..WINDOW).map(|_| f(3.0, 40.0, 0.8, false)));
        let s = recent(&all);
        assert_eq!(s.frames, WINDOW);
        assert!((s.ms_per_frame - 43.0).abs() < 0.01, "{}", s.ms_per_frame);
        assert!(s.fraction > 0.7, "still averaging in the cheap frames");
        // The whole-run mean would be about 12 ms — off by nearly 4x.
        assert!(s.ms_per_frame > 40.0);
    }

    #[test]
    fn an_eta_comes_from_the_window_and_is_absent_before_any_data() {
        let empty = recent(&[]);
        assert_eq!(empty.frames, 0);
        assert!(empty.fps().is_none(), "no fabricated zero");
        assert!(empty.eta_secs(1000).is_none());
        assert_eq!(empty.line(), "no frames yet");

        let steady: Vec<Frame> = (0..10).map(|_| f(2.0, 8.0, 0.3, false)).collect();
        let s = recent(&steady);
        assert!((s.fps().unwrap() - 100.0).abs() < 0.01, "10 ms/frame");
        // 600 frames left at 10 ms each is six seconds.
        assert!((s.eta_secs(600).unwrap() - 6.0).abs() < 0.01);
        assert_eq!(s.eta_secs(0), Some(0.0));
    }

    #[test]
    fn a_short_run_summarises_what_it_has_rather_than_waiting_for_a_full_window() {
        let three = vec![f(1.0, 9.0, 0.1, false), f(1.0, 9.0, 0.1, true), f(4.0, 6.0, 0.1, false)];
        let s = recent(&three);
        assert_eq!(s.frames, 3);
        assert_eq!(s.redraws, 1, "full redraws are counted, not averaged away");
        assert!((s.ms_per_frame - 10.0).abs() < 0.01);
        assert!(s.line().contains("1 full"), "{}", s.line());
    }

    #[test]
    fn the_line_is_readable_and_names_where_the_time_goes() {
        // The pane exists to answer "why is this slow" — so the split
        // between matching and the network has to be visible, not just a
        // total.
        let heavy: Vec<Frame> = (0..20).map(|_| f(2.0, 48.0, 0.9, false)).collect();
        let l = recent(&heavy).line();
        assert!(l.contains("fps") && l.contains("ms/frame"), "{l}");
        assert!(l.contains("A 2") && l.contains("U 48"), "the split is shown: {l}");
        assert!(l.contains("90% redrawn"), "{l}");
    }
}
