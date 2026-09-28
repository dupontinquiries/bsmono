//! history — `HistoryPage`, ported.
//!
//! Recorded readings over time: a line chart, and four numbers under it — min,
//! max, current, and how many points the answer rests on.
//!
//! ## The count is a number, not decoration
//!
//! "Min 18.2, Max 24.6" reads as a fact about the week. Over four samples it is
//! a fact about four moments, and the difference matters to anyone deciding
//! whether the reading means anything. The QML shows the point count beside the
//! statistics for that reason and it comes across.
//!
//! ## A flat series is not a bug
//!
//! Indoors with the heating on, every reading can be the same to a tenth of a
//! degree. A chart that scaled to its own range would then render a wall of
//! noise at full amplitude — the classic autoscale failure, where a still room
//! looks like a storm. So a minimum span is enforced, and a genuinely flat
//! series draws as a flat line through the middle, which is the truth.
//!
//! ## Fewer than two points is not a chart
//!
//! The QML refuses to draw below two points rather than plotting a dot. A
//! single reading has no shape, and a chart with one mark on it invites the eye
//! to read a trend that does not exist.

/// One recorded reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Unix seconds.
    pub at: i64,
    pub temp_c: f32,
}

/// The narrowest range the chart will scale to, in °C.
///
/// Without it, a room that held 21.0–21.1 all day fills the chart with what is
/// really sensor quantisation. Half a degree is below what anyone feels and
/// above what the sensor wobbles by.
pub const MIN_SPAN_C: f32 = 0.5;

/// Below this many points there is no shape to draw.
pub const MIN_POINTS: usize = 2;

/// The chart's data and its derived statistics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    points: Vec<Point>,
}

impl History {
    pub fn new(points: Vec<Point>) -> Self { History { points } }

    pub fn points(&self) -> &[Point] { &self.points }
    pub fn len(&self) -> usize { self.points.len() }
    pub fn is_empty(&self) -> bool { self.points.is_empty() }

    /// Enough to draw a line?
    pub fn plottable(&self) -> bool { self.points.len() >= MIN_POINTS }

    pub fn min(&self) -> Option<f32> {
        self.points.iter().map(|p| p.temp_c).fold(None, |a: Option<f32>, v| {
            Some(a.map_or(v, |a| a.min(v)))
        })
    }

    pub fn max(&self) -> Option<f32> {
        self.points.iter().map(|p| p.temp_c).fold(None, |a: Option<f32>, v| {
            Some(a.map_or(v, |a| a.max(v)))
        })
    }

    /// The latest reading — the series is oldest-first, as the backend returns
    /// it.
    pub fn current(&self) -> Option<f32> { self.points.last().map(|p| p.temp_c) }

    /// The vertical range the chart should use.
    ///
    /// Widened to `MIN_SPAN_C` around the midpoint when the data is flatter
    /// than that, so a still room does not render as a storm.
    pub fn range(&self) -> Option<(f32, f32)> {
        let (lo, hi) = (self.min()?, self.max()?);
        let span = hi - lo;
        if span >= MIN_SPAN_C { return Some((lo, hi)); }
        let mid = (lo + hi) * 0.5;
        Some((mid - MIN_SPAN_C * 0.5, mid + MIN_SPAN_C * 0.5))
    }

    /// A point's position in the chart, 0..=1 on each axis.
    ///
    /// `y` is returned with 0 at the BOTTOM, so the caller flips it once for
    /// screen coordinates rather than every call site remembering to.
    pub fn normalised(&self, i: usize) -> Option<(f32, f32)> {
        if !self.plottable() { return None; }
        let p = self.points.get(i)?;
        let (lo, hi) = self.range()?;
        let x = i as f32 / (self.points.len() - 1) as f32;
        let y = ((p.temp_c - lo) / (hi - lo)).clamp(0.0, 1.0);
        Some((x, y))
    }

    /// The statistics row: min, max, current, count.
    ///
    /// Returns strings because an empty history has no numbers and a row of
    /// zeroes would be a lie about a sensor that has not reported yet.
    pub fn stats(&self) -> [String; 4] {
        let f = |v: Option<f32>| v.map(|v| format!("{v:.1}°")).unwrap_or_else(|| "—".into());
        [f(self.min()), f(self.max()), f(self.current()), self.points.len().to_string()]
    }

    /// What to say instead of a chart.
    pub fn empty_note(&self) -> Option<&'static str> {
        match self.points.len() {
            0 => Some("No readings recorded yet."),
            // Named rather than silent: a user who sees nothing after one
            // reading should know the app is waiting, not broken.
            1 => Some("One reading so far — a chart needs two."),
            _ => None,
        }
    }
}

/// Week navigation, from the GTK page: "◀  This Week  ▶", a Today button
/// and "Export as CSV" beneath the chart.
///
/// `week_offset` counts weeks back from the current one; the host turns it
/// into a query and fills `range` with the dates it actually covered, so the
/// label says what the chart shows rather than what was asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Nav {
    pub week_offset: usize,
    /// "Sep 21 – Sep 27", filled by the host from the query it ran.
    pub range: String,
    /// The last export's outcome, or a fetch error.
    pub note: Option<String>,
}

/// What the navigation asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    PrevWeek,
    NextWeek,
    Today,
    ExportCsv,
}

impl Nav {
    /// "This week", "Last week", "3 weeks ago" — the GTK label.
    pub fn title(&self) -> String {
        match self.week_offset {
            0 => "This week".into(),
            1 => "Last week".into(),
            n => format!("{n} weeks ago"),
        }
    }

    /// Forward is only meaningful from the past; the GTK page greys the
    /// arrow at the current week rather than letting it "go" nowhere.
    pub fn can_go_forward(&self) -> bool { self.week_offset > 0 }

    /// Apply a navigation action locally. Returns true when the week
    /// changed and the host should re-query.
    pub fn apply(&mut self, a: HistoryAction) -> bool {
        let before = self.week_offset;
        match a {
            HistoryAction::PrevWeek => self.week_offset += 1,
            HistoryAction::NextWeek => self.week_offset = self.week_offset.saturating_sub(1),
            HistoryAction::Today => self.week_offset = 0,
            HistoryAction::ExportCsv => {}
        }
        self.week_offset != before
    }
}

/// The whole page: week navigation, the chart, the export row.
pub fn history_page_ui(ui: &mut egui::Ui, h: &History, nav: &Nav,
                       colors: &uikit_egui::PaletteColors) -> Option<HistoryAction> {
    let mut out = None;
    ui.horizontal(|ui| {
        if ui.button("◀").clicked() { out = Some(HistoryAction::PrevWeek); }
        ui.vertical(|ui| {
            ui.colored_label(colors.text, egui::RichText::new(nav.title()).strong());
            if !nav.range.is_empty() {
                ui.colored_label(colors.text_weak, egui::RichText::new(&nav.range).small());
            }
        });
        if ui.add_enabled(nav.can_go_forward(), egui::Button::new("▶")).clicked() {
            out = Some(HistoryAction::NextWeek);
        }
        if ui.add_enabled(nav.can_go_forward(), egui::Button::new("Today")).clicked() {
            out = Some(HistoryAction::Today);
        }
    });
    history_ui(ui, h, colors);
    // What the numbers are: the GTK chart plots the daily mean of the first
    // thermal sensor as recorded, not the ambient prediction, and a 40°
    // "history" on a page about room temperature needs that said.
    ui.colored_label(colors.text_weak, egui::RichText::new(
        "Daily mean of the primary thermal sensor, from the recorded readings.").small());
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(!h.is_empty(), egui::Button::new("Export as CSV")).clicked() {
            out = Some(HistoryAction::ExportCsv);
        }
        if let Some(n) = &nav.note {
            ui.colored_label(colors.text_weak, egui::RichText::new(n).small());
        }
    });
    out
}

/// Draw the history chart and its statistics.
pub fn history_ui(ui: &mut egui::Ui, h: &History, colors: &uikit_egui::PaletteColors) {
    ui.heading("History");

    if let Some(note) = h.empty_note() {
        ui.colored_label(colors.text_weak, note);
    } else {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 140.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 6.0, colors.bg_raised);

        let mut prev: Option<egui::Pos2> = None;
        for i in 0..h.len() {
            let Some((nx, ny)) = h.normalised(i) else { continue };
            // Flipped once here: 0 is the bottom of the chart, the top of the
            // screen rect.
            let p = egui::pos2(rect.left() + nx * rect.width(),
                               rect.bottom() - ny * rect.height());
            if let Some(q) = prev {
                painter.line_segment([q, p], egui::Stroke::new(2.0_f32, colors.accent));
            }
            prev = Some(p);
        }
    }

    ui.add_space(6.0);
    crate::ui::egui::chrome::card_heading(ui, "📈 Recent statistics");
    let stats = h.stats();
    ui.horizontal(|ui| {
        for (label, value) in ["Min", "Max", "Curr", "Pts"].iter().zip(stats.iter()) {
            ui.vertical(|ui| {
                ui.colored_label(colors.text_weak, egui::RichText::new(*label).small());
                ui.colored_label(colors.text, value);
            });
            ui.add_space(12.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(temps: &[f32]) -> History {
        History::new(temps.iter().enumerate()
            .map(|(i, t)| Point { at: i as i64 * 600, temp_c: *t }).collect())
    }

    #[test]
    fn a_single_reading_is_not_a_chart() {
        // A chart with one mark invites the eye to read a trend that is not
        // there. The QML refuses below two points and so does this.
        let one = h(&[21.0]);
        assert!(!one.plottable());
        assert_eq!(one.normalised(0), None);
        assert_eq!(one.empty_note(), Some("One reading so far — a chart needs two."));
    }

    #[test]
    fn an_empty_history_says_so_rather_than_drawing_nothing() {
        // A user who sees a blank panel cannot tell waiting from broken.
        let none = History::default();
        assert_eq!(none.empty_note(), Some("No readings recorded yet."));
        assert_eq!(none.stats(), ["—", "—", "—", "0"]);
    }

    #[test]
    fn the_statistics_are_dashes_rather_than_zeroes_when_there_is_no_data() {
        // A row of 0.0° would be a lie about a sensor that has not reported.
        assert_eq!(History::default().stats()[0], "—");
    }

    #[test]
    fn min_max_and_current_read_off_the_series() {
        let x = h(&[19.0, 23.5, 21.2]);
        assert_eq!(x.min(), Some(19.0));
        assert_eq!(x.max(), Some(23.5));
        assert_eq!(x.current(), Some(21.2), "current is the newest, not the largest");
    }

    #[test]
    fn the_series_is_oldest_first_so_current_is_the_last_point() {
        // The backend returns it that way; reading it backwards would show
        // yesterday's temperature as now.
        let x = h(&[10.0, 20.0]);
        assert_eq!(x.current(), Some(20.0));
    }

    #[test]
    fn the_point_count_is_reported_because_four_samples_is_not_a_week() {
        // "Min 18.2, Max 24.6" reads as a fact about the week; over four
        // samples it is a fact about four moments.
        assert_eq!(h(&[1.0, 2.0, 3.0, 4.0]).stats()[3], "4");
    }

    #[test]
    fn a_flat_series_does_not_render_as_a_storm() {
        // THE autoscale failure: a room that held 21.0-21.1 all day would fill
        // the chart with sensor quantisation.
        let flat = h(&[21.0, 21.05, 21.0, 21.1]);
        let (lo, hi) = flat.range().unwrap();
        assert!(hi - lo >= MIN_SPAN_C, "span was {}", hi - lo);
        // And the points sit near the middle rather than spanning the chart.
        let ys: Vec<f32> = (0..flat.len()).map(|i| flat.normalised(i).unwrap().1).collect();
        assert!(ys.iter().all(|y| (*y - 0.5).abs() < 0.25), "{ys:?}");
    }

    #[test]
    fn a_perfectly_flat_series_draws_through_the_middle() {
        let flat = h(&[21.0, 21.0, 21.0]);
        for i in 0..flat.len() {
            let (_, y) = flat.normalised(i).unwrap();
            assert!((y - 0.5).abs() < 0.001, "got {y}");
        }
    }

    #[test]
    fn a_real_range_is_used_as_is() {
        let varied = h(&[18.0, 24.0]);
        assert_eq!(varied.range(), Some((18.0, 24.0)));
    }

    #[test]
    fn the_x_axis_spans_the_full_width() {
        let x = h(&[1.0, 2.0, 3.0]);
        assert_eq!(x.normalised(0).unwrap().0, 0.0);
        assert_eq!(x.normalised(2).unwrap().0, 1.0);
        assert!((x.normalised(1).unwrap().0 - 0.5).abs() < 0.001);
    }

    #[test]
    fn y_is_zero_at_the_bottom_so_the_flip_happens_once() {
        // Every call site remembering to invert is how half a chart ends up
        // upside down.
        let x = h(&[18.0, 24.0]);
        assert_eq!(x.normalised(0).unwrap().1, 0.0, "the coldest reading is at the bottom");
        assert_eq!(x.normalised(1).unwrap().1, 1.0);
    }

    #[test]
    fn a_point_past_the_end_is_none_rather_than_a_panic() {
        assert_eq!(h(&[1.0, 2.0]).normalised(9), None);
    }

    #[test]
    fn negative_temperatures_plot_normally() {
        // The obvious off-by-a-sign: a winter series must not invert or clamp.
        let cold = h(&[-8.0, -2.0]);
        assert_eq!(cold.min(), Some(-8.0));
        assert_eq!(cold.normalised(0).unwrap().1, 0.0);
        assert_eq!(cold.normalised(1).unwrap().1, 1.0);
    }

    #[test]
    fn the_statistics_are_shown_to_one_decimal() {
        // Matching the sensor's honest precision: two decimals would claim
        // resolution a BME280 does not have.
        assert_eq!(h(&[21.44, 21.46]).stats()[0], "21.4°");
    }

    #[test]
    fn the_week_label_reads_like_the_gtk_one() {
        let mut n = Nav::default();
        assert_eq!(n.title(), "This week");
        assert!(!n.can_go_forward(), "nothing after the current week");
        assert!(n.apply(HistoryAction::PrevWeek));
        assert_eq!(n.title(), "Last week");
        n.apply(HistoryAction::PrevWeek);
        assert_eq!(n.title(), "2 weeks ago");
        assert!(n.apply(HistoryAction::Today));
        assert_eq!(n.week_offset, 0);
        assert!(!n.apply(HistoryAction::NextWeek), "cannot go past today");
        assert!(!n.apply(HistoryAction::ExportCsv), "an export does not move the week");
    }
}
