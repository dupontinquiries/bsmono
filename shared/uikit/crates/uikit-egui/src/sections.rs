//! sections — one grouping of an app's destinations, used by both layouts.
//!
//! A flat row of destinations stops working somewhere around six: gitmind
//! has twelve, which wrapped to two ragged rows of icons with no labels, so
//! the bar's height changed with the window and none of the glyphs were
//! identifiable (three round network-ish ones, four box-ish ones).
//!
//! The fix is not smaller icons. It is admitting the destinations already
//! form groups, and saying so once:
//!
//! * **Compact** — a fixed row of SECTION tabs, labelled, that cannot wrap,
//!   plus the active section's items on a second row. One tap inside the
//!   section you are in, two to anywhere.
//! * **Wide** — the same sections become workspace columns with headers.
//!
//! Chosen over the conventional "four primary + More" because that needs
//! someone to pick the privileged four, and an app whose usage swings by
//! task (deploying vs browsing) has no stable four. Sections are stable and
//! self-describing; "More" hides half the app behind an unlabelled button.
//!
//! Items are `usize` indices into whatever list the app already has, so
//! this stays free of the app's tab enum and testable on its own.

/// A named group of destinations, by index into the app's own list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub label: &'static str,
    pub items: Vec<usize>,
}

impl Section {
    pub fn new(label: &'static str, items: &[usize]) -> Section {
        Section { label, items: items.to_vec() }
    }
}

/// How the navigation should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Few enough destinations that a single labelled row fits. Hierarchy
    /// for five items is worse than no hierarchy.
    Flat,
    /// A section row plus the active section's items.
    TwoLevel,
    /// Sections side by side, each with a header — the desktop workspace.
    Columns(usize),
}

/// Width one labelled tab needs before it starts truncating. Below this a
/// label is not a label.
pub const MIN_TAB: f32 = 96.0;
/// Below this width a workspace of columns is cramped rather than useful.
pub const MIN_COLUMN: f32 = 260.0;

/// Does a row of `n` labelled tabs fit in `avail_w` without wrapping?
pub fn row_fits(avail_w: f32, n: usize, min_tab: f32) -> bool {
    if n == 0 { return true; }
    n as f32 * min_tab.max(1.0) <= avail_w.max(0.0)
}

/// Pick the layout. `item_count` is the total number of destinations.
///
/// Order matters, and the order is: columns, then flat, then two levels.
///
/// Columns win WHENEVER the width allows, because a workspace showing two
/// sections at once beats one page plus a row of tabs — that is the whole
/// point of expanding on a desktop. My first draft preferred a flat row
/// "whenever it fits", which reads sensibly and contradicts the goal: it
/// would give a 1280pt window a single page and eleven unused tabs. The
/// test `a_small_app_is_not_given_a_hierarchy_it_does_not_need` caught it.
///
/// Below that, a flat row is preferred over two levels, because two taps to
/// reach one of four pages is a cost with no benefit.
pub fn plan(avail_w: f32, sections: &[Section], item_count: usize) -> Layout {
    let n_sec = sections.len();
    // Wide enough for the whole grouping side by side.
    if n_sec > 1 && avail_w >= MIN_COLUMN * n_sec as f32 {
        return Layout::Columns(n_sec);
    }
    if row_fits(avail_w, item_count, MIN_TAB) {
        return Layout::Flat;
    }
    if n_sec > 1 && row_fits(avail_w, n_sec, MIN_TAB) {
        return Layout::TwoLevel;
    }
    // Nothing fits with labels. Flat is still the honest answer — the caller
    // scrolls or truncates — because a section row that ALSO does not fit
    // has bought nothing and cost a level.
    Layout::Flat
}

/// Which section holds `item`. `None` when nothing does, which is a
/// grouping bug rather than a runtime state — see `covers_every_item`.
pub fn section_of(sections: &[Section], item: usize) -> Option<usize> {
    sections.iter().position(|s| s.items.contains(&item))
}

/// Every item appears exactly once across the sections. Worth asserting in
/// the app's own test: a destination in no section is unreachable in the
/// compact layout, and one in two sections highlights two tabs at once.
pub fn covers_exactly(sections: &[Section], item_count: usize) -> Result<(), String> {
    let mut seen = vec![0usize; item_count];
    for s in sections {
        for &i in &s.items {
            match seen.get_mut(i) {
                Some(c) => *c += 1,
                None => return Err(format!("section {:?} names item {i}, which does not exist", s.label)),
            }
        }
    }
    let missing: Vec<usize> = seen.iter().enumerate().filter(|(_, c)| **c == 0).map(|(i, _)| i).collect();
    if !missing.is_empty() {
        return Err(format!("items in no section (unreachable when compact): {missing:?}"));
    }
    let dupes: Vec<usize> = seen.iter().enumerate().filter(|(_, c)| **c > 1).map(|(i, _)| i).collect();
    if !dupes.is_empty() {
        return Err(format!("items in more than one section: {dupes:?}"));
    }
    Ok(())
}


/// Move `active` by `dir` sections, wrapping. Wrapping because cycling is a
/// swipe: a gesture that silently does nothing at the ends feels broken,
/// and there is no "end" to a ring of three.
pub fn cycle(active: usize, n: usize, dir: i32) -> usize {
    if n == 0 { return 0; }
    let n_i = n as i64;
    (((active as i64 + dir as i64) % n_i + n_i) % n_i) as usize
}

/// A one-row section picker: the active section's name, swipeable to cycle
/// and tappable to list them all.
///
/// One row rather than two. A row of section tabs shows everything at once
/// but costs a line of a phone screen permanently; a plain dropdown saves
/// the line but makes changing section a two-tap menu. This is both: the
/// swipe is the one-gesture change for the common case, and the tap is the
/// discoverable list for when you do not know what exists.
///
/// What the picker produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Swiped to another section — the caller decides which destination.
    Section(usize),
    /// Tapped a destination in the expanded grid: an index into the app's
    /// own list, ready to use.
    Item(usize),
}

/// Returns what the user picked, if anything.
pub fn picker_ui(ui: &mut egui::Ui, sections: &[Section], active: usize,
                 labels: &[&str], c: &crate::PaletteColors) -> Option<Pick> {
    if sections.is_empty() { return None; }
    let active = active.min(sections.len() - 1);
    let popup_id = ui.make_persistent_id("section-picker-open");
    let mut chosen = None;

    let h = ui.spacing().interact_size.y.max(28.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h), egui::Sense::click_and_drag());

    // SWIPE to cycle. The threshold is the pager's, so a swipe here and a
    // swipe between pages commit at the same distance rather than at two
    // numbers a user has to learn separately.
    if resp.drag_stopped() {
        let step = crate::pager::swipe_commits(resp.drag_delta().x, rect.width());
        if step != 0 {
            chosen = Some(Pick::Section(cycle(active, sections.len(), step)));
        }
    }
    let mut open = ui.memory(|m| m.data.get_temp::<bool>(popup_id).unwrap_or(false));
    if resp.clicked() { open = !open; }

    let p = ui.painter_at(rect);
    p.rect_filled(rect, crate::tokens::RADIUS_SM as f32, c.bg_raised);
    p.text(egui::pos2(rect.left() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER,
           format!("{}  {}", sections[active].label, if open { '\u{25B4}' } else { '\u{25BE}' }),
           egui::FontId::proportional(14.0), c.text);
    // Position dots: which of the three you are on, so the swipe has a
    // visible target and the control says how many there are without
    // being opened.
    let n = sections.len();
    for i in 0..n {
        let x = rect.right() - 12.0 - (n - 1 - i) as f32 * 12.0;
        let on = i == active;
        p.circle_filled(egui::pos2(x, rect.center().y), if on { 3.5 } else { 2.5 },
                        if on { c.accent } else { c.text_weak });
    }

    if open {
        // EXPANDED IS A GRID OF DESTINATIONS, not a second list of section
        // names. A list of sections costs a tap and then another to reach
        // anywhere; a column per section puts the WHOLE app on one panel,
        // one tap to any of it, still grouped. Drawn below in the same
        // column rather than in a floating popup, because this control sits
        // at the bottom of a phone screen where a popup covers what you were
        // reading.
        ui.add_space(4.0);
        ui.columns(sections.len(), |cols| {
            for (i, sec) in sections.iter().enumerate() {
                let ui = &mut cols[i];
                ui.label(egui::RichText::new(sec.label).small().weak());
                for &item in &sec.items {
                    if let Some(name) = labels.get(item) {
                        if ui.selectable_label(false, *name).clicked() {
                            chosen = Some(Pick::Item(item));
                        }
                    }
                }
            }
        });
        if chosen.is_some() { open = false; }
    }
    if chosen.is_some() { open = false; }
    ui.memory_mut(|m| m.data.insert_temp(popup_id, open));
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gitmind() -> Vec<Section> {
        vec![
            Section::new("Devices", &[0, 3, 4, 1]),
            Section::new("Repository", &[7, 8, 10, 6]),
            Section::new("System", &[5, 9, 2, 11]),
        ]
    }

    #[test]
    fn cycling_wraps_in_both_directions_and_survives_an_empty_list() {
        assert_eq!(cycle(0, 3, 1), 1);
        assert_eq!(cycle(2, 3, 1), 0, "forward off the end wraps");
        assert_eq!(cycle(0, 3, -1), 2, "backward off the start wraps");
        assert_eq!(cycle(1, 3, 0), 1);
        assert_eq!(cycle(0, 0, 1), 0, "no sections is not a panic");
        // A full turn is identity, whichever way.
        let mut a = 0;
        for _ in 0..3 { a = cycle(a, 3, 1); }
        assert_eq!(a, 0);
    }

    #[test]
    fn a_phone_gets_two_levels_and_a_desktop_gets_columns() {
        let s = gitmind();
        // 12 destinations never fit a 400pt row at a readable tab width, but
        // three sections do.
        assert!(!row_fits(400.0, 12, MIN_TAB));
        assert!(row_fits(400.0, 3, MIN_TAB));
        assert_eq!(plan(400.0, &s, 12), Layout::TwoLevel);
        // A desktop wide enough for three real columns gets the workspace.
        assert_eq!(plan(1280.0, &s, 12), Layout::Columns(3));
        // In between: too narrow for columns, wide enough for the section row.
        assert_eq!(plan(700.0, &s, 12), Layout::TwoLevel);
    }

    #[test]
    fn a_small_app_is_not_given_a_hierarchy_it_does_not_need() {
        // beatspace has two pages. Two levels to reach one of two is worse
        // than one, at every width.
        let two = vec![Section::new("A", &[0]), Section::new("B", &[1])];
        assert_eq!(plan(400.0, &two, 2), Layout::Flat);
        assert_eq!(plan(360.0, &two, 2), Layout::Flat);
        // ...and at a desktop width two pages side by side IS the workspace,
        // so columns are right even for a small app.
        assert_eq!(plan(1280.0, &two, 2), Layout::Columns(2));
        // Five destinations: columns once there is room for two real ones,
        // a flat row on a wide phone, two levels on a narrow one.
        let five = vec![Section::new("A", &[0, 1, 2]), Section::new("B", &[3, 4])];
        assert_eq!(plan(600.0, &five, 5), Layout::Columns(2));
        assert_eq!(plan(500.0, &five, 5), Layout::Flat, "too narrow for two columns");
        assert_eq!(plan(360.0, &five, 5), Layout::TwoLevel, "and too narrow for five tabs");
    }

    #[test]
    fn when_even_the_sections_do_not_fit_it_says_flat_rather_than_buying_a_level_for_nothing() {
        let s = gitmind();
        // 200pt holds two labelled tabs, not three.
        assert!(!row_fits(200.0, 3, MIN_TAB));
        assert_eq!(plan(200.0, &s, 12), Layout::Flat);
    }

    #[test]
    fn every_destination_lives_in_exactly_one_section() {
        assert_eq!(covers_exactly(&gitmind(), 12), Ok(()));
        assert_eq!(section_of(&gitmind(), 3), Some(0));
        assert_eq!(section_of(&gitmind(), 10), Some(1));
        assert_eq!(section_of(&gitmind(), 11), Some(2));
        assert_eq!(section_of(&gitmind(), 99), None);
    }

    #[test]
    fn a_grouping_that_loses_or_duplicates_a_destination_is_refused() {
        // The two ways this goes wrong, and both are silent at runtime: a
        // page nobody can reach, and a tab that highlights twice.
        let missing = vec![Section::new("A", &[0, 1])];
        assert!(covers_exactly(&missing, 3).unwrap_err().contains("unreachable"));
        let dup = vec![Section::new("A", &[0, 1]), Section::new("B", &[1, 2])];
        assert!(covers_exactly(&dup, 3).unwrap_err().contains("more than one"));
        let bogus = vec![Section::new("A", &[0, 7])];
        assert!(covers_exactly(&bogus, 3).unwrap_err().contains("does not exist"));
    }
}
