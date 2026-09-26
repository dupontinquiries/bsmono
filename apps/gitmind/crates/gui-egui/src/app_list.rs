//! The app catalog as a list you pick from — Fleet's and Push's.
//!
//! It was a 280 px scroll box inside the page's own scroll (Fleet) and a combo
//! box popup (Push): about seven 22 px rows visible at a time, a second
//! scrollbar that only appears on hover, suites folded shut, and on a phone a
//! drag that scrolled the page or the box depending on where it started. Now:
//! a search field, 40 px rows the whole width of the page, part of the page's
//! one scroll, and the list folds to the chosen app once one is picked (see
//! the callers), so the controls below it are not a long scroll away.
//!
//! [`filter`] is the headless half.

use cockpit::suite::{Suite, SuiteApp};

/// Row height: a finger's worth, and the tile readable.
pub const ROW_H: f32 = 40.0;

/// The suites and apps matching `query` (case-insensitive, on the app's name
/// or its suite's name/description), suites with no match dropped. An empty
/// query is everything.
pub fn filter(groups: &[Suite], query: &str) -> Vec<Suite> {
    let q = query.trim().to_lowercase();
    groups.iter().filter_map(|s| {
        let suite_hit = !q.is_empty() && (s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q));
        let apps: Vec<SuiteApp> = s.apps.iter()
            .filter(|a| q.is_empty() || suite_hit || a.name.to_lowercase().contains(&q))
            .cloned().collect();
        (!apps.is_empty()).then(|| Suite { name: s.name.clone(), description: s.description.clone(), apps })
    }).collect()
}

/// One app row: tile, name, the whole width clickable. Returns the response.
pub fn row(ui: &mut egui::Ui, a: &SuiteApp, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
    let v = ui.visuals().clone();
    if selected {
        ui.painter().rect_filled(rect, 6.0, v.selection.bg_fill);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
    }
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 6.0)))
        .layout(egui::Layout::left_to_right(egui::Align::Center)));
    // deterministic identicon tile (ported from the Qt MonoTile), not an emoji
    // The tile and the name are widgets of their own that sense clicks (the
    // tile) or text selection (a label), and so took the press from the row:
    // the union is what was clicked. Caught by clicking a row that did nothing.
    let tile = uikit_egui::mono_tile::mono_tile(&mut inner, &a.name, 28.0);
    inner.add_space(10.0);
    let name = inner.add(egui::Label::new(egui::RichText::new(&a.name).size(15.0)
        .color(if selected { v.strong_text_color() } else { v.text_color() })).selectable(false).sense(egui::Sense::click()));
    resp.union(tile).union(name)
}

/// The search field and the grouped list. Returns the app pressed this frame.
/// `query` is the caller's (so it survives the list folding away).
pub fn list_ui(ui: &mut egui::Ui, id: &str, query: &mut String, selected: Option<&str>) -> Option<SuiteApp> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(query).id_salt(("app-search", id)).hint_text("find an app")
            .desired_width(ui.available_width().min(360.0)));
        if !query.is_empty() && ui.small_button("✕").on_hover_text("clear").clicked() { query.clear(); }
    });
    let groups = filter(&cockpit::flow::store_groups(), query);
    if groups.is_empty() {
        ui.label(egui::RichText::new(format!("no app matches \"{}\"", query.trim())).weak());
    }
    let searching = !query.trim().is_empty();
    for suite in groups {
        let header = egui::CollapsingHeader::new(egui::RichText::new(format!("{}  ·  {}  ({})", suite.name, suite.description, suite.apps.len())).size(14.0))
            .id_salt((id, "suite", &suite.name))
            // Everything open: the page scrolls, so a folded suite only hid
            // apps. Searching forces open so a match is never folded away.
            .default_open(true);
        let header = if searching { header.open(Some(true)) } else { header };
        header.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0; // the rows carry their own padding
            for a in &suite.apps {
                if row(ui, a, selected == Some(a.name.as_str())).clicked() {
                    picked = Some(a.clone());
                }
            }
        });
    }
    picked
}

/// The chosen app, folded: its tile and name, and a button to choose again.
/// Returns true when "change" is pressed.
pub fn chosen_ui(ui: &mut egui::Ui, a: &SuiteApp, detail: &str) -> bool {
    let mut change = false;
    ui.horizontal(|ui| {
        uikit_egui::mono_tile::mono_tile(ui, &a.name, 28.0);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(&a.name).size(16.0).strong());
            if !detail.is_empty() { ui.label(egui::RichText::new(detail).weak()); }
        });
        ui.add_space(10.0);
        change = ui.button("change app").clicked();
    });
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups() -> Vec<Suite> {
        let s = |n: &str, d: &str, apps: &[&str]| Suite { name: n.into(), description: d.into(),
            apps: apps.iter().map(|a| SuiteApp::simple(a, a)).collect() };
        vec![s("coding", "Development & analysis tools", &["gitmind", "logscope"]),
             s("media", "Video, camera, imaging & games", &["snapclone", "photosync", "av1-tuner"])]
    }

    #[test]
    fn an_empty_query_is_everything() {
        assert_eq!(filter(&groups(), "  ").iter().map(|s| s.apps.len()).sum::<usize>(), 5);
    }

    #[test]
    fn a_query_matches_names_case_insensitively_and_drops_empty_suites() {
        let f = filter(&groups(), "SNAP");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "media");
        assert_eq!(f[0].apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["snapclone"]);
        assert!(filter(&groups(), "zzz").is_empty());
    }

    #[test]
    fn a_suite_name_or_description_match_shows_the_whole_suite() {
        assert_eq!(filter(&groups(), "camera")[0].apps.len(), 3, "the description matched");
        assert_eq!(filter(&groups(), "coding")[0].apps.len(), 2);
    }

    /// A press anywhere on a row picks it: on the tile, on the name, and on
    /// the empty width to the right. The tile and the label are widgets that
    /// take presses themselves, and the row first shipped answering only to
    /// the empty part — clicking the name did nothing.
    #[test]
    fn a_press_anywhere_on_the_row_picks_it() {
        let ctx = egui::Context::default();
        let a = SuiteApp::simple("snapclone", "snapclone");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 200.0));
        let mut rect = egui::Rect::NOTHING;
        let mut clicked = false;
        let mut frame = |events: Vec<egui::Event>, rect: &mut egui::Rect, clicked: &mut bool| {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = row(ui, &a, false);
                    *rect = r.rect;
                    *clicked |= r.clicked();
                });
            });
        };
        frame(vec![], &mut rect, &mut clicked);
        for (what, at) in [("tile", rect.left_center() + egui::vec2(22.0, 0.0)),
                           ("name", rect.left_center() + egui::vec2(80.0, 0.0)),
                           ("empty width", rect.right_center() - egui::vec2(30.0, 0.0))] {
            clicked = false;
            let b = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            frame(vec![egui::Event::PointerMoved(at)], &mut rect, &mut clicked);
            frame(vec![b(true)], &mut rect, &mut clicked);
            frame(vec![b(false)], &mut rect, &mut clicked);
            assert!(clicked, "a press on the {what} did not pick the row");
        }
        assert!(rect.height() >= ROW_H && rect.width() > 500.0, "a finger-sized, full-width row: {rect:?}");
    }

    #[test]
    fn the_real_catalog_is_all_reachable_through_the_list() {
        // Every app in the store appears exactly once with no query: nothing
        // is lost to the filter's suite handling.
        let all = cockpit::flow::store_groups();
        let n: usize = all.iter().map(|s| s.apps.len()).sum();
        assert!(n > 10, "the catalog is suspiciously small: {n}");
        assert_eq!(filter(&all, "").iter().map(|s| s.apps.len()).sum::<usize>(), n);
    }
}
