//! krapper-egui — the writing surface, ported from the iced app.
//!
//! krapper had **no egui port at all** and the parity audit never noticed,
//! because it only looks for an `egui-frontend/` directory and krapper has one
//! `src/` with `main.rs` calling `iced::application`. It is the one real
//! application in the fleet with no egui build; the other apparent gaps
//! (`coordd`, `settingsd`, `sharemd`, `voiced`) are daemons and correctly
//! headless.
//!
//! ## What is shared and what is new
//!
//! `vibe.rs`, `scale.rs`, `vocab.rs` and `dict.rs` are **iced-free** — the
//! toolkit only reaches `app.rs`. So the model is mounted by `#[path]` and this
//! crate is only the surface, the same arrangement rusty-board's config and
//! voiceterm's niftyplot use.
//!
//! ## The surface that matters
//!
//! kRapper's premise is that you hum a vibe and write over it, so the writing
//! surface has to show the **stress pattern** — which syllables land hard —
//! against what you are typing. Everything else is support:
//!
//! * the **stress ruler**, one cell per syllable in the bar, the stressed ones
//!   marked;
//! * a **hot-mic level** that must read as armed-but-silent distinctly from
//!   not-armed, because a writer glancing at it needs to know whether a take is
//!   being captured;
//! * **blurb suggestions** filtered to those that fit the remaining syllables,
//!   since a suggestion that does not fit the bar is noise.

#[path = "../../src/vibe.rs"]
#[allow(dead_code)]
pub mod vibe;

#[path = "../../src/scale.rs"]
#[allow(dead_code)]
pub mod scale;

pub use vibe::{render_stress_pattern, VibeSummary};

/// What the mic is doing. Three states, not a bool: a writer glancing at the
/// meter needs to tell "armed and silent" from "not listening", and one
/// boolean cannot say that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mic {
    #[default]
    Off,
    /// Listening, nothing above the floor.
    Armed,
    Recording,
}

/// Below this, a level is silence rather than a quiet take. Without a floor the
/// meter twitches on room noise and reads as "recording something" when it is
/// recording nothing.
pub const SILENCE: f32 = 0.02;

/// The writing surface's state.
#[derive(Debug, Clone)]
pub struct Writer {
    pub vibe: VibeSummary,
    /// The hummed pitch, in the app's own note terms. `scale.rs` was mounted and
    /// never read — which for kRapper is the whole premise going missing: you
    /// hum a vibe and write over it, and a writer who cannot see what key they
    /// hummed in is writing over nothing.
    pub pitch_hz: Option<f32>,
    pub lyrics: String,
    pub mic: Mic,
    /// Smoothed input level, 0..1.
    pub level: f32,
    /// Suggestions, longest-fitting first.
    pub blurbs: Vec<String>,
    pub status: String,
}

impl Default for Writer {
    fn default() -> Self {
        Writer {
            vibe: VibeSummary::default(), pitch_hz: None,
            lyrics: String::new(), mic: Mic::Off,
            level: 0.0, blurbs: Vec::new(), status: "ready".into(),
        }
    }
}

/// Rough syllable count for a line.
///
/// Vowel groups, not vowels: "beat" is one syllable and "bea-t" is not a thing.
/// A trailing silent `e` is dropped, which is wrong for "the" and right far more
/// often — and the writer can see the ruler, so an approximate count that is
/// close beats an exact one that needs a dictionary at every keystroke.
/// Must match packaging/org.krapper.KRapper.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.krapper.KRapper";

pub fn syllables(line: &str) -> usize {
    let l = line.trim().to_lowercase();
    if l.is_empty() { return 0; }
    let mut n = 0;
    let mut in_vowel = false;
    for c in l.chars() {
        let v = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
        if v && !in_vowel { n += 1; }
        in_vowel = v;
    }
    if l.ends_with('e') && n > 1 { n -= 1; }
    n.max(1)
}

impl Writer {
    /// Syllables in the line being written.
    pub fn line_syllables(&self) -> usize {
        self.lyrics.lines().last().map(syllables).unwrap_or(0)
    }

    /// How many syllables are left in the bar. Negative when the line has run
    /// over, which is a normal thing to do on purpose — so it is reported as a
    /// signed count rather than clamped to zero and hidden.
    pub fn remaining(&self) -> i32 {
        self.vibe.syllables_per_bar as i32 - self.line_syllables() as i32
    }

    /// Suggestions that fit what is left.
    ///
    /// A suggestion longer than the remaining bar is noise — the writer would
    /// have to delete something to use it, which is not a suggestion.
    pub fn fitting_blurbs(&self) -> Vec<&str> {
        let left = self.remaining();
        if left <= 0 { return Vec::new(); }
        let mut v: Vec<&str> = self.blurbs.iter().map(|s| s.as_str())
            .filter(|b| syllables(b) as i32 <= left).collect();
        // longest first: the writer wants the biggest thing that still fits
        v.sort_by_key(|b| std::cmp::Reverse(syllables(b)));
        v
    }

    /// The mic state implied by arming and level. A level below the floor while
    /// armed is `Armed`, not `Recording` — room noise must not read as a take.
    pub fn observe(&mut self, armed: bool, level: f32) {
        self.level = level.clamp(0.0, 1.0);
        self.mic = if !armed { Mic::Off }
                   else if self.level > SILENCE { Mic::Recording }
                   else { Mic::Armed };
    }

    /// The hummed note, e.g. "A♯3" — or `None` when nothing is being sung.
    ///
    /// Uses the app's own `scale::freq_to_note`, so the note the writing surface
    /// shows is the note the recogniser heard rather than a second opinion.
    pub fn note(&self) -> Option<String> {
        let hz = self.pitch_hz?;
        if hz <= 0.0 { return None; }
        let (name, octave, cents) = crate::scale::freq_to_note(hz);
        Some(if cents.abs() >= 25 { format!("{name}{octave} {cents:+}¢") }
             else { format!("{name}{octave}") })
    }

    /// The stress cell for a syllable index, repeating the pattern to cover the
    /// bar as the model documents.
    pub fn stress_at(&self, i: usize) -> u8 {
        let p = &self.vibe.stress_pattern;
        if p.is_empty() { 0 } else { p[i % p.len()] }
    }
}

/// Draw the surface. Returns a blurb if one was clicked.
pub fn writer_ui(ui: &mut egui::Ui, w: &mut Writer,
                 colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{:.0} bpm", w.vibe.bpm)).strong());
        ui.label(egui::RichText::new(format!("{}/bar", w.vibe.syllables_per_bar)).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (txt, col) = match w.mic {
                Mic::Off => ("mic off", colors.text_weak),
                Mic::Armed => ("armed", colors.warning),
                Mic::Recording => ("REC", colors.accent),
            };
            ui.label(egui::RichText::new(txt).color(col).small());
            let (r, _) = ui.allocate_exact_size(egui::Vec2::new(60.0, 8.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, colors.bg_card);
            let mut f = r;
            f.set_width(r.width() * w.level);
            ui.painter().rect_filled(f, 2.0, col);
        });
    });
    // the stress ruler: one cell per syllable, stressed ones filled
    ui.horizontal(|ui| {
        for i in 0..w.vibe.syllables_per_bar as usize {
            let on = w.stress_at(i) == 1;
            let used = i < w.line_syllables();
            let (r, _) = ui.allocate_exact_size(egui::Vec2::new(14.0, 16.0), egui::Sense::hover());
            let c = if !used { colors.bg_card }
                    else if on { colors.accent } else { colors.text_weak };
            ui.painter().rect_filled(r, 2.0, c);
        }
        let left = w.remaining();
        ui.label(egui::RichText::new(if left < 0 { format!("{} over", -left) }
                                     else { format!("{left} left") })
            .color(if left < 0 { colors.warning } else { colors.text_weak }).small());
    });
    ui.separator();
    ui.add(egui::TextEdit::multiline(&mut w.lyrics).desired_rows(8).desired_width(f32::INFINITY));
    ui.separator();
    let fits = w.fitting_blurbs().into_iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if fits.is_empty() {
        ui.label(egui::RichText::new(if w.remaining() <= 0 { "bar is full" }
                                     else { "no suggestions fit" }).weak().small());
    } else {
        ui.horizontal_wrapped(|ui| {
            for b in fits {
                if ui.button(&b).clicked() { picked = Some(b.clone()); }
            }
        });
    }
    picked
}

#[cfg(feature = "app")]
pub fn run(title: &str) -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "kRapper", [760.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(title, opts, Box::new(|_cc| Ok(Box::<Shell>::default())))
}

#[cfg(feature = "app")]
#[derive(Default)]
struct Shell { w: Writer }

#[cfg(feature = "app")]
impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        let colors = uikit_egui::palette_colors("midnight");
        egui::CentralPanel::default().show(ui.ctx(), |ui| {
            if let Some(b) = writer_ui(ui, &mut self.w, &colors) {
                if !self.w.lyrics.is_empty() && !self.w.lyrics.ends_with(' ') {
                    self.w.lyrics.push(' ');
                }
                self.w.lyrics.push_str(&b);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w8() -> Writer {
        Writer { blurbs: vec!["run it back".into(), "yeah".into(),
                              "all night every single night".into(), "go".into()],
                 ..Default::default() }
    }

    #[test]
    fn syllables_counts_vowel_groups_not_vowels() {
        // "beat" is one syllable; counting vowels gives two.
        assert_eq!(syllables("beat"), 1);
        assert_eq!(syllables("running"), 2);
        assert_eq!(syllables("yeah"), 1);
        assert_eq!(syllables("all night"), 2);
        // a trailing silent e is dropped
        assert_eq!(syllables("time"), 1);
        // but never to zero — every word is at least one
        assert_eq!(syllables("the"), 1);
        assert_eq!(syllables("e"), 1);
        assert_eq!(syllables(""), 0);
        assert_eq!(syllables("   "), 0);
    }

    #[test]
    fn the_mic_distinguishes_armed_from_recording() {
        // A writer glancing at the meter needs to know whether a take is being
        // captured. One boolean cannot say that, and room noise must not read
        // as a take.
        let mut w = w8();
        w.observe(false, 0.0);
        assert_eq!(w.mic, Mic::Off);
        w.observe(true, 0.001);
        assert_eq!(w.mic, Mic::Armed, "armed and silent is not recording");
        w.observe(true, 0.5);
        assert_eq!(w.mic, Mic::Recording);
        // and disarming while loud still reads off
        w.observe(false, 0.9);
        assert_eq!(w.mic, Mic::Off);
    }

    #[test]
    fn running_over_the_bar_is_reported_rather_than_hidden() {
        // Writing past the bar is a normal thing to do on purpose, so the count
        // is signed rather than clamped to zero.
        let mut w = w8();
        w.vibe.syllables_per_bar = 4;
        // "running all night" is exactly four (run-ning all night), so it fills
        // the bar rather than overrunning it — the first version of this test
        // asserted it was over and the COUNTER was right.
        w.lyrics = "running all night".into();
        assert_eq!(w.remaining(), 0, "exactly full");
        w.lyrics = "running all night every single time".into();
        assert!(w.remaining() < 0, "remaining {}", w.remaining());
    }

    #[test]
    fn suggestions_that_do_not_fit_are_not_offered() {
        // A suggestion longer than the remaining bar is noise: the writer would
        // have to delete something to use it.
        let mut w = w8();
        w.vibe.syllables_per_bar = 8;
        w.lyrics = "running all".into();          // 3 used, 5 left
        let fits = w.fitting_blurbs();
        assert!(fits.iter().all(|b| syllables(b) <= 5), "{fits:?}");
        assert!(!fits.contains(&"all night every single night"), "too long");
        // longest first: the writer wants the biggest thing that still fits
        for pair in fits.windows(2) {
            assert!(syllables(pair[0]) >= syllables(pair[1]), "{fits:?}");
        }
    }

    #[test]
    fn a_full_bar_offers_nothing_rather_than_something_that_will_not_fit() {
        let mut w = w8();
        w.vibe.syllables_per_bar = 2;
        w.lyrics = "running all night".into();
        assert!(w.fitting_blurbs().is_empty());
    }

    #[test]
    fn the_stress_pattern_repeats_to_cover_the_bar() {
        // The model documents that a shorter pattern is repeated, so the ruler
        // must not run off the end of it.
        let mut w = w8();
        w.vibe.stress_pattern = vec![1, 0, 0];
        w.vibe.syllables_per_bar = 8;
        assert_eq!(w.stress_at(0), 1);
        assert_eq!(w.stress_at(3), 1, "the pattern wraps");
        assert_eq!(w.stress_at(7), 0);
        // an empty pattern is unstressed rather than a panic
        w.vibe.stress_pattern.clear();
        assert_eq!(w.stress_at(5), 0);
    }

    #[test]
    fn the_hummed_note_is_reported_in_the_apps_own_terms() {
        // A writer who cannot see what key they hummed in is writing over
        // nothing, which is the whole premise going missing.
        let mut w = w8();
        assert_eq!(w.note(), None, "nothing hummed yet");
        w.pitch_hz = Some(440.0);
        assert_eq!(w.note().as_deref(), Some("A4"), "concert A");
        // a badly-out-of-tune hum reports the cents, so the writer can hear
        // that they are between notes rather than on one
        w.pitch_hz = Some(453.0);
        assert!(w.note().unwrap().contains("¢"), "{:?}", w.note());
        // silence and nonsense do not produce a note
        w.pitch_hz = Some(0.0);
        assert_eq!(w.note(), None);
    }

    #[test]
    fn the_shared_model_is_the_apps_own() {
        // Mounted by #[path], so a change to the vibe model reaches both
        // frontends rather than drifting.
        let v = VibeSummary::default();
        assert_eq!(v.syllables_per_bar as usize, v.stress_pattern.len());
        assert!((v.bps() - v.bpm / 60.0).abs() < 1e-6);
        assert_eq!(render_stress_pattern(&[1, 0, 1]).len(), 3);
    }
}
