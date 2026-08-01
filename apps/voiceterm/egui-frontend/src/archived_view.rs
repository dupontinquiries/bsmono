//! archived_view + bottom_bar_view — two surfaces on the engine's own models.
//!
//! `src/archive.rs` and `src/bottom_bar.rs` are pure Rust with no Qt in them, so
//! both are mounted by `#[path]` and this module is only the drawing and the
//! view state — the same arrangement config, niftyplot and the voice stack use.
//!
//! ## Archived pages
//!
//! voiceterm archives what you read, extracts key points (prices, dates,
//! commands, versions) and lets you search it back. The view's job is to make
//! that findable, which is a different problem from storing it:
//!
//! * **Search covers the key points, not just the title.** Someone looking for a
//!   version number remembers the number, not the page it was on.
//! * **Grouped by domain**, because "that thing on the Rust docs" is how people
//!   actually recall a page.
//! * **The snippet is truncated on a character boundary.** `safe_trunc` in the
//!   engine already does this; the view must not re-truncate naively and split a
//!   multi-byte character, which panics on a slice.
//!
//! ## The bottom bar
//!
//! A row of buttons that **fan out** into option lists. The rule that matters:
//! a button with no options must not fan out into an empty popup — an empty
//! popup reads as a broken button, where a disabled one reads as "nothing here".

#[path = "../../src/bottom_bar.rs"]
#[allow(dead_code, unused_imports)]
pub mod bottom_bar;

/// The archived-page fields a VIEW needs.
///
/// Not a `#[path]` mount, unlike `bottom_bar` below: `archive.rs` pulls in the
/// `scraper` HTML crate to extract key points, which drags a parser into a
/// widget crate for six fields it never uses. Same call as `mail_view` — the
/// field names match the engine's exactly, so the mapping is checkable by eye
/// and the conversion is written once at the boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchivedPage {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub archived_at: String,
    pub key_points: Vec<KeyPoint>,
    pub text_snippet: String,
    pub html_size: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyPoint {
    pub kind: &'static str,
    pub text: String,
    pub context: String,
}

/// Truncate on a CHARACTER boundary.
///
/// Mirrors the engine's `safe_trunc`. Slicing a String by byte index panics
/// mid-character, and archived text is full of non-ASCII — a curly quote in a
/// headline is enough.
pub fn safe_trunc(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}
pub use bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};

/// Archived-page browser state.
#[derive(Debug, Clone, Default)]
pub struct ArchiveView {
    pub query: String,
    /// Domain filter; empty shows everything.
    pub domain: String,
    pub selected: Option<String>,
    /// Group by domain rather than listing flat.
    pub grouped: bool,
}

impl ArchiveView {
    /// Pages matching the query.
    ///
    /// Searches title, url AND key points: someone looking for a version number
    /// remembers the number, not which page carried it. Case-insensitive,
    /// because a search that needs exact case is one nobody uses twice.
    pub fn matches<'a>(&self, pages: &'a [ArchivedPage]) -> Vec<&'a ArchivedPage> {
        let q = self.query.trim().to_lowercase();
        pages.iter().filter(|p| {
            if !self.domain.is_empty() && p.domain != self.domain { return false; }
            q.is_empty()
                || p.title.to_lowercase().contains(&q)
                || p.url.to_lowercase().contains(&q)
                || p.key_points.iter().any(|k| k.text.to_lowercase().contains(&q))
        }).collect()
    }

    /// Domains present, each with a count, most-archived first.
    ///
    /// Ties break alphabetically so the list does not reshuffle between frames.
    pub fn domains(&self, pages: &[ArchivedPage]) -> Vec<(String, usize)> {
        use std::collections::BTreeMap;
        let mut m: BTreeMap<&str, usize> = BTreeMap::new();
        for p in pages { *m.entry(p.domain.as_str()).or_insert(0) += 1; }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(d, n)| (d.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

/// A short preview of a page, cut on a character boundary.
pub fn preview(p: &ArchivedPage, max: usize) -> &str {
    safe_trunc(&p.text_snippet, max)
}

/// Should this bar button fan out?
///
/// A button with no options must not open an empty popup: an empty popup reads
/// as a broken button, while a disabled one reads as "nothing here yet".
pub fn fans_out(b: &BarButton) -> bool { !b.options.is_empty() }

/// Draw the archive list. Returns a url if one was opened.
pub fn archive_ui(ui: &mut egui::Ui, v: &mut ArchiveView, pages: &[ArchivedPage],
                  colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut opened = None;
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut v.query);
        ui.checkbox(&mut v.grouped, "by domain");
        if !v.domain.is_empty() && ui.small_button(format!("{} ✕", v.domain)).clicked() {
            v.domain.clear();
        }
    });
    ui.separator();
    let hits = v.matches(pages).into_iter().cloned().collect::<Vec<_>>();
    if hits.is_empty() {
        ui.label(egui::RichText::new(if v.query.is_empty() { "nothing archived yet" }
                                     else { "nothing matches" }).weak());
        return None;
    }
    if v.grouped {
        for (d, n) in v.domains(&hits) {
            ui.horizontal(|ui| {
                if ui.small_button(format!("{d} ({n})")).clicked() { v.domain = d.clone(); }
            });
        }
        ui.separator();
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for p in &hits {
            let resp = ui.vertical(|ui| {
                ui.label(egui::RichText::new(&p.title).strong());
                ui.label(egui::RichText::new(&p.domain).weak().small());
                ui.label(egui::RichText::new(preview(p, 140)).small());
                if !p.key_points.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for k in p.key_points.iter().take(6) {
                            ui.label(egui::RichText::new(format!("{}: {}", k.kind, k.text))
                                .color(colors.accent).small());
                        }
                    });
                }
            }).response;
            if resp.interact(egui::Sense::click()).clicked() { opened = Some(p.url.clone()); }
            ui.separator();
        }
    });
    opened
}

/// Draw the bar. Returns the option id chosen, if any.
pub fn bar_ui(ui: &mut egui::Ui, bar: &BottomBar, open: &mut Option<String>,
              colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        for b in bar.buttons() {
            let can = fans_out(b);
            // badge and primary are plain Strings in the engine, not Options —
            // empty means absent, which is the convention the whole module uses.
            let label = if b.badge.is_empty() { b.label.clone() }
                        else { format!("{} {}", b.label, b.badge) };
            let r = ui.add_enabled(can, egui::Button::new(label));
            if r.clicked() {
                *open = if open.as_deref() == Some(b.id.as_str()) { None } else { Some(b.id.clone()) };
            }
        }
    });
    if let Some(id) = open.clone() {
        if let Some(b) = bar.find(&id) {
            ui.separator();
            for o in &b.options {
                if ui.selectable_label(b.primary == o.id, &o.label).clicked() {
                    chosen = Some(o.id.clone());
                    *open = None;
                }
            }
            if b.options.is_empty() {
                ui.label(egui::RichText::new("nothing here").color(colors.text_weak).small());
            }
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str, title: &str, domain: &str, kp: &[(&str, &str)]) -> ArchivedPage {
        ArchivedPage {
            url: url.into(), title: title.into(), domain: domain.into(),
            archived_at: "2026-01-01T00:00:00Z".into(),
            key_points: kp.iter().map(|(k, t)| KeyPoint {
                kind: match *k { "price" => "price", "version" => "version", _ => "date" },
                text: (*t).into(), context: String::new() }).collect(),
            text_snippet: "some archived text".into(), html_size: 1024,
        }
    }

    fn corpus() -> Vec<ArchivedPage> {
        vec![
            page("https://docs.rs/serde", "Serde docs", "docs.rs", &[("version", "1.0.203")]),
            page("https://docs.rs/tokio", "Tokio docs", "docs.rs", &[("version", "1.38")]),
            page("https://shop.example/x", "Widget", "shop.example", &[("price", "£42.00")]),
        ]
    }

    #[test]
    fn search_covers_key_points_not_just_titles() {
        // Someone looking for a version number remembers the NUMBER, not which
        // page carried it.
        let mut v = ArchiveView::default();
        v.query = "1.0.203".into();
        let pages = corpus();
        let hits = v.matches(&pages);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Serde docs");
        // and the price, likewise
        v.query = "£42".into();
        assert_eq!(v.matches(&pages).len(), 1);
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut v = ArchiveView::default();
        v.query = "SERDE".into();
        assert_eq!(v.matches(&corpus()).len(), 1);
        v.query = "  ".into();
        assert_eq!(v.matches(&corpus()).len(), 3, "a blank query shows everything");
    }

    #[test]
    fn the_domain_filter_and_the_query_compose() {
        let mut v = ArchiveView::default();
        v.domain = "docs.rs".into();
        assert_eq!(v.matches(&corpus()).len(), 2);
        v.query = "tokio".into();
        assert_eq!(v.matches(&corpus()).len(), 1);
        v.query = "widget".into();
        assert!(v.matches(&corpus()).is_empty(), "the filter still applies");
    }

    #[test]
    fn domains_are_counted_and_ordered_stably() {
        // A list that reshuffles between frames is one you cannot click.
        let v = ArchiveView::default();
        let a = v.domains(&corpus());
        let b = v.domains(&corpus());
        assert_eq!(a, b);
        assert_eq!(a[0], ("docs.rs".to_string(), 2), "most archived first");
    }

    #[test]
    fn the_preview_never_splits_a_character() {
        // Archived text is full of non-ASCII - a curly quote in a headline is
        // enough - and slicing a String by byte index panics mid-character.
        let mut p = page("u", "t", "d", &[]);
        p.text_snippet = "café — naïve “quoted” ①②③".into();
        for n in 0..p.text_snippet.len() + 4 {
            let s = preview(&p, n);
            assert!(p.text_snippet.starts_with(s), "n={n}");
        }
    }

    #[test]
    fn a_button_with_no_options_does_not_fan_out() {
        // An empty popup reads as a broken button; a disabled one reads as
        // "nothing here yet".
        let empty = BarButton::new("tabs", "Tabs", BarButtonKind::Tabs);
        assert!(!fans_out(&empty));
        let full = BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
            .with_options(vec![BarOption::new("a", "One")]);
        assert!(fans_out(&full));
    }
}
