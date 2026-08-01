//! soundscape — the seamless-loop finder, ported from `SoundscapePage.qml`.
//!
//! A spatial home system plays ambience for hours, so its source material has to
//! **loop without a seam**. The page takes an input file, a target length with a
//! tolerance, and a candidate budget, and finds the cut that joins back to
//! itself most convincingly.
//!
//! ## What makes a loop seamless
//!
//! Two things, and the second is the one people forget:
//!
//! * The audio at the **end** must resemble the audio at the **start** — a cut
//!   from a loud passage back to a quiet one is audible however short the
//!   crossfade.
//! * The **join must not click**. A crossfade hides a spectral mismatch, but it
//!   cannot hide a discontinuity in the waveform itself: if the two ends are at
//!   opposite phase, the sum passes through zero and you hear it as a dip.
//!
//! So a candidate is scored on both, and the score is reported per component so
//! the UI can say *why* a loop was rejected rather than just ranking it.
//!
//! ## Why the candidate budget matters
//!
//! `nCandidates: 2000` in the QML. A loop search is quadratic in the obvious
//! implementation — every start against every end — so the budget is what keeps
//! it interactive on a long recording. Candidates are sampled evenly across the
//! legal range rather than taken from the front, because taking the first N
//! searches only the beginning of the file and misses the good loop at 4:00.

/// The knobs, with the QML's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopParams {
    pub target_s: f64,
    pub tolerance_s: f64,
    pub candidates: usize,
    pub bitrate_kbps: u32,
    pub fade_ms: u32,
}

impl Default for LoopParams {
    fn default() -> Self {
        LoopParams { target_s: 8.0, tolerance_s: 4.0, candidates: 2000,
                     bitrate_kbps: 96, fade_ms: 50 }
    }
}

impl LoopParams {
    /// The legal loop lengths, in seconds. Clamped at zero because a negative
    /// tolerance is a typo and a zero-length loop is not a loop.
    pub fn range(&self) -> (f64, f64) {
        let t = self.tolerance_s.abs();
        (((self.target_s - t).max(0.1)), (self.target_s + t).max(0.2))
    }

    /// Problems worth refusing before a long search.
    pub fn problems(&self, source_s: f64) -> Vec<String> {
        let mut v = Vec::new();
        let (lo, _hi) = self.range();
        if self.target_s <= 0.0 { v.push("target length must be positive".into()); }
        if lo > source_s {
            v.push(format!("the shortest allowed loop ({lo:.1}s) is longer than the source ({source_s:.1}s)"));
        }
        // A crossfade longer than the loop eats the whole thing.
        if self.fade_ms as f64 / 1000.0 > self.target_s * 0.5 {
            v.push("the crossfade is longer than half the loop".into());
        }
        if self.candidates == 0 { v.push("no candidates to search".into()); }
        v
    }
}

/// One candidate loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub start_s: f64,
    pub end_s: f64,
    /// How alike the two ends sound. 1 = identical.
    pub similarity: f64,
    /// How cleanly the waveform joins. 1 = no discontinuity.
    pub continuity: f64,
}

impl Candidate {
    pub fn length_s(&self) -> f64 { self.end_s - self.start_s }

    /// Overall quality.
    ///
    /// Similarity is weighted higher because a spectral mismatch is audible for
    /// the whole loop, while a small discontinuity is audible once and a
    /// crossfade can soften it. Reported alongside the components rather than
    /// instead of them, so the UI can say why a loop scored badly.
    pub fn score(&self) -> f64 { self.similarity * 0.65 + self.continuity * 0.35 }
}

/// Where to sample candidate loop starts.
///
/// Evenly across the legal range, not from the front: taking the first N
/// searches only the beginning of the file and misses the good loop at 4:00.
pub fn candidate_starts(source_s: f64, p: &LoopParams) -> Vec<f64> {
    let (lo, _) = p.range();
    let last_start = source_s - lo;
    if last_start <= 0.0 || p.candidates == 0 { return Vec::new(); }
    let n = p.candidates.min(100_000);
    if n == 1 { return vec![0.0]; }
    (0..n).map(|i| last_start * i as f64 / (n - 1) as f64).collect()
}

/// Rank candidates best first.
///
/// Ties break on **length closest to target**, then on start time — so the same
/// input always produces the same loop. A finder that returns a different answer
/// each run cannot be A/B tested against its own previous output, which is the
/// main thing someone tuning ambience actually does.
pub fn rank(mut c: Vec<Candidate>, target_s: f64) -> Vec<Candidate> {
    c.sort_by(|a, b| {
        b.score().partial_cmp(&a.score()).unwrap()
            .then((a.length_s() - target_s).abs()
                  .partial_cmp(&(b.length_s() - target_s).abs()).unwrap())
            .then(a.start_s.partial_cmp(&b.start_s).unwrap())
    });
    c
}

/// The page's state.
#[derive(Debug, Clone, Default)]
pub struct Soundscape {
    pub input_path: String,
    pub output_path: String,
    pub params: LoopParams,
    pub source_s: f64,
    pub results: Vec<Candidate>,
    pub status: String,
}

impl Soundscape {
    /// `suggestOutput(input)` — derive an output name from the input.
    ///
    /// Suffixed rather than replacing the extension, so the source is never at
    /// risk of being overwritten by its own output. That is a cheap guarantee
    /// and the alternative loses someone's recording.
    pub fn suggest_output(input: &str) -> String {
        if input.is_empty() { return String::new(); }
        let (stem, _ext) = match input.rfind('.') {
            Some(i) if i > 0 => (&input[..i], &input[i..]),
            _ => (input, ""),
        };
        format!("{stem}_loop.opus")
    }

    pub fn set_input(&mut self, path: &str) {
        self.input_path = path.to_string();
        // Only fill the output if the user has not chosen one — overwriting a
        // deliberate choice on every input change is infuriating.
        if self.output_path.is_empty() {
            self.output_path = Self::suggest_output(path);
        }
    }

    pub fn best(&self) -> Option<&Candidate> { self.results.first() }

    pub fn can_run(&self) -> bool {
        !self.input_path.is_empty() && self.params.problems(self.source_s).is_empty()
    }
}

/// Draw the page. Returns true when the user asked to run.
pub fn soundscape_ui(ui: &mut egui::Ui, s: &mut Soundscape,
                     colors: &uikit_egui::PaletteColors) -> bool {
    let mut run = false;
    ui.horizontal(|ui| {
        ui.label("in");
        let mut p = s.input_path.clone();
        if ui.text_edit_singleline(&mut p).changed() { s.set_input(&p); }
    });
    ui.horizontal(|ui| {
        ui.label("out");
        ui.text_edit_singleline(&mut s.output_path);
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut s.params.target_s).speed(0.1)
            .range(0.5..=600.0).prefix("target "). suffix("s"));
        ui.add(egui::DragValue::new(&mut s.params.tolerance_s).speed(0.1)
            .range(0.0..=60.0).prefix("± ").suffix("s"));
        ui.add(egui::DragValue::new(&mut s.params.fade_ms)
            .range(0..=2000).prefix("fade ").suffix("ms"));
    });
    let probs = s.params.problems(s.source_s);
    for p in &probs {
        ui.label(egui::RichText::new(format!("⚠ {p}")).color(colors.warning).small());
    }
    ui.separator();
    if ui.add_enabled(s.can_run(), egui::Button::new("find loop")).clicked() { run = true; }
    if let Some(b) = s.best() {
        ui.label(egui::RichText::new(format!(
            "{:.2}s loop at {:.2}s — match {:.0}%, join {:.0}%",
            b.length_s(), b.start_s, b.similarity * 100.0, b.continuity * 100.0)).strong());
    } else if !s.results.is_empty() {
        ui.label(egui::RichText::new("no usable loop found").weak());
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(start: f64, len: f64, sim: f64, cont: f64) -> Candidate {
        Candidate { start_s: start, end_s: start + len, similarity: sim, continuity: cont }
    }

    #[test]
    fn the_defaults_match_the_qml() {
        // A port that quietly changes defaults produces different output from
        // the same file, which reads as the port being broken.
        let p = LoopParams::default();
        assert_eq!(p.target_s, 8.0);
        assert_eq!(p.tolerance_s, 4.0);
        assert_eq!(p.candidates, 2000);
        assert_eq!(p.bitrate_kbps, 96);
        assert_eq!(p.fade_ms, 50);
        assert_eq!(p.range(), (4.0, 12.0));
    }

    #[test]
    fn candidates_are_spread_across_the_file_not_taken_from_the_front() {
        // Taking the first N searches only the beginning and misses the good
        // loop at 4:00.
        let p = LoopParams { candidates: 5, ..Default::default() };
        let starts = candidate_starts(300.0, &p);
        assert_eq!(starts.len(), 5);
        assert!((starts[0]).abs() < 1e-9);
        assert!(starts[4] > 250.0, "the last candidate is near the end: {}", starts[4]);
        for w in starts.windows(2) { assert!(w[1] > w[0], "strictly increasing"); }
    }

    #[test]
    fn a_source_shorter_than_the_loop_yields_no_candidates() {
        let p = LoopParams::default();          // shortest legal loop is 4s
        assert!(candidate_starts(2.0, &p).is_empty());
        assert!(!p.problems(2.0).is_empty(), "and it is reported, not silent");
    }

    #[test]
    fn similarity_outweighs_continuity_because_it_is_audible_for_the_whole_loop() {
        // A spectral mismatch is heard every time round; a small discontinuity
        // is heard once and a crossfade softens it.
        let good_match = cand(0.0, 8.0, 0.95, 0.50);
        let good_join  = cand(0.0, 8.0, 0.50, 0.95);
        assert!(good_match.score() > good_join.score(),
                "{} vs {}", good_match.score(), good_join.score());
    }

    #[test]
    fn ranking_is_deterministic_so_the_same_file_gives_the_same_loop() {
        // A finder that answers differently each run cannot be A/B tested
        // against its own previous output, which is what tuning ambience is.
        let c = vec![cand(10.0, 8.0, 0.8, 0.8), cand(2.0, 8.0, 0.8, 0.8),
                     cand(5.0, 12.0, 0.8, 0.8)];
        let a = rank(c.clone(), 8.0);
        let b = rank(c, 8.0);
        assert_eq!(a, b);
        // equal scores break on length closest to target, then start time
        assert_eq!(a[0].start_s, 2.0, "8s loops beat the 12s one, earliest first");
        assert_eq!(a[2].length_s(), 12.0);
    }

    #[test]
    fn the_output_name_can_never_be_the_input() {
        // The alternative loses someone's recording.
        let out = Soundscape::suggest_output("/music/rain.wav");
        assert_eq!(out, "/music/rain_loop.opus");
        assert_ne!(out, "/music/rain.wav");
        // a file already ending in .opus still gets a distinct name
        assert_eq!(Soundscape::suggest_output("/x/a.opus"), "/x/a_loop.opus");
        // and no extension is handled
        assert_eq!(Soundscape::suggest_output("/x/noext"), "/x/noext_loop.opus");
        assert_eq!(Soundscape::suggest_output(""), "");
    }

    #[test]
    fn a_chosen_output_is_not_overwritten_when_the_input_changes() {
        // Clobbering a deliberate choice on every input edit is infuriating.
        let mut s = Soundscape::default();
        s.set_input("/a/one.wav");
        assert_eq!(s.output_path, "/a/one_loop.opus");
        s.output_path = "/somewhere/mine.opus".into();
        s.set_input("/a/two.wav");
        assert_eq!(s.output_path, "/somewhere/mine.opus", "the user's choice stands");
    }

    #[test]
    fn a_crossfade_longer_than_the_loop_is_refused() {
        // It would eat the whole thing.
        let p = LoopParams { target_s: 2.0, fade_ms: 1500, ..Default::default() };
        assert!(p.problems(60.0).iter().any(|s| s.contains("crossfade")));
        let ok = LoopParams { target_s: 8.0, fade_ms: 50, ..Default::default() };
        assert!(ok.problems(60.0).is_empty());
    }

    #[test]
    fn a_negative_tolerance_is_treated_as_its_magnitude() {
        // A typed minus sign should not invert the range into nothing.
        let p = LoopParams { tolerance_s: -4.0, ..Default::default() };
        assert_eq!(p.range(), (4.0, 12.0));
    }
}
