//! Every view function drawn headless, checked for ID CLASHES.
//!
//! Two widgets that resolve to the same egui id share one widget's state. For a `TextEdit`
//! that state includes the TEXT CURSOR, so typing into either moves the caret in the other
//! and a keystroke lands in whichever egui considers current — which reads, from the
//! outside, as "I can only get one character into this box". egui detects the clash and
//! paints a red warning at the site; nothing was reading that warning, so a clash was only
//! findable by someone typing into the field and noticing.
//!
//! `uikit_egui::layout_probe` now collects those warnings out of the frame's shapes, and
//! this walks voiceterm's views through it. The bin's own pages are not reachable from a
//! test (they live in `VoicetermApp`), so this covers the view functions the lib exposes —
//! which is where the fields are.
//!
//! Every view is also drawn TWICE in one frame, deliberately: a view whose ids are derived
//! from the position of its widgets is fine drawn once and clashes with itself the moment
//! a layout puts two of it on screen (two panes, a split, a list of cards). That is not
//! hypothetical here — the workspace tiles panes, and a pane is a view.

use uikit_egui::layout_probe;
use voiceterm_egui as vt;

fn colors() -> uikit_egui::PaletteColors {
    uikit_egui::default_palette()
}

/// Draw `f` once and then twice in the same frame; return the two reports.
fn probe_once_and_twice(name: &str, mut f: impl FnMut(&mut egui::Ui)) -> (layout_probe::Report, layout_probe::Report) {
    let one = layout_probe::probe(egui::vec2(900.0, 700.0), 2, false, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| f(ui));
    });
    let two = layout_probe::probe(egui::vec2(900.0, 700.0), 2, false, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                cols[0].push_id("pane-a", |ui| f(ui));
                cols[1].push_id("pane-b", |ui| f(ui));
            });
        });
    });
    let _ = name;
    (one, two)
}

#[test]
fn no_view_puts_two_widgets_on_one_id() {
    let c = colors();

    // ── the mail login form: four text fields, none of them given an id ──
    let mut form = vt::mail_login_view::Form::default();
    form.two_password = true; // the conditional row too
    let (one, two) = probe_once_and_twice("mail login", |ui| {
        let mut f = form.clone();
        let _ = vt::mail_login_view::login_ui(ui, &mut f, &c);
    });
    assert!(one.id_clashes.is_empty(), "mail login, drawn once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "mail login, drawn twice: {}", two.describe());

    // ── the archive search ──
    let mut v = vt::archived_view::ArchiveView::default();
    let (one, two) = probe_once_and_twice("archive", |ui| {
        let _ = vt::archived_view::archive_ui(ui, &mut v, &[], &c);
    });
    assert!(one.id_clashes.is_empty(), "archive, once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "archive, twice: {}", two.describe());

    // ── the table view's op input ──
    let mut t = vt::table_view::TableView::default();
    let headers = vec!["a".to_string(), "b".to_string()];
    let rows = vec![vec!["1".to_string(), "2".to_string()]];
    let (one, two) = probe_once_and_twice("table", |ui| {
        let _ = vt::table_view::table_view_ui(ui, &mut t, &headers, &rows, &c);
    });
    assert!(one.id_clashes.is_empty(), "table, once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "table, twice: {}", two.describe());

    // ── the card deck's filter ──
    let mut d = vt::cards_view::Deck::default();
    let (one, two) = probe_once_and_twice("cards", |ui| {
        let _ = vt::cards_view::deck_ui(ui, &mut d, &c);
    });
    assert!(one.id_clashes.is_empty(), "cards, once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "cards, twice: {}", two.describe());

    // ── the command sheet's recall query (added with the recall wiring) ──
    let mut sheet = vt::cmd_sheet::CmdSheet::default();
    sheet.open_with(Vec::new());
    let (one, two) = probe_once_and_twice("cmd sheet", |ui| {
        let _ = vt::cmd_sheet::cmd_sheet_ui(ui, &mut sheet, &c);
    });
    assert!(one.id_clashes.is_empty(), "cmd sheet, once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "cmd sheet, twice: {}", two.describe());

    // ── the manpage filter ──
    let mut m = vt::manpage_view::ManpageView::default();
    let (one, two) = probe_once_and_twice("manpage", |ui| {
        let _ = vt::manpage_view::manpage_ui(ui, &mut m, &c);
    });
    assert!(one.id_clashes.is_empty(), "manpage, once: {}", one.describe());
    assert!(two.id_clashes.is_empty(), "manpage, twice: {}", two.describe());
}
