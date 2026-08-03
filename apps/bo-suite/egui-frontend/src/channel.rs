//! channel — `ChannelColumn` and `FreqResponseCurve`.
//!
//! The stream mixer has two channels — SPEAKER and INSTRUMENTAL — and the point
//! of both surfaces is to make their INDEPENDENCE visible. Turn up speaker mid
//! and the speaker curve lifts around 1.5kHz and nothing else moves. Turn up
//! instrumental low and the other curve rises around 200Hz, separately. If the
//! two curves overlap in a way that confuses, that is itself the diagnostic:
//! it tells the user to reach for a different slider.
//!
//! ## The column shows its frequencies
//!
//! Each band's locked centre frequency is printed above its slider, with a role
//! hint — "vocal presence" under "1.5 kHz". That is the visual proof the bands
//! are not arbitrary: they are chosen for the channel's musical role, and a
//! slider labelled only "MID" gives the user no way to know that.
//!
//! ## Per-channel reset, separate from reset-all
//!
//! The header's ↺ neutralises THIS column only. It is the common need — you
//! adjust one channel, decide it was a bad call, and want to start over
//! WITHOUT losing the work on the other. A single global reset forces the user
//! to choose between undoing a mistake and keeping something good.
//!
//! ## The curve is logarithmic because hearing is
//!
//! 20Hz to 20kHz on a linear axis puts everything below 2kHz in the first tenth
//! of the width — which is most of the music. Log spacing gives each octave
//! equal room, which is what makes "lift around 200Hz" a visible event rather
//! than a wiggle at the left edge.

/// The audible range the curve spans.
pub const FREQ_LO: f64 = 20.0;
pub const FREQ_HI: f64 = 20_000.0;
/// Grid lines, at the decade and half-decade marks people read frequencies in.
pub const GRID_HZ: [f64; 10] = [20.0, 50.0, 100.0, 200.0, 500.0,
                                1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0];
/// EQ range, ±12 dB.
pub const EQ_MAX_DB: f64 = 12.0;
/// Volume range, 0% to 200%.
pub const VOLUME_MAX: f64 = 200.0;
/// The bell width of one EQ band, in octaves — wide enough to be musical, narrow
/// enough that the three bands stay visibly separate.
pub const BAND_Q_OCTAVES: f64 = 1.2;

/// Which channel a column drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel { Speaker, Instrumental }

impl Channel {
    pub fn label(self) -> &'static str {
        match self { Channel::Speaker => "SPEAKER", Channel::Instrumental => "INSTRUMENTAL" }
    }
}

/// One band of a channel: where it sits, what it does, and how far it is moved.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelBand {
    pub center_hz: f64,
    pub gain_db: f64,
    /// "vocal presence" — the reason this frequency was chosen.
    pub role_hint: String,
}

impl ChannelBand {
    /// The label above the slider. kHz above a thousand, because "1500 Hz" is
    /// read as a number and "1.5 kHz" is read as a place.
    pub fn freq_label(&self) -> String {
        if self.center_hz >= 1000.0 {
            let k = self.center_hz / 1000.0;
            if (k - k.round()).abs() < 0.05 { format!("{:.0} kHz", k) } else { format!("{k:.1} kHz") }
        } else {
            format!("{:.0} Hz", self.center_hz)
        }
    }
}

/// One column of the mixer.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelColumn {
    pub channel: Channel,
    pub low: ChannelBand,
    pub mid: ChannelBand,
    pub high: ChannelBand,
    /// Percent, 0..=200. 100 is unity.
    pub volume: f64,
}

impl ChannelColumn {
    pub fn new(channel: Channel, low_hz: f64, mid_hz: f64, high_hz: f64,
               roles: [&str; 3]) -> Self {
        ChannelColumn {
            channel,
            low: ChannelBand { center_hz: low_hz, gain_db: 0.0, role_hint: roles[0].into() },
            mid: ChannelBand { center_hz: mid_hz, gain_db: 0.0, role_hint: roles[1].into() },
            high: ChannelBand { center_hz: high_hz, gain_db: 0.0, role_hint: roles[2].into() },
            volume: 100.0,
        }
    }

    pub fn bands(&self) -> [&ChannelBand; 3] { [&self.low, &self.mid, &self.high] }

    /// Neutralise THIS column only. The other channel is untouched, which is
    /// the whole reason this exists beside the global reset.
    pub fn reset(&mut self) {
        self.low.gain_db = 0.0;
        self.mid.gain_db = 0.0;
        self.high.gain_db = 0.0;
        self.volume = 100.0;
    }

    /// Is this column doing anything?
    pub fn is_neutral(&self) -> bool {
        self.bands().iter().all(|b| b.gain_db.abs() < 0.05)
            && (self.volume - 100.0).abs() < 0.5
    }

    /// Clamp a band gain into range.
    pub fn set_gain(&mut self, i: usize, db: f64) {
        let db = db.clamp(-EQ_MAX_DB, EQ_MAX_DB);
        match i { 0 => self.low.gain_db = db, 1 => self.mid.gain_db = db,
                  _ => self.high.gain_db = db }
    }

    pub fn set_volume(&mut self, pct: f64) { self.volume = pct.clamp(0.0, VOLUME_MAX); }

    /// Volume expressed in dB, which is what the response curve needs.
    ///
    /// Silence is not "minus a lot of dB", it is the absence of signal — so it
    /// is reported as a floor rather than as negative infinity, which would
    /// make the curve disappear off the bottom of the plot instead of sitting
    /// on it.
    pub fn volume_db(&self) -> f64 {
        if self.volume <= 0.0 { return -60.0; }
        20.0 * (self.volume / 100.0).log10()
    }

    /// The channel's response at a frequency, in dB: the summed band bells plus
    /// the channel volume.
    pub fn response_db(&self, hz: f64) -> f64 {
        let mut db = self.volume_db();
        for b in self.bands() {
            db += band_contribution(b.center_hz, b.gain_db, hz);
        }
        db
    }
}

/// One band's contribution at a frequency: a bell in LOG-frequency space.
///
/// Log space is what makes the bell symmetric to the ear — an octave below a
/// centre and an octave above are equally far away musically, though one is a
/// tenth of the Hz distance of the other. A linear bell would spill upward and
/// barely reach downward.
pub fn band_contribution(center_hz: f64, gain_db: f64, hz: f64) -> f64 {
    if gain_db == 0.0 || center_hz <= 0.0 || hz <= 0.0 { return 0.0; }
    let octaves = (hz / center_hz).log2();
    let t = octaves / BAND_Q_OCTAVES;
    gain_db * (-(t * t)).exp()
}

/// Where a frequency sits horizontally, 0..=1 across the plot.
pub fn x_for_hz(hz: f64) -> f64 {
    let (lo, hi) = (FREQ_LO.ln(), FREQ_HI.ln());
    ((hz.max(1e-6).ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Draw one column. Returns the index of a band whose gain changed, or 3 for
/// the volume.
pub fn column_ui(ui: &mut egui::Ui, col: &mut ChannelColumn,
                 c: &uikit_egui::PaletteColors) -> Option<usize> {
    let mut changed = None;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.colored_label(c.text_weak, col.channel.label());
            // Per-channel reset, disabled when there is nothing to undo — an
            // enabled reset on a neutral column offers to undo nothing.
            if ui.add_enabled(!col.is_neutral(), egui::Button::new("↺").frame(false))
                .on_hover_text("Reset this channel only").clicked() {
                col.reset();
                changed = Some(3);
            }
        });
        ui.horizontal(|ui| {
            for i in 0..3 {
                let b = match i { 0 => &col.low, 1 => &col.mid, _ => &col.high };
                let (label, hint) = (b.freq_label(), b.role_hint.clone());
                let mut g = b.gain_db;
                ui.vertical(|ui| {
                    // The frequency and its role, above the slider: the visual
                    // proof the bands are chosen rather than arbitrary.
                    ui.colored_label(c.text_weak, egui::RichText::new(label).small());
                    if !hint.is_empty() {
                        ui.colored_label(c.text_weak, egui::RichText::new(hint).small());
                    }
                    if ui.add(egui::Slider::new(&mut g, -EQ_MAX_DB..=EQ_MAX_DB)
                        .vertical().show_value(false)).changed() {
                        changed = Some(i);
                    }
                });
                if changed == Some(i) { col.set_gain(i, g); }
            }
            let mut v = col.volume;
            ui.vertical(|ui| {
                ui.colored_label(c.text_weak, egui::RichText::new("VOL").small());
                if ui.add(egui::Slider::new(&mut v, 0.0..=VOLUME_MAX)
                    .vertical().show_value(false)).changed() {
                    changed = Some(3);
                }
            });
            if changed == Some(3) { col.set_volume(v); }
        });
    });
    changed
}

/// Draw both response curves over a log frequency axis.
pub fn curve_ui(ui: &mut egui::Ui, speaker: &ChannelColumn, instrumental: &ChannelColumn,
                height: f32, c: &uikit_egui::PaletteColors) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, height), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, c.bg_raised);

    // Grid at the marks people actually read frequencies in.
    for hz in GRID_HZ {
        let x = rect.left() + rect.width() * x_for_hz(hz) as f32;
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                       egui::Stroke::new(1.0, c.border));
    }
    // The 0 dB line, centred — the curve reads as deviation from flat.
    let mid_y = rect.center().y;
    p.line_segment([egui::pos2(rect.left(), mid_y), egui::pos2(rect.right(), mid_y)],
                   egui::Stroke::new(1.0, c.text_weak));

    let plot = |col: &ChannelColumn, colour: egui::Color32| {
        let mut prev: Option<egui::Pos2> = None;
        for i in 0..=96 {
            let t = i as f64 / 96.0;
            let hz = FREQ_LO * (FREQ_HI / FREQ_LO).powf(t);
            let db = col.response_db(hz);
            let x = rect.left() + rect.width() * x_for_hz(hz) as f32;
            let y = mid_y - (db / 24.0) as f32 * rect.height() * 0.5;
            let pt = egui::pos2(x, y.clamp(rect.top(), rect.bottom()));
            if let Some(q) = prev { p.line_segment([q, pt], egui::Stroke::new(2.0, colour)); }
            prev = Some(pt);
        }
    };
    plot(speaker, c.accent);
    plot(instrumental, c.success);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speaker() -> ChannelColumn {
        ChannelColumn::new(Channel::Speaker, 200.0, 1500.0, 8000.0,
                           ["body", "vocal presence", "air"])
    }
    fn instrumental() -> ChannelColumn {
        ChannelColumn::new(Channel::Instrumental, 200.0, 1500.0, 8000.0,
                           ["weight", "midrange", "shimmer"])
    }

    #[test]
    fn a_fresh_column_is_neutral() {
        let c = speaker();
        assert!(c.is_neutral());
        assert_eq!(c.volume, 100.0, "unity, not zero");
    }

    #[test]
    fn resetting_one_column_leaves_the_other_alone() {
        // The whole reason per-channel reset exists beside the global one: you
        // adjust one channel, decide it was a bad call, and want to start over
        // WITHOUT losing the work on the other.
        let mut s = speaker();
        let mut i = instrumental();
        s.set_gain(1, 6.0);
        i.set_gain(0, -4.0);
        s.reset();
        assert!(s.is_neutral());
        assert!(!i.is_neutral(), "the other channel kept its adjustment");
    }

    #[test]
    fn reset_returns_volume_to_unity_not_to_zero() {
        // Resetting to silence would be a reset that mutes you.
        let mut s = speaker();
        s.set_volume(180.0);
        s.reset();
        assert_eq!(s.volume, 100.0);
    }

    #[test]
    fn gains_are_clamped_to_the_twelve_db_range() {
        let mut s = speaker();
        s.set_gain(0, 99.0);
        assert_eq!(s.low.gain_db, EQ_MAX_DB);
        s.set_gain(0, -99.0);
        assert_eq!(s.low.gain_db, -EQ_MAX_DB);
    }

    #[test]
    fn volume_spans_zero_to_two_hundred_percent() {
        let mut s = speaker();
        s.set_volume(500.0);
        assert_eq!(s.volume, VOLUME_MAX);
        s.set_volume(-5.0);
        assert_eq!(s.volume, 0.0);
    }

    #[test]
    fn unity_volume_is_zero_db() {
        let s = speaker();
        assert!(s.volume_db().abs() < 0.001);
    }

    #[test]
    fn double_volume_is_about_six_db() {
        let mut s = speaker();
        s.set_volume(200.0);
        assert!((s.volume_db() - 6.02).abs() < 0.05, "got {}", s.volume_db());
    }

    #[test]
    fn silence_is_a_floor_rather_than_negative_infinity() {
        // Otherwise the curve disappears off the bottom of the plot instead of
        // sitting on it, and the user cannot see where their channel went.
        let mut s = speaker();
        s.set_volume(0.0);
        assert!(s.volume_db().is_finite());
        assert!(s.response_db(1000.0).is_finite());
    }

    #[test]
    fn a_band_peaks_at_its_own_centre_frequency() {
        assert!((band_contribution(1500.0, 6.0, 1500.0) - 6.0).abs() < 0.001);
    }

    #[test]
    fn a_bands_bell_is_symmetric_in_octaves_not_in_hertz() {
        // An octave below and an octave above are equally far away musically,
        // though one is a tenth of the Hz distance of the other. A linear bell
        // would spill upward and barely reach downward.
        let below = band_contribution(1000.0, 6.0, 500.0);
        let above = band_contribution(1000.0, 6.0, 2000.0);
        assert!((below - above).abs() < 0.001, "{below} vs {above}");
        assert!(below > 0.0 && below < 6.0);
    }

    #[test]
    fn a_band_at_zero_gain_contributes_nothing_anywhere() {
        for hz in [20.0, 200.0, 1500.0, 20_000.0] {
            assert_eq!(band_contribution(1500.0, 0.0, hz), 0.0);
        }
    }

    #[test]
    fn a_bands_influence_fades_with_distance() {
        let near = band_contribution(1000.0, 12.0, 1200.0);
        let far = band_contribution(1000.0, 12.0, 8000.0);
        assert!(near > far);
        assert!(far.abs() < 1.0, "three octaves out should be nearly nothing, got {far}");
    }

    #[test]
    fn turning_up_one_band_lifts_the_curve_near_it_and_leaves_the_rest() {
        // The point of the whole visualisation: independence made visible.
        let mut s = speaker();
        let before_low = s.response_db(200.0);
        let before_high = s.response_db(8000.0);
        s.set_gain(1, 8.0);            // mid, at 1.5kHz
        assert!(s.response_db(1500.0) > 7.0, "the mid lifted");
        assert!((s.response_db(200.0) - before_low).abs() < 0.5, "the low barely moved");
        assert!((s.response_db(8000.0) - before_high).abs() < 0.5, "nor the high");
    }

    #[test]
    fn the_two_channels_are_independent() {
        let mut s = speaker();
        let i = instrumental();
        s.set_gain(0, 10.0);
        assert!(s.response_db(200.0) > 8.0);
        assert!(i.response_db(200.0).abs() < 0.001, "the other channel did not move");
    }

    #[test]
    fn the_frequency_axis_is_logarithmic() {
        // On a linear axis everything below 2kHz lands in the first tenth of
        // the width — which is most of the music.
        let x200 = x_for_hz(200.0);
        let x2k = x_for_hz(2000.0);
        let x20k = x_for_hz(20_000.0);
        // Each decade takes equal room.
        assert!(((x2k - x200) - (x20k - x2k)).abs() < 0.01);
        assert!((x_for_hz(20.0) - 0.0).abs() < 0.001);
        assert!((x20k - 1.0).abs() < 0.001);
    }

    #[test]
    fn the_axis_clamps_rather_than_running_off_the_plot() {
        assert_eq!(x_for_hz(1.0), 0.0);
        assert_eq!(x_for_hz(100_000.0), 1.0);
        assert!(x_for_hz(0.0).is_finite(), "a zero frequency must not be -inf");
    }

    #[test]
    fn the_grid_marks_are_the_ones_people_read_frequencies_in() {
        assert!(GRID_HZ.contains(&1000.0));
        assert!(GRID_HZ.contains(&100.0));
        for hz in GRID_HZ {
            let x = x_for_hz(hz);
            assert!((0.0..=1.0).contains(&x), "{hz} fell off the plot");
        }
    }

    #[test]
    fn a_frequency_label_reads_as_a_place_not_a_number() {
        // "1.5 kHz" is somewhere; "1500 Hz" is a quantity.
        let b = |hz| ChannelBand { center_hz: hz, gain_db: 0.0, role_hint: String::new() };
        assert_eq!(b(200.0).freq_label(), "200 Hz");
        assert_eq!(b(1500.0).freq_label(), "1.5 kHz");
        assert_eq!(b(8000.0).freq_label(), "8 kHz");
        assert_eq!(b(1000.0).freq_label(), "1 kHz", "no pointless .0");
    }

    #[test]
    fn a_column_knows_when_it_has_nothing_to_undo() {
        let mut s = speaker();
        assert!(s.is_neutral());
        // The tolerance is for float noise, not for a nudge. A user who moved
        // the slider one percent DID something and should be able to undo it.
        s.set_volume(100.2);
        assert!(s.is_neutral(), "rounding noise is not an adjustment");
        s.set_volume(101.0);
        assert!(!s.is_neutral(), "but a deliberate one percent is");
    }

    #[test]
    fn every_band_carries_the_reason_its_frequency_was_chosen() {
        // A slider labelled only "MID" gives the user no way to know the bands
        // are picked for the channel's musical role.
        let s = speaker();
        assert_eq!(s.mid.role_hint, "vocal presence");
        assert!(s.bands().iter().all(|b| !b.role_hint.is_empty()));
    }
}
