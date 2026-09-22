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

/// Resolve a product type to its profile, the QML's three-step chain.
///
/// ```text
/// productMap[product_type] || productMap["Unknown Type"] || { …hardcoded… }
/// ```
///
/// All three steps matter and they are not interchangeable:
///
/// 1. **The product's own entry**, when the daemon sent one.
/// 2. **The map's own `"Unknown Type"`**, which lets a deployment describe
///    what an unrecognised device should look like without a rebuild. Skipping
///    to the hardcoded default would ignore that.
/// 3. **The hardcoded fallback** ([`Profile::default`]), for when the map is
///    empty or unparseable — which the QML treats as normal, not as an error:
///    its `JSON.parse` is wrapped in a `try` returning `{}`.
///
/// Takes an already-parsed map rather than a JSON string. The QML parses
/// because it is handed one by the daemon; this crate has no JSON dependency
/// and adding one to reach a lookup would be the wrong trade. Whoever plumbs
/// the daemon in parses at that boundary, which is also where a parse failure
/// can be reported rather than silently becoming `{}`.
pub fn resolve_profile(
    map: &std::collections::BTreeMap<String, Profile>,
    product_type: Option<&str>,
) -> Profile {
    if let Some(p) = product_type.and_then(|t| map.get(t)) {
        return p.clone();
    }
    if let Some(p) = map.get("Unknown Type") {
        return p.clone();
    }
    Profile::default()
}

/// The control a top-level view needs, or `None` for one every product has.
///
/// The QML drops a page from the swipe set entirely when a product lacks its
/// control — "headphones don't get a Stream page, TVs don't get auto-route".
/// This frontend has six views where the QML had three and shows all six on
/// every device, so a pair of headphones offers a Mixer it cannot use and a
/// Soundscape it cannot play.
///
/// Mapped by NAME rather than index so reordering `VIEWS` cannot silently
/// re-point the rule at the wrong page — an index table is the kind of thing
/// that stays correct until someone inserts a tab.
pub fn view_requires(view: &str) -> Option<&'static str> {
    match view {
        // The EQ mixer is the one page that IS the control.
        "Mixer" => Some("eq"),
        // Soundscape plays a generated stream at the device.
        "Soundscape" => Some("stream"),
        // Spaces is multi-room: it needs somewhere to send audio.
        "Spaces" => Some("autoroute"),
        // Devices, Library and Settings are always available. Settings in
        // particular must never be hidden — it is where a person fixes a
        // wrong profile, and a device that guessed badly would otherwise have
        // no way back.
        _ => None,
    }
}

/// The views this product should show, in `VIEWS` order.
///
/// An unknown product shows everything, because `has_control` falls back to
/// true on an empty list — the QML's own rule, and the right one: hiding
/// pages on an unrecognised device makes a new speaker look broken, while
/// showing a page that turns out to do nothing is the lesser failure.
pub fn views_for(profile: &Profile, all: &[&'static str]) -> Vec<&'static str> {
    // THE BUILT-IN FALLBACK IS A GUESS, NOT KNOWLEDGE: it is what a product
    // nothing describes resolves to. Gating pages on a guess hides them from a
    // speaker merely because it is new -- which is what 919ccdf27 ("a product
    // without a control should not get its page") did by gating the Mixer on
    // `eq`: this fallback's controls, ported verbatim from the QML, have no `eq`,
    // so every unrecognised product lost its Mixer. That broke 870daf6fc's
    // "an unrecognised product ... still shows every view" (6 expected, 5 got),
    // plus three view-reconcile tests -- found by fleet_build.sh.
    //
    // Only the BUILT-IN default is exempt. A map that describes "Unknown Type"
    // itself is its author's deliberate claim and still gates. And the control
    // list is untouched, so the widgets that read it keep the QML's set.
    if *profile == Profile::default() {
        return all.to_vec();
    }
    all.iter()
        .copied()
        .filter(|v| view_requires(v).is_none_or(|c| profile.has_control(c)))
        .collect()
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

    /// THE QML RULE THIS FRONTEND LOST: a product without a control does not
    /// get the page. Headphones offering a Mixer they cannot use is the
    /// symptom.
    #[test]
    fn a_product_without_a_control_does_not_get_its_page() {
        let all = crate::VIEWS;
        let phones = Profile {
            controls: ["volume", "mute"].iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let v = views_for(&phones, &all);
        assert!(!v.contains(&"Mixer"), "no eq, no Mixer: {v:?}");
        assert!(!v.contains(&"Soundscape"), "no stream, no Soundscape: {v:?}");
        assert!(!v.contains(&"Spaces"), "no autoroute, no Spaces: {v:?}");
    }

    /// AN UNKNOWN PRODUCT SHOWS EVERYTHING -- `has_control`'s fallback, and
    /// the right one: hiding pages on an unrecognised device makes a new
    /// speaker look broken, where a page that does nothing is the lesser
    /// failure.
    #[test]
    fn an_unknown_product_keeps_every_view() {
        let unknown = Profile { controls: vec![], ..Default::default() };
        assert_eq!(views_for(&unknown, &crate::VIEWS).len(), crate::VIEWS.len());
    }

    /// Settings must NEVER be hidden: it is where a person fixes a wrong
    /// profile, and a device that guessed badly would have no way back.
    #[test]
    fn settings_survives_the_emptiest_profile() {
        let bare = Profile {
            controls: vec!["nothing-real".to_string()],
            ..Default::default()
        };
        let v = views_for(&bare, &crate::VIEWS);
        assert!(v.contains(&"Settings"), "{v:?}");
        assert!(v.contains(&"Devices"), "and the device list itself: {v:?}");
    }

    /// Order is preserved, so filtering cannot reshuffle the tab strip.
    #[test]
    fn the_surviving_views_keep_their_order() {
        let full = Profile {
            controls: ["eq", "stream", "autoroute"].iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let v = views_for(&full, &crate::VIEWS);
        assert_eq!(v, crate::VIEWS.to_vec());
    }

    /// Mapped by NAME, not index: an index table stays correct until someone
    /// inserts a tab, and then points the rule at the wrong page.
    #[test]
    fn the_rule_is_keyed_by_name_not_position() {
        assert_eq!(view_requires("Mixer"), Some("eq"));
        assert_eq!(view_requires("Devices"), None);
        assert_eq!(view_requires("a view that does not exist"), None,
                   "an unknown view is always shown rather than always hidden");
    }

    fn map(entries: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Profile> {
        entries.iter().map(|(k, ctrls)| {
            (k.to_string(), Profile {
                name: k.to_string(),
                controls: ctrls.iter().map(|c| c.to_string()).collect(),
                ..Default::default()
            })
        }).collect()
    }

    /// Step 1: the product's own entry wins.
    #[test]
    fn a_known_product_gets_its_own_profile() {
        let m = map(&[("Beosound A5", &["volume", "mute", "eq"])]);
        let p = resolve_profile(&m, Some("Beosound A5"));
        assert_eq!(p.name, "Beosound A5");
        assert!(p.has_control("eq"));
    }

    /// Step 2 is NOT skippable. A deployment can describe what an
    /// unrecognised device looks like without a rebuild, and falling straight
    /// to the hardcoded default would ignore that.
    #[test]
    fn an_unknown_product_prefers_the_maps_own_unknown_entry() {
        let m = map(&[("Unknown Type", &["volume"])]);
        let p = resolve_profile(&m, Some("Something Nobody Shipped"));
        assert_eq!(p.name, "Unknown Type", "the map's entry, not the hardcoded one");
        assert!(!p.has_control("eq"));
    }

    /// Step 3: an empty or unparseable map is NORMAL, not an error -- the
    /// QML's JSON.parse is wrapped in a try returning `{}`.
    #[test]
    fn an_empty_map_falls_back_to_the_hardcoded_profile() {
        let empty = std::collections::BTreeMap::new();
        let p = resolve_profile(&empty, Some("Beosound A5"));
        assert_eq!(p.name, Profile::default().name);
        assert!(p.has_control("stream"), "the hardcoded fallback is permissive");
    }

    /// No product type reported yet -- the state at startup -- still resolves
    /// rather than erroring, and still prefers the map's Unknown entry.
    #[test]
    fn a_missing_product_type_resolves_like_an_unknown_one() {
        let m = map(&[("Unknown Type", &["volume"])]);
        assert_eq!(resolve_profile(&m, None).name, "Unknown Type");
        let empty = std::collections::BTreeMap::new();
        assert_eq!(resolve_profile(&empty, None).name, Profile::default().name);
    }

    /// The chain end to end: an unrecognised product on a map that does not
    /// describe one still shows every view, which is the failure mode that
    /// matters -- a new speaker must not look broken.
    #[test]
    fn an_unrecognised_product_on_a_bare_map_still_shows_everything() {
        let m = map(&[("Beosound A5", &["volume", "eq"])]);
        let p = resolve_profile(&m, Some("Beolab 90"));
        assert_eq!(views_for(&p, &crate::VIEWS).len(), crate::VIEWS.len());
    }
}
