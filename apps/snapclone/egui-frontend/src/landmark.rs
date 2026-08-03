//! landmark — `AddLandmarkDialog`. Naming a place you can be found at.
//!
//! The wayfinder places people. Landmarks are the other half: a named point you
//! can be resolved to — "my apartment", "the studio" — so a contact can be
//! matched to a place without either of you sharing a live position.
//!
//! ## V1 is deliberately small
//!
//! Name and current location. No photo picker, no scope picker; every landmark
//! is `Private`. That is the Qt header's own scoping and it is kept, because
//! the alternative — shipping a scope control that only has one working value —
//! promises a choice the storage layer cannot honour yet.
//!
//! ## No fix, no save
//!
//! The caller pre-resolves the position from `lastKnownLocation`. If it is NaN
//! the dialog REFUSES rather than saving a landmark at (0, 0) — null island,
//! off the coast of Ghana, which is where every dropped coordinate in the world
//! ends up. It says where to go instead: Settings → Privacy → Manual location.
//! An error that names the fix is the difference between a dead end and a
//! detour.
//!
//! ## The photo never leaves the dialog
//!
//! A captured frame is hashed and the HASHES go on the record; the pixels do
//! not, anywhere. That is the Qt comment's point and it is a real distinction:
//! a landmark is a thing you sync, and a photo of your front door is not
//! something to sync just because it helped you recognise the place.
//!
//! ## geohash7, and why it is coarse on purpose
//!
//! The record documents `geohash7` as the DHT lookup key for public landmarks,
//! at ~7 characters / ~76m, with the coarseness called deliberate: it prevents
//! fingerprinting "this exact house" from a query. Qt leaves the field empty in
//! V31, pending "a future helper". This is that helper — the encoder plus tests
//! pinning the cell size the privacy claim rests on, since a geohash7 that
//! silently resolved to 7m instead of 76m would break the guarantee while
//! looking correct.

/// Base-32 alphabet for geohash: digits and lowercase, minus a, i, l, o.
const GEOHASH32: &[u8] = b"0123456789bcdefghjkmnpqrstuvwxyz";
/// Characters of precision. ~76m cell, and that is the privacy budget.
pub const GEOHASH_PRECISION: usize = 7;

/// Encode a WGS-84 position to a geohash of `GEOHASH_PRECISION` characters.
pub fn geohash7(lat: f64, lon: f64) -> String {
    let (mut lat_lo, mut lat_hi) = (-90.0f64, 90.0f64);
    let (mut lon_lo, mut lon_hi) = (-180.0f64, 180.0f64);
    let mut out = String::with_capacity(GEOHASH_PRECISION);
    let (mut bit, mut ch, mut even) = (0u8, 0usize, true);
    while out.len() < GEOHASH_PRECISION {
        if even {
            let mid = (lon_lo + lon_hi) / 2.0;
            if lon > mid { ch = (ch << 1) | 1; lon_lo = mid; } else { ch <<= 1; lon_hi = mid; }
        } else {
            let mid = (lat_lo + lat_hi) / 2.0;
            if lat > mid { ch = (ch << 1) | 1; lat_lo = mid; } else { ch <<= 1; lat_hi = mid; }
        }
        even = !even;
        bit += 1;
        if bit == 5 {
            out.push(GEOHASH32[ch] as char);
            bit = 0;
            ch = 0;
        }
    }
    out
}

/// Who can see a landmark. Only one value is reachable in V1, and the enum
/// exists anyway so the storage record's shape is honest about what is coming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope { #[default] Private, Public }

/// The record the dialog produces. The dialog does NOT persist it — the parent
/// page does, so the dialog stays stateless and has no storage access.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    pub id: String,
    pub creator_id: String,
    pub display_name: String,
    pub lat: f64,
    pub lon: f64,
    pub accuracy_m: f64,
    /// Populated only for `Public` landmarks: it is a DHT lookup key, and a
    /// private landmark that is never looked up has no reason to carry one.
    pub geohash7: String,
    pub scope: Scope,
    pub created_at_ms: i64,
    /// Lossy one-way hashes of a captured frame. Never the pixels.
    pub photo_hashes: Vec<String>,
}

/// A position, or the absence of one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix { pub lat: f64, pub lon: f64, pub accuracy_m: f64 }

impl Fix {
    /// NaN is how the caller reports "no fix", so it is checked rather than
    /// trusted. A landmark saved from NaN lands at null island.
    pub fn valid(&self) -> bool { self.lat.is_finite() && self.lon.is_finite() }
}

pub const NO_FIX_HINT: &str =
    "Set your location first — Settings → Privacy → Manual location";
pub const NAME_REQUIRED: &str = "Give the landmark a name.";

/// The dialog.
#[derive(Debug, Clone)]
pub struct AddLandmark {
    pub name: String,
    pub fix: Option<Fix>,
    pub error: Option<String>,
    /// Hashes of an attached frame, if one was captured.
    pub photo_hashes: Vec<String>,
}

impl AddLandmark {
    pub fn new(fix: Option<Fix>) -> Self {
        // A NaN-carrying fix is no fix. Normalising here means every later
        // check is a simple Option test rather than a NaN test somebody forgets.
        let fix = fix.filter(|f| f.valid());
        AddLandmark { name: String::new(), fix, error: None, photo_hashes: Vec::new() }
    }

    pub fn have_fix(&self) -> bool { self.fix.is_some() }

    /// Save is enabled only with a position. The button is disabled rather than
    /// hidden, with the hint as its tooltip: a missing button reads as a
    /// missing feature, a disabled one reads as a missing precondition.
    pub fn can_save(&self) -> bool { self.have_fix() }

    pub fn save_hint(&self) -> Option<&'static str> {
        if self.have_fix() { None } else { Some(NO_FIX_HINT) }
    }

    /// Attach a captured frame's hashes. The frame itself is not stored.
    pub fn attach_photo_hashes(&mut self, hashes: Vec<String>) {
        self.photo_hashes = hashes;
    }

    /// Build the record. `id` and `now_ms` come from the host so this stays
    /// pure and testable — a dialog that reaches for a clock and a UUID source
    /// cannot be tested without both.
    pub fn save(&mut self, creator_id: &str, id: &str, now_ms: i64) -> Option<Landmark> {
        let Some(fix) = self.fix else {
            self.error = Some(NO_FIX_HINT.into());
            return None;
        };
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.error = Some(NAME_REQUIRED.into());
            return None;
        }
        self.error = None;
        let scope = Scope::Private;   // V1: every landmark is private.
        Some(Landmark {
            id: format!("sc-landmark-{id}"),
            creator_id: creator_id.to_string(),
            display_name: name,
            lat: fix.lat,
            lon: fix.lon,
            accuracy_m: fix.accuracy_m,
            // Empty for Private, which is every V1 landmark: the geohash is a
            // DHT lookup key and a landmark nobody looks up does not need one.
            // Writing it anyway would put a coarse location into a record that
            // had no reason to carry it.
            geohash7: if scope == Scope::Public { geohash7(fix.lat, fix.lon) } else { String::new() },
            scope,
            created_at_ms: now_ms,
            photo_hashes: std::mem::take(&mut self.photo_hashes),
        })
    }
}

/// Draw the dialog. Returns the record once it is built.
pub fn add_landmark_ui(ui: &mut egui::Ui, d: &mut AddLandmark, creator_id: &str,
                       id: &str, now_ms: i64,
                       c: &uikit_egui::PaletteColors) -> Option<Landmark> {
    let mut out = None;
    ui.vertical(|ui| {
        ui.strong("Add landmark");
        ui.add(egui::TextEdit::singleline(&mut d.name)
            .hint_text("Name, e.g. My apartment").desired_width(240.0));

        match d.fix {
            Some(f) => ui.colored_label(c.text_weak,
                format!("{:.5}, {:.5}  ±{:.0}m", f.lat, f.lon, f.accuracy_m)),
            // The hint names the exact path rather than saying "no location",
            // which is the difference between a dead end and a detour.
            None => ui.colored_label(c.warning, NO_FIX_HINT),
        };

        if !d.photo_hashes.is_empty() {
            ui.colored_label(c.text_weak,
                format!("{} photo fingerprint(s) attached — the image is not saved",
                        d.photo_hashes.len()));
        }

        let btn = ui.add_enabled(d.can_save(), egui::Button::new("Save"));
        if let Some(h) = d.save_hint() { btn.clone().on_disabled_hover_text(h); }
        if btn.clicked() { out = d.save(creator_id, id, now_ms); }
        if let Some(e) = &d.error { ui.colored_label(c.danger, e); }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Fix { Fix { lat: 51.5074, lon: -0.1278, accuracy_m: 12.0 } }

    #[test]
    fn a_landmark_needs_a_position_and_says_where_to_get_one() {
        // Saving from a missing fix lands the landmark at null island, off the
        // coast of Ghana, where every dropped coordinate ends up.
        let mut d = AddLandmark::new(None);
        d.name = "Home".into();
        assert!(!d.can_save());
        assert!(d.save("sc-me", "u1", 0).is_none());
        assert_eq!(d.save_hint(), Some(NO_FIX_HINT));
        assert!(NO_FIX_HINT.contains("Settings"), "the hint names the path, not just the problem");
    }

    #[test]
    fn a_nan_fix_is_no_fix() {
        // NaN is how the caller reports "no location", so it is checked rather
        // than trusted into the record.
        for bad in [Fix { lat: f64::NAN, lon: 0.0, accuracy_m: 1.0 },
                    Fix { lat: 0.0, lon: f64::NAN, accuracy_m: 1.0 },
                    Fix { lat: f64::INFINITY, lon: 0.0, accuracy_m: 1.0 }] {
            let d = AddLandmark::new(Some(bad));
            assert!(!d.have_fix(), "{bad:?} should not count as a fix");
        }
    }

    #[test]
    fn a_landmark_needs_a_name() {
        let mut d = AddLandmark::new(Some(fix()));
        assert!(d.save("sc-me", "u1", 0).is_none());
        assert_eq!(d.error.as_deref(), Some(NAME_REQUIRED));
        d.name = "   ".into();
        assert!(d.save("sc-me", "u1", 0).is_none(), "whitespace is not a name");
    }

    #[test]
    fn the_name_is_trimmed() {
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "  My apartment  ".into();
        assert_eq!(d.save("sc-me", "u1", 0).unwrap().display_name, "My apartment");
    }

    #[test]
    fn a_saved_landmark_carries_the_creator_the_position_and_the_accuracy() {
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "Studio".into();
        let lm = d.save("sc-me", "abcd", 1_700_000_000_000).unwrap();
        assert_eq!(lm.id, "sc-landmark-abcd");
        assert_eq!(lm.creator_id, "sc-me");
        assert_eq!(lm.lat, 51.5074);
        assert_eq!(lm.accuracy_m, 12.0, "the accuracy at capture is part of the record");
        assert_eq!(lm.created_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn every_v1_landmark_is_private() {
        // Shipping a scope picker with one working value promises a choice the
        // storage layer cannot honour yet.
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "Home".into();
        assert_eq!(d.save("sc-me", "u1", 0).unwrap().scope, Scope::Private);
    }

    #[test]
    fn a_private_landmark_carries_no_geohash() {
        // The geohash is a DHT lookup key. A landmark nobody looks up has no
        // reason to carry a coarse location it never needed.
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "Home".into();
        assert!(d.save("sc-me", "u1", 0).unwrap().geohash7.is_empty());
    }

    #[test]
    fn a_captured_photo_contributes_hashes_and_never_pixels() {
        // A landmark is a thing you sync; a photo of your front door is not
        // something to sync just because it helped you recognise the place.
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "Home".into();
        d.attach_photo_hashes(vec!["ph1".into(), "ph2".into()]);
        let lm = d.save("sc-me", "u1", 0).unwrap();
        assert_eq!(lm.photo_hashes, vec!["ph1".to_string(), "ph2".to_string()]);
        let dbg = format!("{lm:?}");
        assert!(!dbg.contains("pixels") && !dbg.contains("image"));
    }

    #[test]
    fn saving_twice_does_not_reattach_the_same_photo() {
        // The hashes belong to the record that was built, not to the dialog.
        let mut d = AddLandmark::new(Some(fix()));
        d.name = "Home".into();
        d.attach_photo_hashes(vec!["ph1".into()]);
        assert_eq!(d.save("sc-me", "u1", 0).unwrap().photo_hashes.len(), 1);
        d.name = "Home 2".into();
        assert!(d.save("sc-me", "u2", 0).unwrap().photo_hashes.is_empty());
    }

    #[test]
    fn a_successful_save_clears_a_previous_error() {
        let mut d = AddLandmark::new(Some(fix()));
        assert!(d.save("sc-me", "u1", 0).is_none());
        assert!(d.error.is_some());
        d.name = "Home".into();
        assert!(d.save("sc-me", "u1", 0).is_some());
        assert!(d.error.is_none());
    }

    // ── geohash ─────────────────────────────────────────────────────────────

    #[test]
    fn geohash_matches_the_published_reference_values() {
        // Checked against the canonical examples rather than against itself: a
        // geohash that is subtly wrong points DHT lookups at the wrong cell,
        // and the failure looks like "nobody is ever nearby".
        assert_eq!(geohash7(57.64911, 10.40744), "u4pruyd");
        assert_eq!(geohash7(51.5074, -0.1278), "gcpvj0d");
    }

    #[test]
    fn a_point_exactly_on_a_cell_boundary_resolves_downward_consistently() {
        // (0,0) sits on every division line at once, and implementations
        // disagree about which side of a boundary a value ON the line falls.
        // This one uses `> mid`, so it takes the lower half every time and
        // lands at the top of the south-west cell. Pinned rather than left to
        // chance: two peers computing different keys for the same landmark
        // would each look up a cell the other never writes to, and the failure
        // presents as "nobody is ever nearby".
        assert_eq!(geohash7(0.0, 0.0), "7zzzzzz");
        // A hair either side lands where you would expect.
        assert!(geohash7(0.0001, 0.0001).starts_with('s'));
    }

    #[test]
    fn geohash_is_exactly_seven_characters() {
        for (lat, lon) in [(90.0, 180.0), (-90.0, -180.0), (0.0, 0.0), (51.5, -0.12)] {
            assert_eq!(geohash7(lat, lon).len(), GEOHASH_PRECISION, "at {lat},{lon}");
        }
    }

    #[test]
    fn the_cell_is_coarse_enough_to_keep_its_privacy_promise() {
        // THE property the record's comment rests on: ~76m, deliberately too
        // coarse to fingerprint "this exact house". A geohash7 that silently
        // resolved to 7m would break the guarantee while looking correct.
        //
        // Two points ~30m apart should usually share a cell; two ~500m apart
        // must not.
        let base = geohash7(51.50740, -0.12780);
        let close = geohash7(51.50766, -0.12780);        // ~29m north
        let far = geohash7(51.51190, -0.12780);          // ~500m north
        assert_eq!(base, close, "30m apart should land in the same ~76m cell");
        assert_ne!(base, far, "500m apart must not");
    }

    #[test]
    fn neighbouring_cells_share_a_prefix() {
        // The property that makes a geohash useful as a lookup key at all.
        let a = geohash7(51.5074, -0.1278);
        let b = geohash7(51.5075, -0.1279);
        assert_eq!(&a[..5], &b[..5]);
    }

    #[test]
    fn the_alphabet_omits_the_ambiguous_letters() {
        // a, i, l and o are excluded so a geohash read aloud or typed by hand
        // cannot be confused with 1 and 0.
        let s: String = (0..32).map(|i| GEOHASH32[i] as char).collect();
        for c in ['a', 'i', 'l', 'o'] {
            assert!(!s.contains(c), "{c} should not be in the alphabet");
        }
        assert_eq!(s.len(), 32);
    }
}
