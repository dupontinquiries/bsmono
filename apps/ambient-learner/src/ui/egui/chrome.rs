//! chrome — `SectionHeader`, and the card headings that grew up beside it.
//!
//! Two roles that look similar and are not:
//!
//! * **Section header** — the floating page title: "LIVE", "MODEL". Uppercase,
//!   letter-spaced, centred, tinted per section. One per page.
//! * **Card heading** — "📊 Model statistics", "⚙️ Device state". Left-aligned,
//!   sentence case, several per page.
//!
//! They are separated here because they were not separated before. The port
//! carried the pages and left `SectionHeader` behind, so every page grew its own
//! `RichText::new(...).strong()` — six of them across five files by the time
//! this was written. That is the same duplication this programme keeps finding
//! in the Qt code, committed again, in a module small enough that nobody would
//! notice until the day someone wanted to restyle a heading and had to find all
//! six.
//!
//! ## The width rule is the whole reason SectionHeader exists
//!
//! Its QML comment records a real bug: a zero-width rectangle plus
//! `anchors.centerIn` centres the label on x=0, and the leading characters clip
//! off the left edge of the screen. The fix was to always fill the container's
//! width. In egui the mechanism is different but the failure is not — a centred
//! label inside a zero-width allocation clips exactly the same way — so the
//! header allocates the full available width before centring.
//!
//! ## Long titles elide rather than clipping
//!
//! Also from the QML, and worth keeping: a title too long for a narrow phone is
//! cut off mid-character otherwise, which reads as a rendering fault. Eliding
//! shows it was truncated on purpose.

/// Letter spacing for a section title, in pixels.
///
/// The QML's `font.letterSpacing: 4`. It is what makes a section title read as
/// a label for the page rather than as a heading inside it — the same words at
/// normal spacing would compete with the card headings below.
pub const SECTION_TRACKING: f32 = 4.0;

/// Section title height, matching the QML's 56.
pub const SECTION_HEIGHT: f32 = 56.0;

/// Horizontal room reserved so eliding has somewhere to apply.
pub const SECTION_MARGIN: f32 = 32.0;

/// Which section a page belongs to, for the accent tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section { Live, History, Places, Map, Model, Sync, Debug }

impl Section {
    /// The title as it is shown: uppercase, always.
    ///
    /// Applied here rather than trusting callers, so one page cannot end up
    /// title-cased while the others shout.
    pub fn title(self) -> &'static str {
        match self {
            Section::Live => "LIVE",
            Section::History => "HISTORY",
            Section::Places => "PLACES",
            Section::Map => "MAP",
            Section::Model => "MODEL",
            Section::Sync => "SYNC",
            Section::Debug => "DEBUG",
        }
    }

    /// Every section, in navigation order.
    ///
    /// Live first because it is what the app is for; Debug last because it is
    /// for us. The order is data rather than an ad-hoc list at each call site,
    /// so the tab strip, the keyboard shortcuts and any future command palette
    /// cannot disagree about what comes after what.
    pub const ALL: [Section; 7] = [
        Section::Live, Section::History, Section::Places, Section::Map,
        Section::Model, Section::Sync, Section::Debug,
    ];

    /// A glyph for the tab strip.
    ///
    /// Emoji rather than an icon font, which is the iced shell's reasoning and
    /// worth keeping verbatim: they "work on most Linux text rasterizers and
    /// let us avoid bundling an icon font", and they "fall back gracefully on
    /// older systems (the label is always visible)".
    ///
    /// That last clause is the load-bearing half. On mobile Linux a missing
    /// glyph renders as a tofu box, so the icon is never the only label — a tab
    /// strip that degraded to seven identical boxes would be unusable, and this
    /// is exactly the stack where that happens.
    pub fn icon(self) -> &'static str {
        match self {
            Section::Live => "🌡",
            Section::History => "🕘",
            Section::Places => "📍",
            Section::Map => "🗺",
            Section::Model => "🧠",
            Section::Sync => "🔄",
            Section::Debug => "🔧",
        }
    }

    /// Digit shortcut, matching the iced shell's 1-based numbering.
    ///
    /// Returns `None` past the ninth section rather than wrapping to '0' or
    /// running into punctuation: a shortcut nobody can guess is worse than no
    /// shortcut, because it still consumes the keystroke.
    pub fn shortcut(self) -> Option<char> {
        let i = Section::ALL.iter().position(|s| *s == self)?;
        if i >= 9 { return None; }
        char::from_digit(i as u32 + 1, 10)
    }

    /// The section a digit selects.
    pub fn from_shortcut(c: char) -> Option<Section> {
        let d = c.to_digit(10)?;
        if d == 0 { return None; }
        Section::ALL.get((d - 1) as usize).copied()
    }

    /// Label for a tab: icon then title, or the title alone when glyphs are
    /// unavailable.
    pub fn tab_label(self, glyphs: bool) -> String {
        if glyphs { format!("{} {}", self.icon(), self.title()) } else { self.title().into() }
    }
}

/// Add letter spacing to a title by interleaving thin spaces.
///
/// egui has no tracking control, so the spacing is in the string. Thin spaces
/// rather than ordinary ones because a normal space at this size reads as four
/// separate words rather than one spaced word.
pub fn tracked(title: &str) -> String {
    title.chars().map(|c| c.to_string()).collect::<Vec<_>>().join("\u{2009}")
}

/// Elide a title that will not fit, in characters.
///
/// Character-based rather than pixel-based on purpose: this is a fallback for
/// a title too long for a narrow phone, and a rough limit that always applies
/// beats an exact one that needs a font context the caller may not have.
pub fn elided(title: &str, max_chars: usize) -> String {
    if max_chars == 0 { return String::new(); }
    if title.chars().count() <= max_chars { return title.to_string(); }
    // The ellipsis takes one of the budget, so the result is never longer than
    // asked for — an "elided" string that overflows is worse than none.
    let keep = max_chars.saturating_sub(1);
    let mut out: String = title.chars().take(keep).collect();
    out.push('…');
    out
}

/// Draw the floating page title.
pub fn section_header(ui: &mut egui::Ui, section: Section, accent: egui::Color32) {
    // Allocate the FULL width before centring. The QML comment records what
    // happens otherwise: the label centres on x=0 and the leading characters
    // clip off the left edge.
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, SECTION_HEIGHT),
                                           egui::Sense::hover());
    let budget = ((w - SECTION_MARGIN) / 14.0).max(1.0) as usize;
    let text = tracked(&elided(section.title(), budget));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text,
                      egui::FontId::proportional(18.0), accent);
}

/// Draw a heading inside a page.
///
/// One function so the six that existed cannot drift, and so a restyle is one
/// edit rather than a search.
pub fn card_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_titles_are_always_uppercase() {
        // Applied here rather than trusted to callers, so one page cannot end
        // up title-cased while the others shout.
        for s in [Section::Live, Section::History, Section::Places, Section::Map,
                  Section::Model, Section::Sync, Section::Debug] {
            assert_eq!(s.title(), s.title().to_uppercase());
        }
    }

    #[test]
    fn every_section_has_its_own_title() {
        let titles: Vec<&str> = [Section::Live, Section::History, Section::Places,
                                 Section::Map, Section::Model, Section::Sync,
                                 Section::Debug].iter().map(|s| s.title()).collect();
        let mut sorted = titles.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), titles.len(), "two sections share a title");
    }

    #[test]
    fn tracking_spaces_the_letters_without_splitting_the_word() {
        // Thin spaces rather than ordinary ones: a normal space at this size
        // reads as four separate words rather than one spaced word.
        let t = tracked("LIVE");
        assert!(t.contains('\u{2009}'));
        assert!(!t.contains(' '), "an ordinary space would break the word up");
        assert_eq!(t.chars().filter(|c| c.is_alphabetic()).count(), 4);
    }

    #[test]
    fn tracking_a_single_character_adds_nothing() {
        assert_eq!(tracked("X"), "X");
        assert_eq!(tracked(""), "");
    }

    #[test]
    fn a_long_title_elides_rather_than_clipping() {
        // Cut off mid-character reads as a rendering fault; an ellipsis reads
        // as a decision.
        let e = elided("CALIBRATION HISTORY", 8);
        assert!(e.ends_with('…'));
        assert_eq!(e.chars().count(), 8);
    }

    #[test]
    fn an_elided_title_is_never_longer_than_the_budget() {
        // The ellipsis takes one of the budget. An "elided" string that
        // overflows is worse than none.
        for n in 1..12 {
            assert!(elided("CALIBRATION HISTORY", n).chars().count() <= n, "at {n}");
        }
    }

    #[test]
    fn a_title_that_fits_is_left_alone() {
        assert_eq!(elided("LIVE", 8), "LIVE");
        assert_eq!(elided("LIVE", 4), "LIVE", "exactly fitting is fitting");
    }

    #[test]
    fn a_zero_budget_produces_nothing_rather_than_a_bare_ellipsis() {
        // An ellipsis alone tells the reader there was a title and not what it
        // was, which is worse than an empty header they can see is empty.
        assert_eq!(elided("LIVE", 0), "");
    }

    #[test]
    fn eliding_counts_characters_not_bytes() {
        // A title with an emoji or an accent would otherwise be cut mid-glyph.
        let e = elided("MODÈLE ÉTENDU", 6);
        assert_eq!(e.chars().count(), 6);
        assert!(e.starts_with("MODÈL"));
    }

    #[test]
    fn every_section_is_reachable_by_a_digit() {
        // A tab strip with a shortcut on some tabs and not others teaches
        // nobody the pattern.
        for s in Section::ALL {
            let c = s.shortcut().unwrap_or_else(|| panic!("{s:?} has no shortcut"));
            assert_eq!(Section::from_shortcut(c), Some(s), "{s:?} round trip");
        }
    }

    #[test]
    fn shortcuts_are_one_based_and_match_navigation_order() {
        // The iced shell numbers from 1, and two front-ends of one app
        // disagreeing about which key opens which page is worse than either
        // choice.
        assert_eq!(Section::Live.shortcut(), Some('1'));
        assert_eq!(Section::History.shortcut(), Some('2'));
        assert_eq!(Section::Debug.shortcut(), Some('7'));
    }

    #[test]
    fn zero_selects_nothing_rather_than_the_first_section() {
        // '0' sits beside '1' on every keyboard, and a mis-hit that navigates
        // is worse than one that does nothing.
        assert_eq!(Section::from_shortcut('0'), None);
        assert_eq!(Section::from_shortcut('9'), None, "past the end");
        assert_eq!(Section::from_shortcut('x'), None);
    }

    #[test]
    fn the_label_survives_a_platform_with_no_emoji_glyphs() {
        // On mobile Linux a missing glyph renders as a tofu box. A tab strip
        // degrading to seven identical boxes would be unusable, which is why
        // the icon is never the only label.
        for s in Section::ALL {
            assert!(s.tab_label(false).contains(s.title()));
            assert!(s.tab_label(true).contains(s.title()), "title always present");
        }
    }

    #[test]
    fn no_two_sections_share_an_icon() {
        // Two tabs with one glyph is the same failure as no glyphs, arrived at
        // by a different route.
        let mut icons: Vec<&str> = Section::ALL.iter().map(|s| s.icon()).collect();
        icons.sort_unstable();
        let before = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), before, "duplicate icon among {:?}", Section::ALL);
    }

    #[test]
    fn navigation_order_covers_every_variant_exactly_once() {
        // ALL is the single source the tab strip, the shortcuts and any future
        // command palette read from; a section missing from it would be
        // unreachable rather than merely misplaced.
        assert_eq!(Section::ALL.len(), 7);
        let mut seen = Section::ALL.to_vec();
        seen.dedup();
        assert_eq!(seen.len(), 7, "a section appears twice");
        assert_eq!(Section::ALL[0], Section::Live, "Live is what the app is for");
        assert_eq!(Section::ALL[6], Section::Debug, "Debug is for us");
    }
}
