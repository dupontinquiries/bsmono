//! device — product capability profiles and the EQ mixer.
//!
//! `volume_knobs.qml` is 2,323 lines and `MixerPage.qml` another 500. Both are
//! per-device: what a Beosound can do is not what a Beoplay can, and the QML
//! drives that from a **product map** rather than branching on a model name.
//!
//! ## `hasControl(n)` is the whole design
//!
//! A profile lists the controls its product has. Anything not listed is hidden
//! — and crucially, an **unknown product shows everything**, because the QML's
//! fallback is `c ? c.indexOf(n) !== -1 : true`. Hiding controls on an
//! unrecognised device would make a new speaker look broken; showing a control
//! that turns out to do nothing is the lesser failure.
//!
//! ## The formatters are exact
//!
//! `hzLabel`, `dbLabel` and `volLabel` are ported to the digit, because a mixer
//! whose labels differ from the one people already use reads as a different
//! product. `4 kHz` not `4.0 kHz`; `0 dB` not `+0.0 dB`.

/// What a product can do.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub emoji: String,
    pub accent: String,
    pub accent_soft: String,
    pub shape: String,
    pub tagline: String,
    /// Controls this product has. **Empty means unknown, not none** — see
    /// `has_control`.
    pub controls: Vec<String>,
}

impl Default for Profile {
    /// The QML's `"Unknown Type"` fallback, verbatim.
    fn default() -> Self {
        Profile {
            name: "Bang & Olufsen".into(), emoji: "🎵".into(),
            accent: "#C8A04B".into(), accent_soft: "#33291a".into(),
            shape: "disc".into(), tagline: "Audio device".into(),
            controls: ["volume", "mute", "stream", "sources", "autoroute"]
                .iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Profile {
    /// `hasControl(n)`.
    ///
    /// An **empty** control list means unknown, so everything shows: hiding
    /// controls on an unrecognised device makes a new speaker look broken, while
    /// showing one that turns out to do nothing is the lesser failure.
    pub fn has_control(&self, name: &str) -> bool {
        self.controls.is_empty() || self.controls.iter().any(|c| c == name)
    }
}

/// Which theme to use. `resolvedMode()` in the QML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Follow the system.
    #[default]
    Auto,
    Light,
    Dark,
}

impl ThemeMode {
    /// Resolve against the system preference. Auto is not a third appearance —
    /// it is a deferral, and a UI that renders "auto" as its own look is wrong
    /// in a way nobody can describe.
    pub fn resolve(self, system_dark: bool) -> bool {
        match self { ThemeMode::Auto => system_dark, ThemeMode::Dark => true, ThemeMode::Light => false }
    }
    pub fn tag(self) -> &'static str {
        match self { ThemeMode::Auto => "auto", ThemeMode::Light => "light", ThemeMode::Dark => "dark" }
    }
}

// ── formatters, ported exactly ──────────────────────────────────────────────

/// `hzLabel` — "250 Hz" or "4 kHz".
///
/// A whole number of kHz drops the decimal: `4 kHz`, not `4.0 kHz`. That is the
/// QML's `toFixed(hz % 1000 === 0 ? 0 : 1)` and it is the difference between a
/// label that reads like the product and one that reads like a debug print.
pub fn hz_label(hz: f64) -> String {
    if hz == 0.0 { return String::new(); }
    if hz >= 1000.0 {
        if hz % 1000.0 == 0.0 { format!("{} kHz", (hz / 1000.0) as i64) }
        else { format!("{:.1} kHz", hz / 1000.0) }
    } else {
        format!("{} Hz", hz.round() as i64)
    }
}

/// `dbLabel` — signed, one decimal, and a bare "0 dB" at unity.
///
/// The deadband matters: without it a slider resting at 0.04 reads "+0.0 dB",
/// which claims a boost that is not there.
pub fn db_label(db: f64) -> String {
    if db.abs() < 0.1 { return "0 dB".into(); }
    if db > 0.0 { format!("+{db:.1} dB") } else { format!("{db:.1} dB") }
}

/// `volLabel` — a linear 0..1 as a percentage.
pub fn vol_label(v: f64) -> String { format!("{}%", (v.clamp(0.0, 1.0) * 100.0).round() as i64) }

// ── mixer ───────────────────────────────────────────────────────────────────

/// One EQ band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub center_hz: f64,
    pub gain_db: f64,
}

/// The EQ, plus the two source levels the page mixes between.
#[derive(Debug, Clone, Default)]
pub struct Mixer {
    pub bands: Vec<Band>,
    /// 0..1, the speaker/instrumental balance the page exists to set.
    pub speaker: f64,
    pub instrumental: f64,
}

/// How close to neutral a band snaps to exactly 0 dB, from `BandSlider.qml`.
///
/// A detent that "makes neutral easy to hit by feel". Without it, a slider left
/// at 0.1 dB looks flat, reads as `0 dB` through the deadband in `db_label`, and
/// is not flat — so `is_flat()` says the EQ is doing something while every label
/// says it is not.
pub const DETENT_DB: f64 = 0.4;

/// Apply the detent.
///
/// `is_volume` gets NO detent, and the QML says why: "the user often wants
/// subtle gain changes". A volume that refuses to sit at 0.3 is a volume that
/// cannot be nudged, which is most of what a volume is for.
pub fn detent(value_db: f64, is_volume: bool) -> f64 {
    if !is_volume && value_db.abs() < DETENT_DB { 0.0 } else { value_db }
}

/// The furthest a band may be pushed. Beyond about 12 dB an EQ stops shaping
/// and starts clipping, and a slider that allows it invites exactly that.
pub const MAX_GAIN_DB: f64 = 12.0;

impl Mixer {
    pub fn set_gain(&mut self, i: usize, db: f64) -> bool {
        match self.bands.get_mut(i) {
            // Detent BEFORE clamp: snapping a value that was already clamped
            // would be a no-op at the rails, and the detent only ever applies
            // near zero anyway — but the order is worth fixing so a future
            // change to either cannot silently reorder them.
            Some(b) => { b.gain_db = detent(db, false).clamp(-MAX_GAIN_DB, MAX_GAIN_DB); true }
            None => false,
        }
    }

    /// Is the EQ doing anything? Used to show a "flat" state, because a row of
    /// sliders all at zero looks the same as a broken EQ.
    pub fn is_flat(&self) -> bool { self.bands.iter().all(|b| b.gain_db.abs() < 0.1) }

    pub fn reset(&mut self) { for b in self.bands.iter_mut() { b.gain_db = 0.0; } }

    /// Bands ordered low to high, as a graphic EQ is always drawn. The backend
    /// sends a map, and map order is not frequency order.
    pub fn ordered(&self) -> Vec<(usize, Band)> {
        let mut v: Vec<(usize, Band)> = self.bands.iter().copied().enumerate().collect();
        v.sort_by(|a, b| a.1.center_hz.partial_cmp(&b.1.center_hz).unwrap());
        v
    }

    /// The loudest boost, for a headroom warning. Cuts do not clip, so only
    /// positive gain counts.
    pub fn peak_boost_db(&self) -> f64 {
        self.bands.iter().map(|b| b.gain_db).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixer() -> Mixer {
        Mixer { bands: vec![Band { center_hz: 4000.0, gain_db: 0.0 },
                            Band { center_hz: 250.0, gain_db: 0.0 },
                            Band { center_hz: 1000.0, gain_db: 0.0 }],
                speaker: 0.5, instrumental: 0.5 }
    }

    #[test]
    fn an_unknown_product_shows_every_control() {
        // Hiding controls on an unrecognised device makes a new speaker look
        // broken; showing one that does nothing is the lesser failure. The QML's
        // fallback is `c ? c.indexOf(n) !== -1 : true`.
        let unknown = Profile { controls: vec![], ..Default::default() };
        assert!(unknown.has_control("volume"));
        assert!(unknown.has_control("anything-at-all"));
        // a product that DOES list its controls hides the rest
        let limited = Profile { controls: vec!["volume".into()], ..Default::default() };
        assert!(limited.has_control("volume"));
        assert!(!limited.has_control("autoroute"));
    }

    #[test]
    fn the_default_profile_is_the_qmls_fallback() {
        // A port that invents its own fallback shows a different device name on
        // an unrecognised speaker, which reads as the port being wrong.
        let p = Profile::default();
        assert_eq!(p.name, "Bang & Olufsen");
        assert_eq!(p.accent, "#C8A04B");
        assert_eq!(p.shape, "disc");
        assert_eq!(p.controls.len(), 5);
        assert!(p.has_control("autoroute"));
    }

    #[test]
    fn auto_is_a_deferral_not_a_third_appearance() {
        // A UI that renders "auto" as its own look is wrong in a way nobody can
        // describe.
        assert!(ThemeMode::Auto.resolve(true));
        assert!(!ThemeMode::Auto.resolve(false));
        assert!(ThemeMode::Dark.resolve(false), "explicit beats the system");
        assert!(!ThemeMode::Light.resolve(true));
    }

    #[test]
    fn hz_labels_drop_the_decimal_on_whole_kilohertz() {
        // "4 kHz", not "4.0 kHz" — the difference between a product label and a
        // debug print.
        assert_eq!(hz_label(250.0), "250 Hz");
        assert_eq!(hz_label(4000.0), "4 kHz");
        assert_eq!(hz_label(4500.0), "4.5 kHz");
        assert_eq!(hz_label(1000.0), "1 kHz");
        assert_eq!(hz_label(0.0), "", "zero is not a band");
    }

    #[test]
    fn a_slider_resting_near_zero_reads_as_zero() {
        // Without the deadband a slider at 0.04 reads "+0.0 dB", claiming a
        // boost that is not there.
        assert_eq!(db_label(0.0), "0 dB");
        assert_eq!(db_label(0.04), "0 dB");
        assert_eq!(db_label(-0.04), "0 dB");
        assert_eq!(db_label(3.0), "+3.0 dB");
        assert_eq!(db_label(-2.5), "-2.5 dB");
    }

    #[test]
    fn volume_reads_as_a_percentage_and_cannot_exceed_it() {
        assert_eq!(vol_label(0.0), "0%");
        assert_eq!(vol_label(0.5), "50%");
        assert_eq!(vol_label(1.0), "100%");
        assert_eq!(vol_label(1.5), "100%", "clamped, not 150%");
        assert_eq!(vol_label(-1.0), "0%");
    }

    #[test]
    fn bands_are_drawn_low_to_high_whatever_order_they_arrive_in() {
        // The backend sends a map, and map order is not frequency order.
        let m = mixer();
        let hz: Vec<f64> = m.ordered().iter().map(|(_, b)| b.center_hz).collect();
        assert_eq!(hz, vec![250.0, 1000.0, 4000.0]);
        // and the original indices come with them, so a drag edits the right band
        assert_eq!(m.ordered()[0].0, 1);
    }

    #[test]
    fn gain_is_clamped_because_a_slider_that_allows_clipping_invites_it() {
        let mut m = mixer();
        assert!(m.set_gain(0, 99.0));
        assert_eq!(m.bands[0].gain_db, MAX_GAIN_DB);
        m.set_gain(0, -99.0);
        assert_eq!(m.bands[0].gain_db, -MAX_GAIN_DB);
        assert!(!m.set_gain(99, 3.0), "an unknown band is refused, not created");
    }

    #[test]
    fn a_band_snaps_to_neutral_by_feel() {
        // The detent from BandSlider.qml. Without it a band left at 0.1 dB
        // LOOKS flat, READS as "0 dB" through db_label's deadband, and is not —
        // so is_flat() disagrees with every label on screen.
        let mut m = mixer();
        m.set_gain(0, 0.3);
        assert_eq!(m.bands[0].gain_db, 0.0, "snapped");
        assert!(m.is_flat(), "and the EQ agrees with its own labels");
        m.set_gain(0, 0.5);
        assert_eq!(m.bands[0].gain_db, 0.5, "outside the detent, untouched");
        m.set_gain(0, -0.39);
        assert_eq!(m.bands[0].gain_db, 0.0, "symmetric about neutral");
    }

    #[test]
    fn volume_has_no_detent_because_subtle_gain_changes_are_the_point() {
        // The QML's own reason. A volume that refuses to sit at 0.3 cannot be
        // nudged, which is most of what a volume is for.
        assert_eq!(detent(0.3, true), 0.3);
        assert_eq!(detent(0.3, false), 0.0);
        assert_eq!(detent(0.0, true), 0.0);
    }

    #[test]
    fn a_flat_eq_is_reported_so_it_does_not_look_broken() {
        // A row of sliders all at zero looks the same as an EQ that is not
        // working.
        let mut m = mixer();
        assert!(m.is_flat());
        m.set_gain(0, 3.0);
        assert!(!m.is_flat());
        m.reset();
        assert!(m.is_flat());
    }

    #[test]
    fn only_boosts_count_toward_headroom() {
        // Cuts do not clip.
        let mut m = mixer();
        m.set_gain(0, -6.0);
        assert_eq!(m.peak_boost_db(), 0.0);
        m.set_gain(1, 4.0);
        assert_eq!(m.peak_boost_db(), 4.0);
    }
}
