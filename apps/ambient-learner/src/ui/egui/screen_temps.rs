//! screen_temps — `DebugScreenTempsPage` and the frame it reads.
//!
//! ambient-learner publishes a 2D reconstruction of the screen surface
//! temperature to `$XDG_RUNTIME_DIR/ambient-learner/screen-temps.bin` at 4 Hz,
//! for any app on the device to read. This is the viewer, and — more usefully —
//! the parser other apps need.
//!
//! ## It is labelled a debug page and it is not only a debug feature
//!
//! `SCREEN_TEMPS.md` lists the intended consumers, and two of them are things
//! this fleet has already built around:
//!
//! * **Wet-screen touch correction.** Water on the glass makes false touches at
//!   COLD spots, because a droplet pulls heat out of the surface. Subtracting
//!   the cold-spot pattern recovers the real touches.
//! * **Glove-finger detection.** A finger inside a glove still warms the
//!   contact patch. Warm under an ambiguous touch means finger; cold plus touch
//!   means precipitation.
//!
//! That second one is the exact problem glove mode exists for, arrived at from
//! the other direction — cold mode assumes the screen cannot be touched at all
//! and routes around it, while this could tell a gloved tap from a raindrop and
//! let some touches through. Worth connecting later; noted here rather than
//! built, because it needs the touch layer as well as this one.
//!
//! ## An error and a frame are mutually exclusive
//!
//! The QML says so in a comment and keeps two properties in step by hand. Here
//! it is a `Result`, so the state that comment describes cannot be violated —
//! there is no way to hold a stale frame and an error at once and render both.

/// The magic at the head of every frame.
pub const MAGIC: &[u8; 4] = b"AMBT";
/// The format version this reader understands.
pub const VERSION: u16 = 1;
/// Header length before the cell data.
pub const HEADER_LEN: usize = 24;

/// Publish cadence, 4 Hz.
pub const PUBLISH_MS: i64 = 250;

/// A frame older than this is stale.
///
/// Four publish intervals. Long enough that one missed write is not an alarm,
/// short enough that a publisher which has died is noticed before the user
/// starts trusting the picture.
pub const STALE_AFTER_MS: i64 = PUBLISH_MS * 4;

/// Why a frame could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The publisher is not running, or has not written yet.
    Missing,
    /// Not our format at all.
    BadMagic,
    /// A newer publisher. Refused rather than guessed at.
    Version(u16),
    /// Truncated, or the cell count does not match the grid.
    Truncated { expected: usize, got: usize },
    /// A grid with no cells.
    EmptyGrid,
}

impl FrameError {
    /// What the debug page shows beside its warning triangle.
    pub fn message(&self) -> String {
        match self {
            FrameError::Missing => "No frame published yet".into(),
            FrameError::BadMagic => "Not a screen-temp frame".into(),
            // Names the version rather than saying "unsupported": the reader
            // being old is a different fix from the file being corrupt, and the
            // number is what tells them apart.
            FrameError::Version(v) => format!("Frame version {v}, this reader understands {VERSION}"),
            FrameError::Truncated { expected, got } =>
                format!("Truncated: expected {expected} cells, got {got}"),
            FrameError::EmptyGrid => "Grid has no cells".into(),
        }
    }
}

/// One decoded frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    /// Unix nanoseconds, as published.
    pub timestamp_ns: u64,
    /// Monotonic, so a viewer can tell a fresh frame from a repeat.
    pub generation: u32,
    /// Row-major, `cols * rows` cells in °C.
    pub cells: Vec<f32>,
}

impl Frame {
    /// Decode a published frame.
    pub fn parse(bytes: &[u8]) -> Result<Frame, FrameError> {
        if bytes.is_empty() { return Err(FrameError::Missing); }
        if bytes.len() < HEADER_LEN { return Err(FrameError::Truncated {
            expected: HEADER_LEN, got: bytes.len() }); }
        if &bytes[0..4] != MAGIC { return Err(FrameError::BadMagic); }
        let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let version = u16at(4);
        // A NEWER publisher is refused rather than read optimistically: the
        // fields may mean something else, and a plausible-looking wrong
        // temperature map is worse than no map.
        if version != VERSION { return Err(FrameError::Version(version)); }
        let cols = u16at(6);
        let rows = u16at(8);
        if cols == 0 || rows == 0 { return Err(FrameError::EmptyGrid); }

        let timestamp_ns = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let generation = u32::from_le_bytes(bytes[20..24].try_into().unwrap());

        let want = cols as usize * rows as usize;
        let avail = (bytes.len() - HEADER_LEN) / 4;
        if avail < want { return Err(FrameError::Truncated { expected: want, got: avail }); }

        let mut cells = Vec::with_capacity(want);
        for i in 0..want {
            let o = HEADER_LEN + i * 4;
            cells.push(f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()));
        }
        Ok(Frame { cols, rows, timestamp_ns, generation, cells })
    }

    pub fn cell(&self, col: u16, row: u16) -> Option<f32> {
        if col >= self.cols || row >= self.rows { return None; }
        self.cells.get(row as usize * self.cols as usize + col as usize).copied()
    }

    /// Age in milliseconds against a caller-supplied clock.
    pub fn age_ms(&self, now_ns: u64) -> i64 {
        ((now_ns as i128 - self.timestamp_ns as i128) / 1_000_000) as i64
    }

    /// Has the publisher stopped?
    pub fn is_stale(&self, now_ns: u64) -> bool { self.age_ms(now_ns) > STALE_AFTER_MS }

    /// The observed temperature band, ignoring non-finite cells.
    ///
    /// The publisher tells the viewer its own range so the colours stretch to
    /// whatever band the device is actually in — a phone in a warm room and one
    /// on a cold windowsill both use the full scale, which is the only way the
    /// pattern is visible in either.
    pub fn range(&self) -> Option<(f32, f32)> {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for c in self.cells.iter().filter(|c| c.is_finite()) {
            lo = lo.min(*c); hi = hi.max(*c);
        }
        if !lo.is_finite() { return None; }
        // A perfectly uniform surface would divide by zero when normalising.
        if hi - lo < 0.01 { return Some((lo - 0.5, hi + 0.5)); }
        Some((lo, hi))
    }

    /// Normalise a cell into 0..=1 across the frame's own range.
    pub fn normalised(&self, col: u16, row: u16) -> Option<f32> {
        let v = self.cell(col, row)?;
        if !v.is_finite() { return None; }
        let (lo, hi) = self.range()?;
        Some(((v - lo) / (hi - lo)).clamp(0.0, 1.0))
    }
}

/// Cold-to-warm colour for a normalised cell.
///
/// Blue through to red. Deliberately not a rainbow: a hue ramp makes
/// mid-range values look like distinct categories, and this is a continuous
/// quantity where the question is nearly always "which end".
pub fn heat_color(t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let r = (40.0 + 215.0 * t) as u8;
    let g = (60.0 + 80.0 * (1.0 - (t - 0.5).abs() * 2.0).max(0.0)) as u8;
    let b = (200.0 * (1.0 - t) + 40.0) as u8;
    egui::Color32::from_rgb(r, g, b)
}

/// The debug page's state: a frame or a reason there is not one, never both.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenTempsPage {
    pub state: Result<Frame, FrameError>,
}

impl Default for ScreenTempsPage {
    fn default() -> Self { ScreenTempsPage { state: Err(FrameError::Missing) } }
}

impl ScreenTempsPage {
    /// The header row: grid, generation, age, range.
    pub fn header(&self, now_ns: u64) -> [(&'static str, String); 4] {
        match &self.state {
            Err(_) => [("Grid", "—".into()), ("Gen", "—".into()),
                       ("Age", "—".into()), ("Range", "—".into())],
            Ok(f) => [
                ("Grid", format!("{}×{}", f.cols, f.rows)),
                ("Gen", f.generation.to_string()),
                ("Age", format!("{} ms", f.age_ms(now_ns))),
                ("Range", match f.range() {
                    Some((lo, hi)) => format!("{lo:.1}–{hi:.1}°C"),
                    None => "—".into(),
                }),
            ],
        }
    }
}

/// Draw the viewer.
pub fn screen_temps_ui(ui: &mut egui::Ui, p: &ScreenTempsPage, now_ns: u64,
                       c: &uikit_egui::PaletteColors) {
    if let Err(e) = &p.state {
        ui.colored_label(c.danger, format!("⚠  {}", e.message()));
        return;
    }
    let f = p.state.as_ref().unwrap();
    ui.horizontal(|ui| {
        for (label, value) in p.header(now_ns) {
            ui.vertical(|ui| {
                ui.colored_label(c.text_weak, egui::RichText::new(label).small());
                ui.colored_label(c.text, value);
            });
            ui.add_space(12.0);
        }
    });
    if f.is_stale(now_ns) {
        // Said explicitly rather than shown by a frozen picture: a still
        // heatmap looks exactly like a stable one.
        ui.colored_label(c.warning, "⚠  Publisher may have stopped");
    }

    let size = egui::vec2(ui.available_width(), 220.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let cw = rect.width() / f.cols as f32;
    let ch = rect.height() / f.rows as f32;
    for row in 0..f.rows {
        for col in 0..f.cols {
            let Some(n) = f.normalised(col, row) else { continue };
            let cell = egui::Rect::from_min_size(
                egui::pos2(rect.left() + col as f32 * cw, rect.top() + row as f32 * ch),
                egui::vec2(cw, ch));
            ui.painter().rect_filled(cell, 0.0, heat_color(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_bytes(cols: u16, rows: u16, gen: u32, ts: u64, cells: &[f32]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&cols.to_le_bytes());
        b.extend_from_slice(&rows.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&gen.to_le_bytes());
        for c in cells { b.extend_from_slice(&c.to_le_bytes()); }
        b
    }

    fn good() -> Vec<u8> {
        frame_bytes(2, 2, 7, 1_000_000_000, &[20.0, 25.0, 30.0, 35.0])
    }

    #[test]
    fn a_published_frame_round_trips() {
        let f = Frame::parse(&good()).expect("parses");
        assert_eq!((f.cols, f.rows), (2, 2));
        assert_eq!(f.generation, 7);
        assert_eq!(f.cell(0, 0), Some(20.0));
        assert_eq!(f.cell(1, 1), Some(35.0));
    }

    #[test]
    fn a_cell_outside_the_grid_is_none_rather_than_wrapping() {
        // Row-major indexing that wrapped would show the next row's value and
        // look like a plausible reading.
        let f = Frame::parse(&good()).unwrap();
        assert_eq!(f.cell(2, 0), None);
        assert_eq!(f.cell(0, 2), None);
    }

    #[test]
    fn nothing_published_yet_is_missing_rather_than_an_error() {
        // The publisher not having run is the ordinary state on a fresh boot,
        // and must not read as corruption.
        assert_eq!(Frame::parse(&[]), Err(FrameError::Missing));
    }

    #[test]
    fn a_foreign_file_is_rejected_by_its_magic() {
        let mut b = good();
        b[0] = b'X';
        assert_eq!(Frame::parse(&b), Err(FrameError::BadMagic));
    }

    #[test]
    fn a_newer_version_is_refused_rather_than_read_optimistically() {
        // The fields may mean something else, and a plausible-looking wrong
        // temperature map is worse than no map. The message names the version,
        // because "reader is old" and "file is corrupt" are different fixes.
        let mut b = good();
        b[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(Frame::parse(&b), Err(FrameError::Version(2)));
        assert!(Frame::parse(&b).unwrap_err().message().contains('2'));
    }

    #[test]
    fn a_truncated_frame_says_how_short_it_is() {
        // A half-written file caught mid-publish is expected at 4 Hz, and the
        // numbers say whether it is that or a wrong grid size.
        let mut b = good();
        b.truncate(HEADER_LEN + 4);
        match Frame::parse(&b) {
            Err(FrameError::Truncated { expected, got }) => {
                assert_eq!(expected, 4);
                assert_eq!(got, 1);
            }
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_grid_is_refused_before_it_divides_by_zero() {
        let b = frame_bytes(0, 4, 1, 0, &[]);
        assert_eq!(Frame::parse(&b), Err(FrameError::EmptyGrid));
    }

    #[test]
    fn the_range_stretches_to_whatever_band_the_device_is_in() {
        // A phone in a warm room and one on a cold windowsill both use the full
        // scale, which is the only way the pattern is visible in either.
        let f = Frame::parse(&good()).unwrap();
        assert_eq!(f.range(), Some((20.0, 35.0)));
        assert_eq!(f.normalised(0, 0), Some(0.0));
        assert_eq!(f.normalised(1, 1), Some(1.0));
    }

    #[test]
    fn a_uniform_surface_does_not_divide_by_zero() {
        let b = frame_bytes(2, 1, 1, 0, &[21.0, 21.0]);
        let f = Frame::parse(&b).unwrap();
        let (lo, hi) = f.range().unwrap();
        assert!(hi > lo);
        assert!(f.normalised(0, 0).unwrap().is_finite());
    }

    #[test]
    fn a_non_finite_cell_is_skipped_rather_than_poisoning_the_range() {
        // One NaN would make the whole map blank.
        let b = frame_bytes(3, 1, 1, 0, &[20.0, f32::NAN, 30.0]);
        let f = Frame::parse(&b).unwrap();
        assert_eq!(f.range(), Some((20.0, 30.0)));
        assert_eq!(f.normalised(1, 0), None, "and that one cell is not drawn");
    }

    #[test]
    fn staleness_allows_one_missed_publish_but_not_many() {
        // Four intervals: a single dropped write at 4 Hz is not an alarm, a
        // dead publisher is noticed before the user trusts the picture.
        let f = Frame::parse(&good()).unwrap();
        let base = f.timestamp_ns;
        assert!(!f.is_stale(base + 500 * 1_000_000));
        assert!(f.is_stale(base + 2_000 * 1_000_000));
    }

    #[test]
    fn a_frame_from_the_future_is_not_stale() {
        // Clock skew between publisher and reader must not flag a fresh frame.
        let f = Frame::parse(&good()).unwrap();
        assert!(!f.is_stale(f.timestamp_ns - 1_000_000_000));
    }

    #[test]
    fn an_error_and_a_frame_cannot_both_be_held() {
        // The QML keeps two properties in step by hand and says in a comment
        // that they are mutually exclusive. A Result makes that unviolatable.
        let p = ScreenTempsPage::default();
        assert!(p.state.is_err());
        let head = p.header(0);
        assert!(head.iter().all(|(_, v)| v == "—"),
                "no frame means no numbers, not stale ones");
    }

    #[test]
    fn the_header_reports_the_grid_generation_age_and_range() {
        let p = ScreenTempsPage { state: Frame::parse(&good()) };
        let h = p.header(1_500_000_000);
        assert_eq!(h[0].1, "2×2");
        assert_eq!(h[1].1, "7");
        assert_eq!(h[2].1, "500 ms");
        assert!(h[3].1.contains("20.0"));
    }

    #[test]
    fn the_heat_scale_runs_cold_to_warm_without_a_rainbow() {
        // A hue ramp makes mid-range values look like distinct categories, and
        // this is a continuous quantity where the question is which end.
        let cold = heat_color(0.0);
        let hot = heat_color(1.0);
        assert!(hot.r() > cold.r());
        assert!(cold.b() > hot.b());
    }
}
