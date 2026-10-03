//! style -- the looks a script can be printed in. System font stacks only (the page is self-contained and
//! works offline, so nothing is fetched): each stack names its ideal face first and falls back to what every
//! system has.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLook {
    pub uppercase: bool,
    pub bold: bool,
    pub small_caps: bool,
    /// Letter spacing, em.
    pub spacing_em: f32,
    /// A thin rule under the heading.
    pub rule: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerLook {
    pub uppercase: bool,
    pub small_caps: bool,
    pub bold: bool,
    pub spacing_em: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub name: &'static str,
    pub blurb: &'static str,
    /// CSS font-family value.
    pub font: &'static str,
    pub size_pt: f32,
    /// Line height, a multiple of the size.
    pub leading: f32,
    pub scene: SceneLook,
    pub speaker: SpeakerLook,
    /// The text column's width on the page, inches.
    pub column_in: f32,
    /// Screen colours; printing is always black on white.
    pub paper: &'static str,
    pub ink: &'static str,
    pub accent: &'static str,
}

pub const STYLES: [Style; 6] = [
    Style {
        name: "classic", blurb: "Courier, 12 pt: the look of a typed screenplay",
        font: "\"Courier Prime\", \"Courier New\", Courier, monospace", size_pt: 12.0, leading: 1.25,
        scene: SceneLook { uppercase: true, bold: true, small_caps: false, spacing_em: 0.0, rule: false },
        speaker: SpeakerLook { uppercase: true, small_caps: false, bold: false, spacing_em: 0.0 },
        column_in: 5.2, paper: "#ffffff", ink: "#111111", accent: "#111111",
    },
    Style {
        name: "modern", blurb: "clean sans, airy: a contemporary treatment",
        font: "\"Inter\", \"Helvetica Neue\", Helvetica, Arial, sans-serif", size_pt: 11.0, leading: 1.55,
        scene: SceneLook { uppercase: true, bold: true, small_caps: false, spacing_em: 0.12, rule: true },
        speaker: SpeakerLook { uppercase: true, small_caps: false, bold: true, spacing_em: 0.1 },
        column_in: 5.0, paper: "#ffffff", ink: "#1a1a1a", accent: "#c0392b",
    },
    Style {
        name: "literary", blurb: "a book serif: for a script that reads like prose",
        font: "\"EB Garamond\", Garamond, \"Iowan Old Style\", \"Palatino Linotype\", Georgia, serif", size_pt: 13.0, leading: 1.5,
        scene: SceneLook { uppercase: false, bold: false, small_caps: true, spacing_em: 0.08, rule: true },
        speaker: SpeakerLook { uppercase: false, small_caps: true, bold: false, spacing_em: 0.08 },
        column_in: 4.8, paper: "#fbf7ee", ink: "#2a2118", accent: "#7a4b2a",
    },
    Style {
        name: "marquee", blurb: "display serif, wide-spaced headings: a title-card feel",
        font: "\"Playfair Display\", Didot, \"Bodoni 72\", \"Times New Roman\", serif", size_pt: 12.0, leading: 1.5,
        scene: SceneLook { uppercase: true, bold: false, small_caps: false, spacing_em: 0.3, rule: true },
        speaker: SpeakerLook { uppercase: true, small_caps: false, bold: false, spacing_em: 0.22 },
        column_in: 4.6, paper: "#ffffff", ink: "#161616", accent: "#a8842c",
    },
    Style {
        name: "noir", blurb: "typewriter on a dark page (prints black on white)",
        font: "\"Special Elite\", \"American Typewriter\", \"Courier New\", monospace", size_pt: 12.0, leading: 1.45,
        scene: SceneLook { uppercase: true, bold: true, small_caps: false, spacing_em: 0.05, rule: false },
        speaker: SpeakerLook { uppercase: true, small_caps: false, bold: false, spacing_em: 0.04 },
        column_in: 5.0, paper: "#121212", ink: "#e8e2d4", accent: "#d9a441",
    },
    Style {
        name: "typewriter", blurb: "wide-set mono: an old machine, a little uneven",
        font: "\"Courier New\", Courier, \"Liberation Mono\", monospace", size_pt: 12.0, leading: 1.7,
        scene: SceneLook { uppercase: true, bold: true, small_caps: false, spacing_em: 0.06, rule: false },
        speaker: SpeakerLook { uppercase: true, small_caps: false, bold: false, spacing_em: 0.06 },
        column_in: 5.4, paper: "#fffdf6", ink: "#222222", accent: "#222222",
    },
];

pub fn find(name: &str) -> Option<Style> {
    STYLES.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim())).copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    Letter,
    A4,
}

impl Paper {
    pub fn css(self) -> &'static str { match self { Paper::Letter => "letter", Paper::A4 => "A4" } }
    pub fn parse(s: &str) -> Option<Paper> {
        match s.trim().to_ascii_lowercase().as_str() { "letter" | "us" => Some(Paper::Letter), "a4" => Some(Paper::A4), _ => None }
    }
}

/// What to print.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub style: Style,
    /// A font stack replacing the style's.
    pub font: Option<String>,
    pub size_pt: Option<f32>,
    pub paper: Paper,
    /// Number the scenes (`1.`).
    pub scene_numbers: bool,
    pub page_numbers: bool,
    /// A title page from the script's Title/Author lines (when it has any).
    pub title_page: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { style: STYLES[0], font: None, size_pt: None, paper: Paper::Letter, scene_numbers: false, page_numbers: true, title_page: true }
    }
}

/// A font stack that is safe to put in a CSS value: names, commas, quotes, spaces, hyphens only. Anything
/// else (a `;`, `}`, `<`, a url) is refused, so an option can never inject markup or rules.
pub fn sanitize_font(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.len() > 160 { return None; }
    if t.chars().all(|c| c.is_alphanumeric() || " ,'\"-_.".contains(c)) { Some(t.to_string()) } else { None }
}

impl Options {
    pub fn font_stack(&self) -> String {
        match self.font.as_deref().and_then(sanitize_font) {
            Some(f) => {
                // the chosen face first, the style's own stack behind it as the fallback
                format!("{f}, {}", self.style.font)
            }
            None => self.style.font.to_string(),
        }
    }
    pub fn size(&self) -> f32 { self.size_pt.unwrap_or(self.style.size_pt).clamp(7.0, 28.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_styles_are_distinct_and_findable() {
        let mut names: Vec<_> = STYLES.iter().map(|s| s.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), STYLES.len(), "unique names");
        for s in STYLES {
            assert_eq!(find(s.name), Some(s));
            assert_eq!(find(&s.name.to_uppercase()), Some(s), "case does not matter");
            assert!(s.font.contains("serif") || s.font.contains("monospace") || s.font.contains("sans-serif"), "{} ends in a generic family", s.name);
            assert!((3.5..=6.0).contains(&s.column_in), "{} column {}", s.name, s.column_in);
            assert!(!s.blurb.is_empty());
        }
        assert_eq!(find("nope"), None);
    }

    #[test]
    fn a_font_override_cannot_inject_css() {
        assert_eq!(sanitize_font("Georgia, serif"), Some("Georgia, serif".into()));
        assert_eq!(sanitize_font("\"Gill Sans\", sans-serif"), Some("\"Gill Sans\", sans-serif".into()));
        for bad in ["x; } body { display:none", "a</style><script>", "url(http://evil)", "a{b}", "", "   ", &"a".repeat(300)] {
            assert_eq!(sanitize_font(bad), None, "{bad:?}");
        }
        let o = Options { font: Some("Georgia".into()), ..Options::default() };
        assert!(o.font_stack().starts_with("Georgia, "), "the chosen face leads, the style's stack follows: {}", o.font_stack());
        let o = Options { font: Some("bad;".into()), ..Options::default() };
        assert_eq!(o.font_stack(), STYLES[0].font, "a refused font changes nothing");
    }

    #[test]
    fn paper_and_size_parse_and_clamp() {
        assert_eq!(Paper::parse("A4"), Some(Paper::A4));
        assert_eq!(Paper::parse(" us "), Some(Paper::Letter));
        assert_eq!(Paper::parse("legal"), None);
        assert_eq!(Options { size_pt: Some(500.0), ..Options::default() }.size(), 28.0);
        assert_eq!(Options { size_pt: Some(1.0), ..Options::default() }.size(), 7.0);
        assert_eq!(Options::default().size(), 12.0);
    }
}
