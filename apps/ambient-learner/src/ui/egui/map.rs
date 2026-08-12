//! map — `MapPage`, ported.
//!
//! A scatter map of saved places and recorded pins. There is no tile server and
//! no basemap: the whole thing is points projected into a box that fits them.
//! That sounds like a limitation and is mostly a feature — it works offline, on
//! a phone, with no API key, and it answers the question the page is actually
//! for, which is "where have my readings come from" rather than "what street is
//! this".
//!
//! ## The bounds fit the data, which means they must handle one point
//!
//! With a single location the span is zero, and every projection divides by it.
//! The QML widens a degenerate span by 0.01° before padding — about a
//! kilometre — so one pin renders in the middle of a sensible box rather than
//! producing a division by zero or a map zoomed to infinity. Two places in the
//! same building do the same thing, which is the case that actually happens:
//! "Home" and "Office" set from the same desk during setup.
//!
//! ## The 10% padding is not decoration
//!
//! Without it the extreme points sit exactly on the canvas edge, where half of
//! each dot is clipped and the outermost place — usually the interesting one —
//! is the hardest to see and to tap.
//!
//! ## Latitude runs up the screen and down the canvas
//!
//! The y flip is the one line that is wrong in every hand-rolled map. North is
//! up for the reader and increasing y is down for the renderer, so the
//! projection inverts once, here, rather than at each call site.

/// How far a degenerate span is widened, in degrees. About a kilometre.
pub const MIN_SPAN_DEG: f64 = 0.01;

/// Fraction of the span added on each side.
pub const PADDING: f64 = 0.10;

/// A point that can be plotted.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPoint {
    pub id: String,
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    /// A saved place rather than a recorded reading.
    pub is_location: bool,
}

/// The projected box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl Default for Bounds {
    /// The QML's initial box: a degree either side of the origin. Only used
    /// while `valid` is false, and never projected against.
    fn default() -> Self {
        Bounds { min_lat: -1.0, max_lat: 1.0, min_lon: -1.0, max_lon: 1.0 }
    }
}

impl Bounds {
    /// Fit a box to points, widening a degenerate span and padding the result.
    ///
    /// Returns `None` when there is nothing to fit — distinct from a box around
    /// nothing, because the caller must draw an empty state rather than an
    /// empty map. A map with axes and no dots looks like a failed load.
    pub fn fit(points: &[MapPoint]) -> Option<Bounds> {
        if points.is_empty() { return None; }
        let (mut lo_lat, mut hi_lat) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut lo_lon, mut hi_lon) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in points {
            // Non-finite coordinates are skipped rather than poisoning the box.
            // One NaN would make every bound NaN and every dot land nowhere.
            if !p.lat.is_finite() || !p.lon.is_finite() { continue; }
            lo_lat = lo_lat.min(p.lat); hi_lat = hi_lat.max(p.lat);
            lo_lon = lo_lon.min(p.lon); hi_lon = hi_lon.max(p.lon);
        }
        if !lo_lat.is_finite() || !lo_lon.is_finite() { return None; }

        // One point, or several in the same building — "Home" and "Office" set
        // from the same desk during setup is the case that actually happens.
        if hi_lat - lo_lat < 1e-6 { lo_lat -= MIN_SPAN_DEG; hi_lat += MIN_SPAN_DEG; }
        if hi_lon - lo_lon < 1e-6 { lo_lon -= MIN_SPAN_DEG; hi_lon += MIN_SPAN_DEG; }

        let pad_lat = (hi_lat - lo_lat) * PADDING;
        let pad_lon = (hi_lon - lo_lon) * PADDING;
        Some(Bounds { min_lat: lo_lat - pad_lat, max_lat: hi_lat + pad_lat,
                      min_lon: lo_lon - pad_lon, max_lon: hi_lon + pad_lon })
    }

    /// Project a coordinate into canvas pixels.
    ///
    /// The y axis is flipped exactly once, here. North is up for the reader and
    /// increasing y is down for the renderer, and doing that inversion at each
    /// call site is how half a map ends up mirrored.
    pub fn project(&self, lat: f64, lon: f64, w: f32, h: f32) -> (f32, f32) {
        let span_lat = self.max_lat - self.min_lat;
        let span_lon = self.max_lon - self.min_lon;
        // A zero span here would mean fit() was bypassed. Centre rather than
        // divide by zero, matching the QML's own guard.
        if span_lat.abs() < 1e-12 || span_lon.abs() < 1e-12 {
            return (w / 2.0, h / 2.0);
        }
        let nx = (lon - self.min_lon) / span_lon;
        let ny = (lat - self.min_lat) / span_lat;
        (nx as f32 * w, (1.0 - ny as f32) * h)
    }

    /// Reverse a canvas position back to a coordinate, for hit-testing taps.
    pub fn unproject(&self, x: f32, y: f32, w: f32, h: f32) -> (f64, f64) {
        if w <= 0.0 || h <= 0.0 { return (self.min_lat, self.min_lon); }
        let nx = (x / w) as f64;
        let ny = 1.0 - (y / h) as f64;
        (self.min_lat + ny * (self.max_lat - self.min_lat),
         self.min_lon + nx * (self.max_lon - self.min_lon))
    }
}

/// How close a tap must be to a dot, in pixels.
///
/// Generous for the same reason the glove cursor's snap radius is: a dot is a
/// few pixels across and a fingertip is not.
pub const HIT_RADIUS_PX: f32 = 22.0;

/// The page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapPage {
    points: Vec<MapPoint>,
    bounds: Option<Bounds>,
    pub selected: Option<String>,
}

pub const EMPTY_TEXT: &str = "No mapped locations yet.";

impl MapPage {
    pub fn set(&mut self, points: Vec<MapPoint>) {
        self.bounds = Bounds::fit(&points);
        // A selection that is no longer on the map is cleared, or the side
        // panel would describe a place that has gone.
        if let Some(sel) = &self.selected {
            if !points.iter().any(|p| &p.id == sel) { self.selected = None; }
        }
        self.points = points;
    }

    pub fn points(&self) -> &[MapPoint] { &self.points }
    pub fn bounds(&self) -> Option<Bounds> { self.bounds }
    pub fn is_empty(&self) -> bool { self.points.is_empty() }

    pub fn selected_point(&self) -> Option<&MapPoint> {
        let id = self.selected.as_ref()?;
        self.points.iter().find(|p| &p.id == id)
    }

    /// Which point a tap landed on, if any.
    ///
    /// The NEAREST within the radius rather than the first found, because dots
    /// overlap when two places are close and the first in the list is an
    /// arbitrary choice the user cannot see.
    pub fn hit(&self, x: f32, y: f32, w: f32, h: f32) -> Option<&MapPoint> {
        let b = self.bounds?;
        self.points.iter()
            .map(|p| {
                let (px, py) = b.project(p.lat, p.lon, w, h);
                (p, (px - x).hypot(py - y))
            })
            .filter(|(_, d)| *d <= HIT_RADIUS_PX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(p, _)| p)
    }

    /// Tap the canvas. Tapping empty space CLEARS the selection.
    ///
    /// Deliberate: on a map with no close button, tapping away is the only
    /// gesture that obviously means "never mind".
    pub fn tap(&mut self, x: f32, y: f32, w: f32, h: f32) -> Option<String> {
        let hit = self.hit(x, y, w, h).map(|p| p.id.clone());
        self.selected = hit.clone();
        hit
    }
}

/// Draw the map.
pub fn map_ui(ui: &mut egui::Ui, p: &mut MapPage, c: &uikit_egui::PaletteColors) {
    if p.is_empty() {
        ui.colored_label(c.text_weak, EMPTY_TEXT);
        return;
    }
    let size = egui::vec2(ui.available_width(), 260.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, c.bg_raised);

    if let Some(b) = p.bounds() {
        for pt in p.points() {
            let (x, y) = b.project(pt.lat, pt.lon, rect.width(), rect.height());
            let at = egui::pos2(rect.left() + x, rect.top() + y);
            let selected = p.selected.as_deref() == Some(pt.id.as_str());
            // Saved places are larger than recorded pins: there are far fewer
            // of them and they are what the user navigates by.
            let r = if pt.is_location { 6.0 } else { 3.5 };
            let col = if pt.is_location { c.accent } else { c.text_weak };
            painter.circle_filled(at, r, col);
            if selected {
                painter.circle_stroke(at, r + 4.0, egui::Stroke::new(2.0_f32, c.accent));
            }
        }
    }

    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.clicked() {
            p.tap(pos.x - rect.left(), pos.y - rect.top(), rect.width(), rect.height());
        }
    }

    if let Some(sel) = p.selected_point() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&sel.label).strong());
        ui.colored_label(c.text_weak, format!("{:.4}, {:.4}", sel.lat, sel.lon));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: &str, lat: f64, lon: f64) -> MapPoint {
        MapPoint { id: id.into(), label: id.into(), lat, lon, is_location: true }
    }

    #[test]
    fn one_point_still_produces_a_sensible_box() {
        // The span is zero and every projection divides by it. Widening by
        // about a kilometre puts the pin in the middle of a real box rather
        // than dividing by zero or zooming to infinity.
        let b = Bounds::fit(&[pt("home", 51.5, -0.12)]).expect("fits");
        assert!(b.max_lat > b.min_lat);
        assert!(b.max_lon > b.min_lon);
        let (x, y) = b.project(51.5, -0.12, 200.0, 100.0);
        assert!((x - 100.0).abs() < 1.0 && (y - 50.0).abs() < 1.0, "centred: {x},{y}");
    }

    #[test]
    fn two_places_in_the_same_building_do_not_collapse() {
        // The case that actually happens: "Home" and "Office" both set from the
        // same desk during setup.
        let b = Bounds::fit(&[pt("a", 51.5, -0.12), pt("b", 51.5, -0.12)]).expect("fits");
        assert!(b.max_lat - b.min_lat > MIN_SPAN_DEG);
    }

    #[test]
    fn the_box_is_padded_so_dots_are_not_clipped_at_the_edge() {
        // Without padding the outermost place — usually the interesting one —
        // is half clipped and the hardest to tap.
        let pts = [pt("a", 10.0, 10.0), pt("b", 20.0, 20.0)];
        let b = Bounds::fit(&pts).expect("fits");
        assert!(b.min_lat < 10.0 && b.max_lat > 20.0);
        let (_, y) = b.project(20.0, 20.0, 100.0, 100.0);
        assert!(y > 2.0, "the top point sits inside the canvas, at {y}");
    }

    #[test]
    fn latitude_runs_up_the_screen() {
        // The y flip: north is up for the reader, increasing y is down for the
        // renderer. Inverted once, here, rather than at each call site.
        let pts = [pt("s", 10.0, 10.0), pt("n", 20.0, 10.0)];
        let b = Bounds::fit(&pts).expect("fits");
        let (_, y_south) = b.project(10.0, 10.0, 100.0, 100.0);
        let (_, y_north) = b.project(20.0, 10.0, 100.0, 100.0);
        assert!(y_north < y_south, "north must be higher on the canvas");
    }

    #[test]
    fn longitude_runs_right() {
        let pts = [pt("w", 10.0, 10.0), pt("e", 10.0, 20.0)];
        let b = Bounds::fit(&pts).expect("fits");
        let (x_w, _) = b.project(10.0, 10.0, 100.0, 100.0);
        let (x_e, _) = b.project(10.0, 20.0, 100.0, 100.0);
        assert!(x_e > x_w);
    }

    #[test]
    fn no_points_means_no_bounds_rather_than_a_box_around_nothing() {
        // The caller must draw an empty state: a map with axes and no dots
        // looks like a failed load.
        assert!(Bounds::fit(&[]).is_none());
    }

    #[test]
    fn a_non_finite_coordinate_is_skipped_rather_than_poisoning_the_box() {
        // One NaN would make every bound NaN and every dot land nowhere.
        let pts = [pt("good", 10.0, 10.0), pt("bad", f64::NAN, 12.0)];
        let b = Bounds::fit(&pts).expect("the good point still fits");
        assert!(b.min_lat.is_finite() && b.max_lat.is_finite());
    }

    #[test]
    fn all_points_non_finite_is_the_same_as_none() {
        let pts = [pt("bad", f64::NAN, f64::NAN)];
        assert!(Bounds::fit(&pts).is_none());
    }

    #[test]
    fn a_degenerate_box_centres_rather_than_dividing_by_zero() {
        // Only reachable if fit() were bypassed, but the QML guards it and so
        // does this.
        let b = Bounds { min_lat: 5.0, max_lat: 5.0, min_lon: 5.0, max_lon: 5.0 };
        assert_eq!(b.project(5.0, 5.0, 200.0, 100.0), (100.0, 50.0));
    }

    #[test]
    fn projecting_and_unprojecting_round_trips() {
        let pts = [pt("a", 10.0, 10.0), pt("b", 20.0, 20.0)];
        let b = Bounds::fit(&pts).expect("fits");
        let (x, y) = b.project(15.0, 16.0, 300.0, 200.0);
        let (lat, lon) = b.unproject(x, y, 300.0, 200.0);
        assert!((lat - 15.0).abs() < 0.01 && (lon - 16.0).abs() < 0.01);
    }

    #[test]
    fn a_tap_selects_the_nearest_dot_not_the_first_one() {
        // Dots overlap when two places are close, and the first in the list is
        // an arbitrary choice the user cannot see.
        let mut p = MapPage::default();
        p.set(vec![pt("far", 10.0, 10.0), pt("near", 20.0, 20.0)]);
        let b = p.bounds().unwrap();
        let (x, y) = b.project(20.0, 20.0, 300.0, 200.0);
        assert_eq!(p.tap(x + 2.0, y + 2.0, 300.0, 200.0).as_deref(), Some("near"));
    }

    #[test]
    fn tapping_empty_space_clears_the_selection() {
        // On a map with no close button, tapping away is the only gesture that
        // obviously means "never mind".
        let mut p = MapPage::default();
        p.set(vec![pt("a", 10.0, 10.0), pt("b", 20.0, 20.0)]);
        let b = p.bounds().unwrap();
        let (x, y) = b.project(10.0, 10.0, 300.0, 200.0);
        p.tap(x, y, 300.0, 200.0);
        assert!(p.selected.is_some());
        p.tap(150.0, 100.0, 300.0, 200.0);
        assert_eq!(p.selected, None);
    }

    #[test]
    fn a_selection_that_leaves_the_map_is_cleared() {
        // Otherwise the side panel describes a place that has gone.
        let mut p = MapPage::default();
        p.set(vec![pt("a", 10.0, 10.0), pt("b", 20.0, 20.0)]);
        let b = p.bounds().unwrap();
        let (x, y) = b.project(10.0, 10.0, 300.0, 200.0);
        p.tap(x, y, 300.0, 200.0);
        p.set(vec![pt("b", 20.0, 20.0)]);
        assert_eq!(p.selected, None);
        assert!(p.selected_point().is_none());
    }

    #[test]
    fn a_selection_that_survives_a_refresh_is_kept() {
        // A periodic reload must not deselect what the user is looking at.
        let mut p = MapPage::default();
        p.set(vec![pt("a", 10.0, 10.0), pt("b", 20.0, 20.0)]);
        let b = p.bounds().unwrap();
        let (x, y) = b.project(10.0, 10.0, 300.0, 200.0);
        p.tap(x, y, 300.0, 200.0);
        p.set(vec![pt("a", 10.0, 10.0), pt("b", 20.0, 20.0), pt("c", 30.0, 30.0)]);
        assert_eq!(p.selected.as_deref(), Some("a"));
    }

    #[test]
    fn an_empty_map_says_so() {
        let p = MapPage::default();
        assert!(p.is_empty());
        assert!(p.bounds().is_none());
        assert!(p.hit(10.0, 10.0, 100.0, 100.0).is_none());
    }
}
