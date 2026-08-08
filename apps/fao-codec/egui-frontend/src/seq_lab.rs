//! seq_lab — the sequence lab: the temporal encoder, driven and reported.
//!
//! The Encode Lab exercises `encode_iframe_recon` — one frame, no history. That
//! is the right first slice and it cannot answer the question a temporal codec
//! exists to answer: **what does the second frame cost?**
//!
//! `V2Encoder` is what the GTK app reaches for (`fao_app/src/main.rs`, the
//! `use_v2` branch) and nothing in the port could drive it. This is that page.
//!
//! ## A single number is the wrong report for a sequence
//!
//! `FrameResult` carries `frame_type`, `bits`, `psnr`, `skip_pct` and
//! `encode_ms` per frame, and the interesting behaviour is entirely in how
//! those DIFFER between frames. A K-frame costing ten times a T-frame is the
//! codec working; a T-frame costing as much as a K-frame is motion estimation
//! failing, and an average hides both. So the lab keeps every frame and reports
//! the split.
//!
//! ## Skip percentage is the honest efficiency signal
//!
//! Bits alone cannot distinguish "this frame was cheap because nothing moved"
//! from "this frame was cheap because the quantiser threw it away". `skip_pct`
//! separates them: high skip with high PSNR is a static scene coded well; high
//! skip with low PSNR is the encoder giving up. A lab that reported only bits
//! per frame would make those look identical, and they call for opposite fixes.
//!
//! ## Nothing here decides quality
//!
//! The lab measures and reports. It does not pick a quality, retune a
//! threshold, or "improve" a result — every number it shows comes from the
//! encoder unmodified, so a regression in the codec shows up here rather than
//! being smoothed out by the page displaying it.

/// One frame's report, as `FrameResult` gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStat {
    /// 0 = K (intra), 1 = T (temporal). The encoder's own tag.
    pub frame_type: u8,
    pub bits: u64,
    pub psnr: f64,
    pub skip_pct: f64,
    pub encode_ms: f64,
}

impl FrameStat {
    pub fn is_keyframe(&self) -> bool { self.frame_type == 0 }

    /// Bits per pixel for a given frame size.
    ///
    /// The comparable unit: bits alone cannot be compared between a 320×240
    /// test pattern and a 1080p clip, and someone reading a lab will do exactly
    /// that comparison if the page lets them.
    pub fn bpp(&self, w: usize, h: usize) -> f64 {
        let px = (w * h) as f64;
        if px <= 0.0 { return 0.0; }
        self.bits as f64 / px
    }
}

/// A whole run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeqRun {
    pub width: usize,
    pub height: usize,
    pub quality: f32,
    frames: Vec<FrameStat>,
}

impl SeqRun {
    pub fn new(width: usize, height: usize, quality: f32) -> SeqRun {
        SeqRun { width, height, quality, frames: Vec::new() }
    }

    pub fn push(&mut self, f: FrameStat) { self.frames.push(f); }
    pub fn frames(&self) -> &[FrameStat] { &self.frames }
    pub fn len(&self) -> usize { self.frames.len() }
    pub fn is_empty(&self) -> bool { self.frames.is_empty() }

    /// Mean bits over frames of one kind.
    ///
    /// Returns `None` for a kind with no frames rather than zero: a run with no
    /// T-frames has no T-frame cost, and reporting 0 would read as "temporal
    /// coding is free" — the most flattering possible misreading.
    pub fn mean_bits(&self, keyframes: bool) -> Option<f64> {
        let v: Vec<u64> = self.frames.iter()
            .filter(|f| f.is_keyframe() == keyframes)
            .map(|f| f.bits).collect();
        if v.is_empty() { return None; }
        Some(v.iter().sum::<u64>() as f64 / v.len() as f64)
    }

    /// How much cheaper a T-frame is than a K-frame, as a ratio.
    ///
    /// The single number this page exists to produce. `None` when the run has
    /// no frames of one kind, because a ratio against nothing is not a ratio.
    pub fn temporal_ratio(&self) -> Option<f64> {
        let k = self.mean_bits(true)?;
        let t = self.mean_bits(false)?;
        if k <= 0.0 { return None; }
        Some(t / k)
    }

    /// Total bits across the run.
    pub fn total_bits(&self) -> u64 { self.frames.iter().map(|f| f.bits).sum() }

    /// Mean PSNR. Weighted per frame, not per bit — a viewer sees frames.
    pub fn mean_psnr(&self) -> Option<f64> {
        if self.frames.is_empty() { return None; }
        // Non-finite PSNR means a perfect reconstruction (zero error), which
        // the codec reports as infinity. Averaging that in makes the mean
        // infinite and destroys the whole report, so those frames are counted
        // separately rather than dropped silently.
        let finite: Vec<f64> = self.frames.iter().map(|f| f.psnr)
            .filter(|p| p.is_finite()).collect();
        if finite.is_empty() { return None; }
        Some(finite.iter().sum::<f64>() / finite.len() as f64)
    }

    /// Frames whose reconstruction was exact.
    pub fn lossless_frames(&self) -> usize {
        self.frames.iter().filter(|f| !f.psnr.is_finite()).count()
    }

    /// Mean skip percentage.
    pub fn mean_skip(&self) -> Option<f64> {
        if self.frames.is_empty() { return None; }
        Some(self.frames.iter().map(|f| f.skip_pct).sum::<f64>()
             / self.frames.len() as f64)
    }

    /// A frame that cost as much as a keyframe is motion estimation failing.
    ///
    /// Reported rather than hidden: it is the single most useful thing a
    /// sequence lab can point at, and an average would bury it.
    pub fn expensive_temporal(&self, threshold: f64) -> Vec<usize> {
        let Some(k) = self.mean_bits(true) else { return Vec::new() };
        self.frames.iter().enumerate()
            .filter(|(_, f)| !f.is_keyframe() && f.bits as f64 >= k * threshold)
            .map(|(i, _)| i)
            .collect()
    }

    /// Total encode time.
    pub fn total_ms(&self) -> f64 { self.frames.iter().map(|f| f.encode_ms).sum() }
}

/// Draw the report.
pub fn seq_lab_ui(ui: &mut egui::Ui, run: &SeqRun, c: &uikit_egui::PaletteColors) {
    if run.is_empty() {
        ui.colored_label(c.text_weak, "No sequence encoded yet.");
        return;
    }
    let dash = |v: Option<f64>, unit: &str| match v {
        Some(x) => format!("{x:.3}{unit}"),
        // A dash rather than a zero: the distinction between "no frames of that
        // kind" and "those frames were free" is the whole point.
        None => "—".into(),
    };
    egui::Grid::new("seq_lab").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "Frames");
        ui.colored_label(c.text, run.len().to_string());
        ui.end_row();
        ui.colored_label(c.text_weak, "Mean K bits");
        ui.colored_label(c.text, dash(run.mean_bits(true), ""));
        ui.end_row();
        ui.colored_label(c.text_weak, "Mean T bits");
        ui.colored_label(c.text, dash(run.mean_bits(false), ""));
        ui.end_row();
        ui.colored_label(c.text_weak, "T/K ratio");
        ui.colored_label(c.text, dash(run.temporal_ratio(), ""));
        ui.end_row();
        ui.colored_label(c.text_weak, "Mean PSNR");
        ui.colored_label(c.text, dash(run.mean_psnr(), " dB"));
        ui.end_row();
        ui.colored_label(c.text_weak, "Mean skip");
        ui.colored_label(c.text, dash(run.mean_skip(), "%"));
        ui.end_row();
    });

    let costly = run.expensive_temporal(0.8);
    if !costly.is_empty() {
        // Named, not averaged away: a T-frame costing what a K-frame costs is
        // motion estimation failing on that content.
        ui.colored_label(c.warning,
            format!("⚠  {} temporal frame(s) cost near keyframe size: {:?}",
                    costly.len(), costly));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(bits: u64) -> FrameStat {
        FrameStat { frame_type: 0, bits, psnr: 38.0, skip_pct: 0.0, encode_ms: 4.0 }
    }
    fn t(bits: u64) -> FrameStat {
        FrameStat { frame_type: 1, bits, psnr: 37.0, skip_pct: 60.0, encode_ms: 2.0 }
    }

    fn run() -> SeqRun {
        let mut r = SeqRun::new(320, 240, 0.7);
        r.push(k(100_000));
        r.push(t(9_000));
        r.push(t(11_000));
        r
    }

    #[test]
    fn the_split_between_frame_kinds_is_kept_not_averaged() {
        // A K-frame costing ten times a T-frame is the codec working; a T-frame
        // costing as much as a K-frame is motion estimation failing, and one
        // average hides both.
        let r = run();
        assert_eq!(r.mean_bits(true), Some(100_000.0));
        assert_eq!(r.mean_bits(false), Some(10_000.0));
    }

    #[test]
    fn the_temporal_ratio_is_the_number_the_page_exists_for() {
        let r = run();
        let ratio = r.temporal_ratio().expect("both kinds present");
        assert!((ratio - 0.1).abs() < 1e-9, "got {ratio}");
    }

    #[test]
    fn a_run_with_no_temporal_frames_reports_no_ratio_rather_than_zero() {
        // Zero would read as "temporal coding is free" — the most flattering
        // possible misreading of a run that never tested it.
        let mut r = SeqRun::new(320, 240, 0.7);
        r.push(k(100_000));
        assert_eq!(r.mean_bits(false), None);
        assert_eq!(r.temporal_ratio(), None);
    }

    #[test]
    fn an_all_temporal_run_reports_no_ratio_either() {
        // A ratio against nothing is not a ratio.
        let mut r = SeqRun::new(320, 240, 0.7);
        r.push(t(9_000));
        assert_eq!(r.temporal_ratio(), None);
    }

    #[test]
    fn bits_are_convertible_to_bits_per_pixel() {
        // Bits alone cannot be compared between a 320x240 test pattern and a
        // 1080p clip, and someone reading a lab will make that comparison if
        // the page lets them.
        let f = k(76_800 * 2);
        assert!((f.bpp(320, 240) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_zero_sized_frame_does_not_divide_by_zero() {
        assert_eq!(k(1000).bpp(0, 0), 0.0);
    }

    #[test]
    fn a_perfect_reconstruction_does_not_destroy_the_mean_psnr() {
        // The codec reports infinity for zero error, and averaging that in
        // makes the whole report infinite.
        let mut r = SeqRun::new(320, 240, 1.0);
        r.push(FrameStat { psnr: f64::INFINITY, ..k(100_000) });
        r.push(t(9_000));
        let mean = r.mean_psnr().expect("the finite frame still counts");
        assert!(mean.is_finite());
        assert_eq!(r.lossless_frames(), 1, "and it is counted, not dropped silently");
    }

    #[test]
    fn an_all_lossless_run_reports_no_mean_rather_than_infinity() {
        let mut r = SeqRun::new(320, 240, 1.0);
        r.push(FrameStat { psnr: f64::INFINITY, ..k(100_000) });
        assert_eq!(r.mean_psnr(), None);
        assert_eq!(r.lossless_frames(), 1);
    }

    #[test]
    fn a_temporal_frame_costing_keyframe_bits_is_named() {
        // The single most useful thing a sequence lab can point at, and an
        // average would bury it.
        let mut r = SeqRun::new(320, 240, 0.7);
        r.push(k(100_000));
        r.push(t(9_000));
        r.push(t(95_000));          // motion estimation failed here
        assert_eq!(r.expensive_temporal(0.8), vec![2]);
    }

    #[test]
    fn a_healthy_run_names_nothing() {
        assert!(run().expensive_temporal(0.8).is_empty());
    }

    #[test]
    fn keyframes_are_never_reported_as_expensive_temporal_frames() {
        // They are supposed to cost that much; flagging them would make the
        // warning meaningless and train the reader to ignore it.
        let mut r = SeqRun::new(320, 240, 0.7);
        r.push(k(100_000));
        r.push(k(100_000));
        r.push(t(9_000));
        assert!(r.expensive_temporal(0.8).is_empty());
    }

    #[test]
    fn skip_percentage_is_kept_because_bits_alone_cannot_explain_a_cheap_frame() {
        // High skip with high PSNR is a static scene coded well; high skip with
        // low PSNR is the encoder giving up. Identical bit counts, opposite
        // fixes.
        let mut good = SeqRun::new(320, 240, 0.7);
        good.push(FrameStat { skip_pct: 90.0, psnr: 41.0, ..t(2_000) });
        let mut bad = SeqRun::new(320, 240, 0.2);
        bad.push(FrameStat { skip_pct: 90.0, psnr: 22.0, ..t(2_000) });
        assert_eq!(good.total_bits(), bad.total_bits());
        assert_eq!(good.mean_skip(), bad.mean_skip());
        assert!(good.mean_psnr() > bad.mean_psnr(), "only PSNR separates them");
    }

    #[test]
    fn an_empty_run_reports_nothing_rather_than_zeroes() {
        let r = SeqRun::new(320, 240, 0.7);
        assert!(r.is_empty());
        assert_eq!(r.mean_psnr(), None);
        assert_eq!(r.mean_skip(), None);
        assert_eq!(r.total_bits(), 0);
        assert!(r.expensive_temporal(0.8).is_empty());
    }

    #[test]
    fn totals_come_from_the_frames_unmodified() {
        // The lab measures and reports; it does not retune or smooth. A
        // regression in the codec has to show up here rather than being
        // absorbed by the page displaying it.
        let r = run();
        assert_eq!(r.total_bits(), 120_000);
        assert!((r.total_ms() - 8.0).abs() < 1e-9);
    }
}

// ── driving the real encoder ────────────────────────────────────────────────

/// Encode a synthetic moving sequence with the REAL `V2Encoder`.
///
/// The lab exists to measure the codec, so it calls it rather than a model of
/// it — the same choice `lab.rs` made for the i-frame path. What is synthetic
/// is the CONTENT, not the encoder: a generated pan gives a known, repeatable
/// amount of motion, which is what makes two runs comparable at all. Real video
/// would make every run a different question.
pub fn run_sequence(w: usize, h: usize, quality: f32, frames: usize) -> SeqRun {
    let mut run = SeqRun::new(w, h, quality);
    let mut enc = fao_codec_lib::v2_encoder::V2Encoder::new(w, h, quality);
    let mut out = vec![0u8; w * h * 3];

    for i in 0..frames {
        let rgb = pan_frame(w, h, i);
        let r = enc.encode_frame(&rgb, &mut out);
        run.push(FrameStat {
            frame_type: r.frame_type,
            bits: r.bits,
            psnr: r.psnr,
            skip_pct: r.skip_pct,
            encode_ms: r.encode_ms,
        });
    }
    run
}

/// A test pattern that translates by one pixel per frame.
///
/// A pan rather than noise, deliberately: noise is incompressible and would
/// make every frame look like motion estimation failing, which is exactly the
/// signal this page is trying to surface. A pan is the case a temporal codec
/// should handle well, so a bad T/K ratio here means something.
fn pan_frame(w: usize, h: usize, frame: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let sx = x + frame;
            // Diagonal bars plus a soft gradient: enough structure for motion
            // search to lock onto, enough smooth area for the DCT to be worth
            // anything.
            let bar = if ((sx / 16) + (y / 16)) % 2 == 0 { 200 } else { 40 };
            let grad = ((y * 255) / h.max(1)) as u8;
            let i = (y * w + x) * 3;
            v[i] = bar;
            v[i + 1] = grad;
            v[i + 2] = 255u8.saturating_sub(grad);
        }
    }
    v
}

#[cfg(test)]
mod real_encoder_tests {
    use super::*;

    #[test]
    fn the_real_encoder_produces_a_keyframe_then_temporal_frames() {
        // Driving the codec rather than a model of it: the point of a lab.
        let run = run_sequence(64, 64, 0.7, 4);
        assert_eq!(run.len(), 4);
        assert!(run.frames()[0].is_keyframe(), "the first frame has no history");
        assert!(run.frames()[1..].iter().any(|f| !f.is_keyframe()),
                "and later frames should be able to reference it");
    }

    #[test]
    fn a_pan_compresses_better_temporally_than_a_keyframe() {
        // The claim a temporal codec makes. A pan is the case it should handle
        // well — noise would make every frame look like motion estimation
        // failing, which is the signal this page exists to surface.
        let run = run_sequence(64, 64, 0.7, 6);
        if let Some(ratio) = run.temporal_ratio() {
            assert!(ratio < 1.0, "T frames cost {ratio} of a K frame");
        }
    }

    #[test]
    fn the_report_survives_a_single_frame_run() {
        // One frame is all keyframe: no ratio, and that must not panic or read
        // as zero.
        let run = run_sequence(64, 64, 0.7, 1);
        assert_eq!(run.len(), 1);
        assert_eq!(run.temporal_ratio(), None);
        assert!(run.mean_bits(true).is_some());
    }

    #[test]
    fn a_zero_frame_run_is_empty_rather_than_a_panic() {
        let run = run_sequence(64, 64, 0.7, 0);
        assert!(run.is_empty());
        assert_eq!(run.total_bits(), 0);
    }
}
