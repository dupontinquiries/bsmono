//! pressure — `PressureGauge`, ported.
//!
//! A semicircular dial for absolute pressure and an arrow for where it is
//! going. Two readings, and the second is the one that predicts weather: a
//! barometer at 1004 hPa tells you almost nothing, but 1004 and *falling three
//! an hour* tells you a front is arriving.
//!
//! ## The needle saturates rather than running off
//!
//! The dial covers 980–1040 hPa, which is most weather most places. Outside
//! that the needle sits at the edge, and the QML comment says why: an extreme
//! value means "weather event" whatever the exact number, so direction matters
//! more than precision. A dial that rescaled to fit an outlier would make every
//! ordinary day look identical.
//!
//! ## The trend arrow is sized by magnitude, not just pointed
//!
//! A 0.2 hPa drift and a 6 hPa collapse are different events, and an arrow that
//! only pointed would show them the same way. Size carries the strength; the
//! direction carries the sign.
//!
//! ## The trend can be absent, and absent is not zero
//!
//! `environment::pressure_trend_hpa` returns `None` until it has four samples
//! spanning five minutes — it refuses to extrapolate a trend from a short
//! window, which is right, because a barometer wobbles. Rendering `None` as a
//! flat arrow would claim "steady" when the honest answer is "not yet known",
//! and a user who saw "steady" before a storm would stop trusting the gauge.

/// The dial's range. Most weather, most places.
pub const DIAL_MIN_HPA: f32 = 980.0;
pub const DIAL_MAX_HPA: f32 = 1040.0;

/// Sea-level reference, for the "above/below normal" reading.
pub const SEA_LEVEL_HPA: f32 = 1013.25;

/// A trend at or under this is not worth calling a direction. Below it the
/// arrow reads "steady" rather than pointing at noise.
pub const STEADY_HPA_PER_30M: f32 = 0.3;

/// A trend at or over this is a front, and the arrow is at full size.
pub const STRONG_HPA_PER_30M: f32 = 3.0;

/// Which way the pressure is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// Not enough history yet. Distinct from Steady — see the module docs.
    Unknown,
    Rising,
    Steady,
    Falling,
}

impl Trend {
    pub fn glyph(self) -> &'static str {
        match self {
            Trend::Unknown => "·",
            Trend::Rising => "▲",
            Trend::Steady => "—",
            Trend::Falling => "▼",
        }
    }

    /// What it means, in the terms a forecast uses.
    ///
    /// Named rather than left to the arrow: most people know a falling
    /// barometer is bad news but not which way the arrow points.
    pub fn meaning(self) -> &'static str {
        match self {
            Trend::Unknown => "collecting readings",
            Trend::Rising => "clearing",
            Trend::Steady => "settled",
            Trend::Falling => "front approaching",
        }
    }
}

/// A pressure reading with its trend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pressure {
    pub hpa: f32,
    /// hPa per 30 minutes, as `environment::pressure_trend_hpa` reports it.
    /// `None` means not enough history — NOT zero.
    pub trend_hpa_per_30m: Option<f32>,
}

impl Pressure {
    /// Where the needle sits, 0..=1 across the dial.
    ///
    /// Saturating rather than rescaling: an extreme value means "weather event"
    /// whatever the number, and a dial that stretched to fit one would flatten
    /// every ordinary day into the middle.
    pub fn needle(&self) -> f32 {
        ((self.hpa - DIAL_MIN_HPA) / (DIAL_MAX_HPA - DIAL_MIN_HPA)).clamp(0.0, 1.0)
    }

    /// Is the reading off the end of the dial? Worth showing, because a
    /// saturated needle otherwise looks like a stuck one.
    pub fn saturated(&self) -> bool {
        self.hpa < DIAL_MIN_HPA || self.hpa > DIAL_MAX_HPA
    }

    pub fn trend(&self) -> Trend {
        match self.trend_hpa_per_30m {
            None => Trend::Unknown,
            Some(t) if t.abs() <= STEADY_HPA_PER_30M => Trend::Steady,
            Some(t) if t > 0.0 => Trend::Rising,
            Some(_) => Trend::Falling,
        }
    }

    /// Arrow size, 0..=1 by magnitude.
    ///
    /// A 0.2 hPa drift and a 6 hPa collapse are different events; an arrow that
    /// only pointed would show them identically.
    pub fn trend_strength(&self) -> f32 {
        match self.trend_hpa_per_30m {
            None => 0.0,
            Some(t) => (t.abs() / STRONG_HPA_PER_30M).clamp(0.0, 1.0),
        }
    }

    /// The numeric readout, overlaid so the gauge stays useful at a glance.
    pub fn label(&self) -> String { format!("{:.0} hPa", self.hpa) }

    /// The trend readout. Names the rate, because "falling" alone does not say
    /// whether to bring a coat or cancel the afternoon.
    pub fn trend_label(&self) -> String {
        match self.trend_hpa_per_30m {
            None => "collecting readings".into(),
            Some(t) if t.abs() <= STEADY_HPA_PER_30M => "steady".into(),
            Some(t) => format!("{:+.1} hPa/30m — {}", t, self.trend().meaning()),
        }
    }

    /// Above or below the sea-level reference, for the secondary line.
    pub fn vs_normal(&self) -> String {
        let d = self.hpa - SEA_LEVEL_HPA;
        if d.abs() < 0.5 { "about normal".into() }
        else if d > 0.0 { format!("{d:.0} above normal") }
        else { format!("{:.0} below normal", d.abs()) }
    }
}

/// Draw the gauge. `eased` is the needle position the caller is animating
/// toward the target — held by the caller so the easing survives the frame.
pub fn gauge_ui(ui: &mut egui::Ui, p: Pressure, eased: f32,
                c: &uikit_egui::PaletteColors) {
    let size = ui.available_width().min(180.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size * 0.62),
                                           egui::Sense::hover());
    let centre = egui::pos2(rect.center().x, rect.bottom());
    let radius = size * 0.46;
    let painter = ui.painter();

    // The arc: a semicircle from left (low) to right (high).
    let seg = 40;
    for i in 0..seg {
        let a0 = std::f32::consts::PI * (1.0 + i as f32 / seg as f32);
        let a1 = std::f32::consts::PI * (1.0 + (i + 1) as f32 / seg as f32);
        painter.line_segment(
            [centre + egui::vec2(a0.cos(), a0.sin()) * radius,
             centre + egui::vec2(a1.cos(), a1.sin()) * radius],
            egui::Stroke::new(2.0_f32, c.border));
    }

    // The needle, at the eased position rather than the raw one.
    let a = std::f32::consts::PI * (1.0 + eased.clamp(0.0, 1.0));
    let tip = centre + egui::vec2(a.cos(), a.sin()) * (radius - 6.0);
    // Saturation is shown in the needle's colour: a needle pinned at the edge
    // otherwise looks like a broken one.
    let needle_col = if p.saturated() { c.warning } else { c.accent };
    painter.line_segment([centre, tip], egui::Stroke::new(3.0_f32, needle_col));
    painter.circle_filled(centre, 4.0, needle_col);

    painter.text(centre + egui::vec2(0.0, -radius * 0.45), egui::Align2::CENTER_CENTER,
                 p.label(), egui::FontId::proportional(18.0), c.text);
    painter.text(centre + egui::vec2(0.0, -radius * 0.2), egui::Align2::CENTER_CENTER,
                 p.vs_normal(), egui::FontId::proportional(10.0), c.text_weak);

    ui.horizontal(|ui| {
        let t = p.trend();
        // Sized by magnitude: strength is the information, not just direction.
        let scale = 10.0 + 10.0 * p.trend_strength();
        let col = match t {
            Trend::Falling => c.warning,
            Trend::Rising => c.success,
            _ => c.text_weak,
        };
        ui.label(egui::RichText::new(t.glyph()).size(scale).color(col));
        ui.colored_label(c.text_weak, p.trend_label());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(hpa: f32, trend: Option<f32>) -> Pressure {
        Pressure { hpa, trend_hpa_per_30m: trend }
    }

    #[test]
    fn the_needle_spans_the_dial() {
        assert_eq!(p(DIAL_MIN_HPA, None).needle(), 0.0);
        assert_eq!(p(DIAL_MAX_HPA, None).needle(), 1.0);
        assert!((p(1010.0, None).needle() - 0.5).abs() < 0.01);
    }

    #[test]
    fn an_extreme_reading_saturates_rather_than_rescaling_the_dial() {
        // A dial that stretched to fit an outlier would flatten every ordinary
        // day into the middle.
        assert_eq!(p(940.0, None).needle(), 0.0);
        assert_eq!(p(1080.0, None).needle(), 1.0);
        assert!(p(940.0, None).saturated());
        assert!(!p(1010.0, None).saturated());
    }

    #[test]
    fn an_unknown_trend_is_not_a_steady_one() {
        // THE distinction. environment::pressure_trend_hpa returns None until
        // it has four samples over five minutes — it refuses to extrapolate
        // from a short window, and rendering that as "steady" would claim
        // settled weather right before a storm.
        assert_eq!(p(1004.0, None).trend(), Trend::Unknown);
        assert_ne!(p(1004.0, None).trend(), Trend::Steady);
        assert_eq!(p(1004.0, None).trend_label(), "collecting readings");
    }

    #[test]
    fn a_small_wobble_reads_as_steady_rather_than_a_direction() {
        // A barometer wobbles; pointing an arrow at noise would have it
        // flickering between clearing and storm all afternoon.
        assert_eq!(p(1004.0, Some(0.1)).trend(), Trend::Steady);
        assert_eq!(p(1004.0, Some(-0.2)).trend(), Trend::Steady);
        assert_eq!(p(1004.0, Some(STEADY_HPA_PER_30M)).trend(), Trend::Steady);
    }

    #[test]
    fn a_real_movement_gets_a_direction() {
        assert_eq!(p(1004.0, Some(1.5)).trend(), Trend::Rising);
        assert_eq!(p(1004.0, Some(-1.5)).trend(), Trend::Falling);
    }

    #[test]
    fn the_arrow_is_sized_by_magnitude_not_just_pointed() {
        // A 0.2 drift and a 6 hPa collapse are different events.
        let drift = p(1004.0, Some(0.5)).trend_strength();
        let front = p(1004.0, Some(3.0)).trend_strength();
        assert!(front > drift);
        assert_eq!(front, 1.0, "a full front saturates the arrow");
        assert_eq!(p(1004.0, Some(9.0)).trend_strength(), 1.0, "and stays there");
        assert_eq!(p(1004.0, None).trend_strength(), 0.0);
    }

    #[test]
    fn strength_ignores_direction() {
        assert_eq!(p(1004.0, Some(2.0)).trend_strength(),
                   p(1004.0, Some(-2.0)).trend_strength());
    }

    #[test]
    fn the_trend_label_names_the_rate_and_what_it_means() {
        // "Falling" alone does not say whether to bring a coat or cancel the
        // afternoon.
        let l = p(1004.0, Some(-2.4)).trend_label();
        assert!(l.contains("-2.4"), "got {l}");
        assert!(l.contains("front approaching"));
        let r = p(1004.0, Some(2.4)).trend_label();
        assert!(r.contains("+2.4"), "the sign is explicit: got {r}");
        assert!(r.contains("clearing"));
    }

    #[test]
    fn the_trend_meaning_is_named_because_arrows_are_ambiguous() {
        // Most people know a falling barometer is bad news but not which way
        // the arrow points.
        assert_eq!(Trend::Falling.meaning(), "front approaching");
        assert_eq!(Trend::Rising.meaning(), "clearing");
        assert_eq!(Trend::Unknown.meaning(), "collecting readings");
    }

    #[test]
    fn the_secondary_line_compares_against_sea_level() {
        assert_eq!(p(SEA_LEVEL_HPA, None).vs_normal(), "about normal");
        assert!(p(1020.0, None).vs_normal().contains("above"));
        assert!(p(1000.0, None).vs_normal().contains("below"));
    }

    #[test]
    fn the_numeric_readout_stays_useful_at_a_glance() {
        // The gauge is a shape; the number is what you actually quote.
        assert_eq!(p(1013.6, None).label(), "1014 hPa");
    }

    #[test]
    fn the_needle_never_leaves_the_dial_for_any_input() {
        for hpa in [0.0, 500.0, 980.0, 1013.0, 1040.0, 2000.0, f32::MAX] {
            let n = p(hpa, None).needle();
            assert!((0.0..=1.0).contains(&n), "{hpa} gave {n}");
        }
    }
}
