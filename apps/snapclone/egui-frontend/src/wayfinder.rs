//! wayfinder — `WayfinderPage`. Personal cartography: your contacts ARE the
//! geography.
//!
//! The port's "Wayfinder" page drew group-voice controls — join / mute / push to
//! talk. Not a stub of the wayfinder, a different feature under its name, which
//! is worse than an empty page because nothing looks missing.
//!
//! What the Qt page actually is, from its own header: not a road map. A
//! wayfinder in the spirit of a hiking GPS or an astrolabe. You are the centre;
//! everyone else is placed by BEARING and LOG-SCALED DISTANCE. No tiles, no
//! basemap, no commercial cartographer deciding what matters.
//!
//! ## Why the distance scale is logarithmic
//!
//! Linear distance puts a friend across town and a friend across the country in
//! the same pixel. The anchor is 1km at half the canvas radius, and each decade
//! adds a quarter:
//!
//! ```text
//!   100m  → log10(0.1) = -1 → 0.25
//!   1km   →  0             → 0.50
//!   10km  →  1             → 0.75
//!   100km →  2             → 1.00  (clamped)
//! ```
//!
//! Clamped at both ends: 0.05 keeps someone in the same building off the centre
//! dot, and 1.0 keeps someone on another continent on the canvas instead of
//! projected into the margin.
//!
//! ## A missing fix is not a missing person
//!
//! If EITHER our own position or theirs is unknown, the contact still appears —
//! placed in a nearby cluster and labelled "Nearby" or "Offline" rather than
//! dropped. A contact list that silently omits people whose GPS has not settled
//! reads as contacts disappearing.

use std::f64::consts::PI;

const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// 1km sits at half the canvas radius.
pub const ANCHOR_M: f64 = 1000.0;
/// At most this many floating photos, or the page becomes a collage.
pub const MAX_FLOATING_PHOTOS: usize = 5;
/// A contact counts as "on the path" to another within this bearing spread.
pub const PATH_BEARING_TOLERANCE_DEG: f64 = 20.0;
/// …and only if it is meaningfully closer than the far one.
pub const PATH_DISTANCE_RATIO: f64 = 0.8;
/// The page refreshes online state and locations on this cadence.
pub const REFRESH_SECS: u64 = 30;

/// Initial bearing from one WGS-84 point to another. 0 = north, clockwise.
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let dlon = (lon2 - lon1).to_radians();
    let y = dlon.sin() * phi2.cos();
    let x = phi1.cos() * phi2.sin() - phi1.sin() * phi2.cos() * dlon.cos();
    let brg = y.atan2(x).to_degrees();
    if brg < 0.0 { brg + 360.0 } else { brg }
}

/// Great-circle distance in metres.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlam = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (dlam / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().clamp(0.0, 1.0).asin()
}

/// Eight-point compass name for a bearing.
///
/// The `+ 22.5` is what makes each name cover the 45° centred on it rather than
/// the 45° starting at it — without it, due north reads as "NE".
pub fn cardinal(bearing_deg: f64) -> &'static str {
    const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let b = bearing_deg.rem_euclid(360.0);
    DIRS[(((b + 22.5) / 45.0) as usize) % 8]
}

/// `formatDistance`: metres under a kilometre, one decimal under 100km, whole
/// kilometres beyond. Precision drops as distance grows because "47.3km" and
/// "47km" tell you the same thing, and the decimal is noise at that range.
pub fn format_distance(m: f64) -> String {
    if m < 1000.0 { format!("{}m", m as i64) }
    else if m < 100_000.0 { format!("{:.1}km", m / 1000.0) }
    else { format!("{}km", (m / 1000.0) as i64) }
}

/// Fraction of the canvas radius a distance projects to, 0.05..=1.0.
pub fn radius_fraction(dist_m: f64) -> f64 {
    if dist_m <= 0.0 { return 0.05; }
    (0.5 + 0.25 * (dist_m / ANCHOR_M).log10()).clamp(0.05, 1.0)
}

/// A point on the canvas, in units of the canvas radius from the centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point { pub x: f64, pub y: f64 }

/// Project a bearing and distance onto the canvas.
///
/// Screen y grows downward while bearing 0 is north, so north is -y. Getting
/// this backwards mirrors the whole map top-to-bottom and is invisible until
/// someone notices their northern neighbour is drawn south.
pub fn project(bearing_deg: f64, dist_m: f64) -> Point {
    let r = radius_fraction(dist_m);
    let theta = (bearing_deg - 90.0) * PI / 180.0;
    Point { x: r * theta.cos(), y: r * theta.sin() }
}

/// One contact on the wayfinder.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub display_name: String,
    pub online: bool,
    pub bearing_deg: f64,
    pub distance_m: f64,
    /// True when either fix is missing — placed in the nearby cluster.
    pub no_location: bool,
    pub pos: Point,
}

impl Node {
    /// The label under the dot: `1.2km NW`, or the status when there is no fix.
    pub fn distance_label(&self) -> String {
        if self.no_location {
            return if self.online { "Nearby".into() } else { "Offline".into() };
        }
        format!("{} {}", format_distance(self.distance_m), cardinal(self.bearing_deg))
    }
}

/// Where we are, if we know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix { pub lat: f64, pub lon: f64 }

/// A contact's known position, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub id: String,
    pub display_name: String,
    pub online: bool,
    pub loc: Option<Fix>,
}

/// Build the node set.
///
/// A contact with no position is kept, not dropped: a page that silently omits
/// people whose GPS has not settled reads as contacts disappearing.
pub fn build_nodes(self_fix: Option<Fix>, contacts: &[Contact]) -> Vec<Node> {
    contacts.iter().map(|c| {
        let name = if c.display_name.trim().is_empty() { c.id.clone() }
                   else { c.display_name.clone() };
        match (self_fix, c.loc) {
            (Some(me), Some(them)) => {
                let b = bearing_deg(me.lat, me.lon, them.lat, them.lon);
                let d = haversine_m(me.lat, me.lon, them.lat, them.lon);
                Node { id: c.id.clone(), display_name: name, online: c.online,
                       bearing_deg: b, distance_m: d, no_location: false,
                       pos: project(b, d) }
            }
            // EITHER fix missing, not both: knowing where they are is useless
            // without knowing where we are, so the branch is on the pair.
            _ => Node { id: c.id.clone(), display_name: name, online: c.online,
                        bearing_deg: 0.0, distance_m: 0.0, no_location: true,
                        pos: Point { x: 0.0, y: 0.0 } },
        }
    }).collect()
}

/// The distance rings, in metres — one per decade around the anchor.
pub fn rings() -> [f64; 4] { [100.0, 1_000.0, 10_000.0, 100_000.0] }

/// A snap floating on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatingPhoto {
    pub owner: String,
    pub caption: String,
    pub pos: Point,
    /// Inherited from the owner, and drives border thickness: close photos get
    /// thick borders (present, alive), far ones thin and ghostly.
    pub distance_fraction: f64,
}

/// Place snaps on the map.
///
/// Default is the midpoint between you and the sender. But when another contact
/// sits roughly on the path to them — within 20° of bearing and meaningfully
/// closer — the photo goes between the two contacts instead. That is what makes
/// the page read as terrain rather than as spokes from a hub: photos land along
/// the lines people actually lie on.
///
/// Capped at five. Every snap on screen at once is a collage, not a map.
pub fn place_photos(nodes: &[Node], senders: &[(String, String)]) -> Vec<FloatingPhoto> {
    let mut out = Vec::new();
    for (owner, caption) in senders {
        if out.len() >= MAX_FLOATING_PHOTOS { break; }
        let Some(node) = nodes.iter().find(|n| &n.id == owner) else { continue };
        let mut pos = Point { x: node.pos.x * 0.5, y: node.pos.y * 0.5 };
        for other in nodes {
            if other.id == node.id { continue; }
            let diff = ((other.bearing_deg - node.bearing_deg + 540.0).rem_euclid(360.0) - 180.0).abs();
            if diff < PATH_BEARING_TOLERANCE_DEG
                && other.distance_m < node.distance_m * PATH_DISTANCE_RATIO {
                pos = Point { x: (other.pos.x + node.pos.x) * 0.5,
                              y: (other.pos.y + node.pos.y) * 0.5 };
                break;
            }
        }
        out.push(FloatingPhoto {
            owner: owner.clone(), caption: caption.clone(), pos,
            distance_fraction: radius_fraction(node.distance_m),
        });
    }
    out
}

/// Draw the wayfinder. Returns a contact id when one is tapped, which the shell
/// turns into a jump to the friends page with them selected.
pub fn wayfinder_ui(ui: &mut egui::Ui, nodes: &[Node], photos: &[FloatingPhoto],
                    c: &uikit_egui::PaletteColors) -> Option<String> {
    let mut tapped = None;
    let avail = ui.available_size();
    let side = avail.x.min(avail.y).max(160.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    let centre = rect.center();
    // Margin for the cardinal labels and the distance text under each node.
    let radius = side * 0.5 - 24.0;
    let p = ui.painter();

    for (i, _m) in rings().iter().enumerate() {
        // Rings sit at the projected fraction of each decade, so they line up
        // with the nodes rather than being cosmetic circles.
        let frac = 0.25 + 0.25 * i as f64;
        p.circle_stroke(centre, radius * frac as f32, egui::Stroke::new(1.0, c.border));
    }
    for (label, ang) in [("N", -90.0f32), ("E", 0.0), ("S", 90.0), ("W", 180.0)] {
        let a = ang.to_radians();
        let at = centre + egui::vec2(a.cos(), a.sin()) * (radius + 12.0);
        p.text(at, egui::Align2::CENTER_CENTER, label,
               egui::FontId::proportional(11.0), c.text_weak);
    }
    p.circle_filled(centre, 4.0, c.accent);

    for ph in photos {
        let at = centre + egui::vec2(ph.pos.x as f32, ph.pos.y as f32) * radius;
        // Thickness carries presence: close is thick, far is ghostly.
        let w = 1.0 + 2.0 * (1.0 - ph.distance_fraction as f32);
        p.rect_stroke(egui::Rect::from_center_size(at, egui::vec2(28.0, 28.0)), 4.0,
                      egui::Stroke::new(w, c.text_weak), egui::StrokeKind::Inside);
    }

    for n in nodes {
        let at = centre + egui::vec2(n.pos.x as f32, n.pos.y as f32) * radius;
        p.circle_filled(at, 7.0, if n.online { c.success } else { c.text_weak });
        p.text(at + egui::vec2(0.0, 14.0), egui::Align2::CENTER_TOP,
               &n.display_name, egui::FontId::proportional(11.0), c.text);
        p.text(at + egui::vec2(0.0, 26.0), egui::Align2::CENTER_TOP,
               n.distance_label(), egui::FontId::proportional(10.0), c.text_weak);
        if let Some(click) = resp.interact_pointer_pos() {
            if click.distance(at) < 14.0 && resp.clicked() { tapped = Some(n.id.clone()); }
        }
    }
    tapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, eps: f64) -> bool { (a - b).abs() < eps }

    fn contact(id: &str, lat: f64, lon: f64) -> Contact {
        Contact { id: id.into(), display_name: id.into(), online: true,
                  loc: Some(Fix { lat, lon }) }
    }

    #[test]
    fn bearing_points_the_right_way_at_the_four_cardinals() {
        // From the equator/prime-meridian origin.
        assert!(near(bearing_deg(0.0, 0.0, 1.0, 0.0), 0.0, 0.01), "north");
        assert!(near(bearing_deg(0.0, 0.0, 0.0, 1.0), 90.0, 0.01), "east");
        assert!(near(bearing_deg(0.0, 0.0, -1.0, 0.0), 180.0, 0.01), "south");
        assert!(near(bearing_deg(0.0, 0.0, 0.0, -1.0), 270.0, 0.01), "west");
    }

    #[test]
    fn bearing_is_never_negative() {
        // atan2 returns -180..180; a negative bearing would index the compass
        // table out of range and place a westerly contact off the canvas.
        for lon in [-179.0, -90.0, -1.0, 1.0, 90.0, 179.0] {
            let b = bearing_deg(51.5, -0.12, 51.5, lon);
            assert!((0.0..360.0).contains(&b), "bearing {b} out of range for lon {lon}");
        }
    }

    #[test]
    fn haversine_matches_known_distances() {
        // London to Paris, ~344km.
        let d = haversine_m(51.5074, -0.1278, 48.8566, 2.3522);
        assert!(near(d / 1000.0, 344.0, 3.0), "got {:.1}km", d / 1000.0);
        // One degree of latitude is ~111km anywhere.
        assert!(near(haversine_m(0.0, 0.0, 1.0, 0.0) / 1000.0, 111.19, 0.5));
    }

    #[test]
    fn the_same_point_is_zero_distance_and_does_not_produce_nan() {
        // asin of a hair over 1.0 from floating-point error is NaN, and a NaN
        // distance propagates into the projection and loses the node entirely.
        let d = haversine_m(51.5, -0.12, 51.5, -0.12);
        assert!(d.is_finite());
        assert!(near(d, 0.0, 0.001));
    }

    #[test]
    fn antipodal_points_do_not_produce_nan_either() {
        let d = haversine_m(0.0, 0.0, 0.0, 180.0);
        assert!(d.is_finite(), "antipodes gave {d}");
        assert!(near(d / 1000.0, 20_015.0, 50.0));
    }

    #[test]
    fn the_compass_centres_each_name_on_its_direction() {
        // Without the +22.5 offset, due north reads as "NE".
        assert_eq!(cardinal(0.0), "N");
        assert_eq!(cardinal(359.0), "N");
        assert_eq!(cardinal(22.0), "N");
        assert_eq!(cardinal(23.0), "NE");
        assert_eq!(cardinal(90.0), "E");
        assert_eq!(cardinal(180.0), "S");
        assert_eq!(cardinal(315.0), "NW");
    }

    #[test]
    fn the_compass_survives_a_bearing_outside_zero_to_three_sixty() {
        assert_eq!(cardinal(-90.0), "W");
        assert_eq!(cardinal(450.0), "E");
    }

    #[test]
    fn distance_loses_precision_as_it_grows_because_the_decimal_stops_meaning_anything() {
        assert_eq!(format_distance(0.0), "0m");
        assert_eq!(format_distance(999.0), "999m");
        assert_eq!(format_distance(1000.0), "1.0km");
        assert_eq!(format_distance(1234.0), "1.2km");
        assert_eq!(format_distance(99_900.0), "99.9km");
        assert_eq!(format_distance(150_000.0), "150km");
    }

    #[test]
    fn the_log_scale_puts_each_decade_a_quarter_further_out() {
        assert!(near(radius_fraction(100.0), 0.25, 0.001));
        assert!(near(radius_fraction(1_000.0), 0.50, 0.001));
        assert!(near(radius_fraction(10_000.0), 0.75, 0.001));
        assert!(near(radius_fraction(100_000.0), 1.00, 0.001));
    }

    #[test]
    fn the_scale_clamps_at_both_ends() {
        // Someone in the same building must not sit under the centre dot, and
        // someone on another continent must stay on the canvas.
        assert!(near(radius_fraction(0.5), 0.05, 0.001), "same building");
        assert!(near(radius_fraction(0.0), 0.05, 0.001), "zero distance");
        assert!(near(radius_fraction(20_000_000.0), 1.0, 0.001), "antipodes");
    }

    #[test]
    fn north_projects_upward_because_screen_y_grows_downward() {
        // Getting this backwards mirrors the map and is invisible until someone
        // notices their northern neighbour drawn to the south.
        let n = project(0.0, ANCHOR_M);
        assert!(near(n.x, 0.0, 0.001));
        assert!(n.y < 0.0, "north should be -y, got {}", n.y);
        let e = project(90.0, ANCHOR_M);
        assert!(e.x > 0.0 && near(e.y, 0.0, 0.001), "east should be +x");
        let s = project(180.0, ANCHOR_M);
        assert!(s.y > 0.0, "south should be +y");
    }

    #[test]
    fn a_contact_with_no_fix_is_kept_and_labelled_rather_than_dropped() {
        // A page that silently omits people whose GPS has not settled reads as
        // contacts disappearing.
        let me = Some(Fix { lat: 51.5, lon: -0.12 });
        let cs = vec![
            Contact { id: "ada".into(), display_name: "Ada".into(), online: true, loc: None },
            Contact { id: "bo".into(), display_name: "Bo".into(), online: false, loc: None },
        ];
        let nodes = build_nodes(me, &cs);
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.no_location));
        assert_eq!(nodes[0].distance_label(), "Nearby");
        assert_eq!(nodes[1].distance_label(), "Offline", "offline outranks nearby");
    }

    #[test]
    fn no_fix_of_our_own_means_nobody_is_placed_geographically() {
        // Knowing where they are is useless without knowing where we are.
        let nodes = build_nodes(None, &[contact("ada", 48.85, 2.35)]);
        assert!(nodes[0].no_location);
        assert_eq!(nodes[0].pos, Point { x: 0.0, y: 0.0 });
    }

    #[test]
    fn a_located_contact_gets_a_bearing_distance_and_label() {
        let me = Some(Fix { lat: 51.5074, lon: -0.1278 });
        let nodes = build_nodes(me, &[contact("paris", 48.8566, 2.3522)]);
        let n = &nodes[0];
        assert!(!n.no_location);
        assert!(n.distance_label().ends_with("SE"), "Paris is SE of London: {}", n.distance_label());
        assert!(n.distance_label().starts_with("343km"), "got {}", n.distance_label());
    }

    #[test]
    fn an_unnamed_contact_falls_back_to_its_id() {
        let c = Contact { id: "sc-abcd".into(), display_name: "  ".into(),
                          online: true, loc: None };
        assert_eq!(build_nodes(None, &[c])[0].display_name, "sc-abcd");
    }

    #[test]
    fn a_photo_defaults_to_the_midpoint_between_you_and_the_sender() {
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let nodes = build_nodes(me, &[contact("ada", 0.0, 1.0)]);
        let photos = place_photos(&nodes, &[("ada".into(), "Ada".into())]);
        assert_eq!(photos.len(), 1);
        assert!(near(photos[0].pos.x, nodes[0].pos.x / 2.0, 1e-9));
        assert!(near(photos[0].pos.y, nodes[0].pos.y / 2.0, 1e-9));
    }

    #[test]
    fn a_contact_on_the_path_pulls_the_photo_between_the_two_of_them() {
        // This is what makes the page read as terrain rather than spokes from a
        // hub: photos land along the lines people actually lie on.
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let nodes = build_nodes(me, &[
            contact("far", 0.0, 1.0),      // due east, ~111km
            contact("near", 0.0, 0.2),     // due east, ~22km — same bearing, closer
        ]);
        let photos = place_photos(&nodes, &[("far".into(), "Far".into())]);
        let far = nodes.iter().find(|n| n.id == "far").unwrap();
        let near_n = nodes.iter().find(|n| n.id == "near").unwrap();
        assert!(near(photos[0].pos.x, (far.pos.x + near_n.pos.x) / 2.0, 1e-9),
                "should sit between the two contacts, not between you and the far one");
    }

    #[test]
    fn a_contact_off_the_path_does_not_pull_the_photo() {
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let nodes = build_nodes(me, &[
            contact("east", 0.0, 1.0),
            contact("north", 0.5, 0.0),    // 90° away — not on the path
        ]);
        let photos = place_photos(&nodes, &[("east".into(), "East".into())]);
        let east = nodes.iter().find(|n| n.id == "east").unwrap();
        assert!(near(photos[0].pos.x, east.pos.x / 2.0, 1e-9));
    }

    #[test]
    fn a_contact_that_is_barely_closer_does_not_count_as_between() {
        // The 0.8 ratio: two people at nearly the same distance are beside each
        // other, not one on the way to the other.
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let nodes = build_nodes(me, &[
            contact("far", 0.0, 1.0),
            contact("almost", 0.0, 0.95),
        ]);
        let photos = place_photos(&nodes, &[("far".into(), "Far".into())]);
        let far = nodes.iter().find(|n| n.id == "far").unwrap();
        assert!(near(photos[0].pos.x, far.pos.x / 2.0, 1e-9));
    }

    #[test]
    fn floating_photos_are_capped_so_the_map_stays_a_map() {
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let cs: Vec<Contact> = (0..10)
            .map(|i| contact(&format!("c{i}"), 0.0, 0.1 * (i + 1) as f64)).collect();
        let nodes = build_nodes(me, &cs);
        let senders: Vec<(String, String)> = (0..10)
            .map(|i| (format!("c{i}"), format!("C{i}"))).collect();
        assert_eq!(place_photos(&nodes, &senders).len(), MAX_FLOATING_PHOTOS);
    }

    #[test]
    fn a_snap_from_someone_not_on_the_page_is_skipped_not_placed_at_the_origin() {
        let nodes = build_nodes(Some(Fix { lat: 0.0, lon: 0.0 }), &[contact("ada", 0.0, 1.0)]);
        let photos = place_photos(&nodes, &[("ghost".into(), "Ghost".into())]);
        assert!(photos.is_empty(), "a photo at the centre would read as ours");
    }

    #[test]
    fn photo_thickness_tracks_the_owners_distance() {
        // Close photos are thick and present; far ones thin and ghostly.
        let me = Some(Fix { lat: 0.0, lon: 0.0 });
        let nodes = build_nodes(me, &[contact("near", 0.0, 0.005), contact("far", 0.0, 5.0)]);
        let p = place_photos(&nodes, &[("near".into(), "N".into()), ("far".into(), "F".into())]);
        assert!(p[0].distance_fraction < p[1].distance_fraction);
    }

    #[test]
    fn the_rings_are_the_decades_the_scale_is_built_on() {
        for (i, m) in rings().iter().enumerate() {
            assert!(near(radius_fraction(*m), 0.25 + 0.25 * i as f64, 0.001));
        }
    }
}
