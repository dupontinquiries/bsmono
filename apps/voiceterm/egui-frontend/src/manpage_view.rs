//! manpage_view — the reader for voiceterm's bundled documents.
//!
//! ## Why this one is NOT an include
//!
//! Every other engine this port has reached for came across with `#[path]` —
//! the table engine, the row cache, the extractors, the session walk. This one
//! cannot, and the reason is worth writing down rather than discovering again.
//!
//! `manpages.rs` is 4,104 lines and splits cleanly in principle: `get`,
//! `list_names` and `to_html` are Qt-free, while `to_qml` and friends pull in
//! `qml_gen` (5,880 lines) and `scraper`. A module cannot be half-included, so
//! sharing it needs the Qt-free half lifted into its own file upstream.
//!
//! I did not do that, because **the app crate cannot be built in this
//! container**: its build script shells out to `qmake`, and there is no Qt here.
//! A refactor of a 4,000-line module that I cannot compile is not a refactor,
//! it is a guess. Recorded in PORT_STATUS as the next real step for whoever has
//! a Qt host.
//!
//! ## So the view takes documents rather than fetching them
//!
//! The host owns the registry and the markdown→HTML conversion it already has;
//! this renders through `uikit_web::parse_html`, the shared kit's document
//! model. Nothing is duplicated — not the registry list, not the converter, not
//! the renderer. The same "declare what you NEED, host supplies" shape as
//! `catalog_source`.
//!
//! ## A reader needs a way back
//!
//! Documents link to each other, and a reader that can only go forward turns
//! every wrong turn into a trip through the index. The back stack is the whole
//! difference between a document viewer and a document dumper.

/// One document the host has made available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub name: String,
    /// The title shown in the index and the header, if the host has one.
    pub title: String,
    /// Already-converted HTML. The host owns the markdown and the conversion.
    pub html: String,
}

impl Doc {
    /// Never blank: an untitled document shows its name, which is what the user
    /// typed or clicked to get here.
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() { self.name.clone() } else { self.title.clone() }
    }
}

/// The reader.
#[derive(Debug, Clone, Default)]
pub struct ManpageView {
    docs: Vec<Doc>,
    /// Names visited, oldest first. The last is the current page.
    stack: Vec<String>,
    /// The index filter.
    pub filter: String,
}

/// What a name that is not in the registry produces.
pub const NOT_FOUND_PREFIX: &str = "No manual page named ";

impl ManpageView {
    pub fn set_docs(&mut self, docs: Vec<Doc>) {
        // A stack entry for a document that has gone would render a not-found
        // page the user never asked for, so prune rather than keep.
        self.stack.retain(|n| docs.iter().any(|d| &d.name == n));
        self.docs = docs;
    }

    pub fn docs(&self) -> &[Doc] { &self.docs }
    pub fn is_empty(&self) -> bool { self.docs.is_empty() }

    /// The document being read, if any.
    pub fn current(&self) -> Option<&Doc> {
        let name = self.stack.last()?;
        self.docs.iter().find(|d| &d.name == name)
    }

    pub fn current_name(&self) -> Option<&str> { self.stack.last().map(|s| s.as_str()) }
    pub fn depth(&self) -> usize { self.stack.len() }
    pub fn can_go_back(&self) -> bool { self.stack.len() > 1 }

    /// Is there a page by this name?
    ///
    /// Split out from `open` so a caller can decide BEFORE navigating —
    /// a cross-reference to a page this machine does not have should leave
    /// you where you are rather than being attempted and silently refused,
    /// which reads as the link being broken twice.
    pub fn has(&self, name: &str) -> bool {
        self.docs.iter().any(|d| d.name == name)
    }

    /// Open a document. Returns false when there is no such page.
    ///
    /// Opening the page you are already on is not a navigation: it would push a
    /// duplicate and make Back appear to do nothing.
    pub fn open(&mut self, name: &str) -> bool {
        if !self.docs.iter().any(|d| d.name == name) { return false; }
        if self.current_name() == Some(name) { return true; }
        self.stack.push(name.to_string());
        true
    }

    /// Go back one page.
    pub fn back(&mut self) -> bool {
        if !self.can_go_back() { return false; }
        self.stack.pop();
        true
    }

    /// Return to the index, clearing the trail.
    pub fn close(&mut self) { self.stack.clear(); }

    /// What to show when a link points at a page that does not exist.
    ///
    /// Names the page rather than saying "not found": a link in a document can
    /// go stale, and the name is the only thing that tells the reader which
    /// link was wrong.
    pub fn not_found(name: &str) -> String { format!("{NOT_FOUND_PREFIX}\"{name}\"") }

    /// The index, filtered.
    ///
    /// Matches the name AND the title, because a user looking for "getting
    /// started" should find `welcome` — the file name is an implementation
    /// detail they never chose.
    pub fn index(&self) -> Vec<&Doc> {
        let f = self.filter.trim().to_lowercase();
        self.docs.iter()
            .filter(|d| f.is_empty()
                || d.name.to_lowercase().contains(&f)
                || d.title.to_lowercase().contains(&f))
            .collect()
    }

    /// The trail, for a breadcrumb — where the reader has been to get here.
    pub fn trail(&self) -> Vec<String> {
        self.stack.iter()
            .map(|n| self.docs.iter().find(|d| &d.name == n)
                 .map(|d| d.label()).unwrap_or_else(|| n.clone()))
            .collect()
    }
}

/// Draw the reader. Returns a name when the user navigates.
pub fn manpage_ui(ui: &mut egui::Ui, v: &mut ManpageView,
                  c: &uikit_egui::PaletteColors) -> Option<String> {
    let mut nav = None;
    match v.current() {
        Some(doc) => {
            let (label, html) = (doc.label(), doc.html.clone());
            ui.horizontal(|ui| {
                // Back is disabled at the first page rather than hidden: a
                // control that vanishes reads as a missing feature.
                if ui.add_enabled(v.can_go_back(), egui::Button::new("‹")).clicked() {
                    v.back();
                }
                if ui.button("index").clicked() { v.close(); }
                ui.strong(label);
            });
            ui.separator();
            // Rendered through the shared document model — the same one the
            // browser view uses, so a manual page and a web page look alike.
            let doc = uikit_web::parse_html(&html);
            egui::ScrollArea::vertical().id_salt("manpage").show(ui, |ui| {
                // A LINK IN A MANUAL PAGE IS A CROSS-REFERENCE. `render`
                // returns the href now, and here that is the name of
                // another page — following it is the whole reason a manual
                // is hyperlinked, and it did nothing before because the
                // renderer had nowhere to report a click.
                //
                // The leading '#' or '/' is stripped: manual cross-refs are
                // written both ways, and `open` wants a bare name.
                if let Some(href) = uikit_web::render(ui, &doc, c) {
                    let name = href.trim_start_matches(['#', '/']).trim();
                    if !name.is_empty() && v.has(name) {
                        nav = Some(name.to_string());
                    }
                }
            });
        }
        None => {
            ui.add(egui::TextEdit::singleline(&mut v.filter)
                .hint_text("Search the manual").desired_width(240.0));
            if v.is_empty() {
                ui.colored_label(c.text_weak, "No documents available.");
                return None;
            }
            let hits = v.index().into_iter().map(|d| (d.name.clone(), d.label()))
                .collect::<Vec<_>>();
            if hits.is_empty() {
                // Names the filter, so the user can see what they searched for.
                ui.colored_label(c.text_weak, format!("Nothing matching \"{}\"", v.filter.trim()));
                return None;
            }
            egui::ScrollArea::vertical().id_salt("manpage_index").show(ui, |ui| {
                for (name, label) in hits {
                    if ui.add(egui::Button::new(&label).frame(false)).clicked() {
                        nav = Some(name.clone());
                    }
                }
            });
        }
    }
    if let Some(n) = &nav { v.open(n); }
    nav
}

/// Driven through real frames, because the thing being added is a click.
#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::frame_probe::Probe;

    fn view() -> ManpageView {
        let mut v = ManpageView::default();
        v.set_docs(vec![
            // One link per paragraph, deliberately: `click_text` sweeps the
            // ROW it finds the text on (see `frame_probe`'s note on egui's
            // unreliable text x), so two links side by side would both be
            // reachable from either name and the test would be measuring
            // whichever came first.
            Doc { name: "grep".into(), title: "grep".into(),
                  html: "<p>see <a href=\"#sed\">sed</a></p>\
                         <p>also <a href=\"#nosuch\">nosuch</a></p>".into() },
            Doc { name: "sed".into(), title: "sed".into(),
                  html: "<p>stream editor</p>".into() },
        ]);
        v.open("grep");
        v
    }

    #[test]
    fn following_a_cross_reference_navigates_to_that_page() {
        // A hyperlinked manual whose links do nothing is a manual with
        // blue words in it. The renderer had nowhere to report a click
        // until now, so this could not have worked.
        let p = Probe::default();
        let c = p.colors();
        let mut v = view();
        let nav = p.click_text("sed", |ui| manpage_ui(ui, &mut v, &c));
        assert_eq!(nav.as_deref(), Some("sed"));
    }

    #[test]
    fn a_reference_to_a_page_this_machine_lacks_leaves_you_where_you_are() {
        // Attempting it and being silently refused reads as the link being
        // broken twice.
        let p = Probe::default();
        let c = p.colors();
        let mut v = view();
        let nav = p.click_text("nosuch", |ui| manpage_ui(ui, &mut v, &c));
        assert_eq!(nav, None);
        assert_eq!(v.current_name(), Some("grep"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, title: &str) -> Doc {
        Doc { name: name.into(), title: title.into(),
              html: format!("<h1>{title}</h1><p>body of {name}</p>") }
    }

    fn view() -> ManpageView {
        let mut v = ManpageView::default();
        v.set_docs(vec![doc("welcome", "Getting started"),
                        doc("voice", "Voice commands"),
                        doc("keys", "Keyboard reference")]);
        v
    }

    #[test]
    fn a_reader_starts_on_the_index() {
        let v = view();
        assert!(v.current().is_none());
        assert_eq!(v.depth(), 0);
    }

    #[test]
    fn opening_a_document_shows_it() {
        let mut v = view();
        assert!(v.open("voice"));
        assert_eq!(v.current().map(|d| d.name.as_str()), Some("voice"));
    }

    #[test]
    fn opening_a_page_that_does_not_exist_is_refused() {
        let mut v = view();
        assert!(!v.open("nonsense"));
        assert!(v.current().is_none());
    }

    #[test]
    fn a_reader_can_go_back_which_is_the_point_of_a_reader() {
        // A viewer that can only go forward turns every wrong turn into a trip
        // through the index.
        let mut v = view();
        v.open("welcome");
        v.open("voice");
        assert!(v.can_go_back());
        assert!(v.back());
        assert_eq!(v.current_name(), Some("welcome"));
    }

    #[test]
    fn back_at_the_first_page_is_refused_rather_than_emptying_the_reader() {
        let mut v = view();
        v.open("welcome");
        assert!(!v.can_go_back());
        assert!(!v.back());
        assert_eq!(v.current_name(), Some("welcome"), "still reading");
    }

    #[test]
    fn opening_the_page_you_are_already_on_is_not_a_navigation() {
        // It would push a duplicate and make Back appear to do nothing.
        let mut v = view();
        v.open("welcome");
        assert!(v.open("welcome"));
        assert_eq!(v.depth(), 1);
        assert!(!v.can_go_back());
    }

    #[test]
    fn revisiting_an_earlier_page_still_stacks() {
        // welcome → voice → welcome is three steps of reading, and Back should
        // retrace them.
        let mut v = view();
        v.open("welcome");
        v.open("voice");
        v.open("welcome");
        assert_eq!(v.depth(), 3);
        v.back();
        assert_eq!(v.current_name(), Some("voice"));
    }

    #[test]
    fn closing_returns_to_the_index_and_clears_the_trail() {
        let mut v = view();
        v.open("welcome");
        v.open("voice");
        v.close();
        assert!(v.current().is_none());
        assert_eq!(v.depth(), 0);
    }

    #[test]
    fn a_document_that_disappears_is_pruned_from_the_trail() {
        // Otherwise the reader renders a not-found page the user never asked
        // for.
        let mut v = view();
        v.open("welcome");
        v.open("voice");
        v.set_docs(vec![doc("welcome", "Getting started")]);
        assert_eq!(v.current_name(), Some("welcome"));
        assert_eq!(v.depth(), 1);
    }

    #[test]
    fn the_not_found_message_names_the_page() {
        // A link in a document can go stale, and the name is the only thing
        // that tells the reader which link was wrong.
        let msg = ManpageView::not_found("voice-old");
        assert!(msg.contains("voice-old"));
    }

    #[test]
    fn the_index_matches_titles_as_well_as_names() {
        // Someone looking for "getting started" should find `welcome` — the
        // file name is an implementation detail they never chose.
        let mut v = view();
        v.filter = "getting".into();
        let hits = v.index();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "welcome");
    }

    #[test]
    fn the_index_filter_is_case_insensitive_and_trimmed() {
        let mut v = view();
        v.filter = "  VOICE  ".into();
        assert_eq!(v.index().len(), 1);
    }

    #[test]
    fn an_empty_filter_shows_everything() {
        let mut v = view();
        v.filter = "   ".into();
        assert_eq!(v.index().len(), 3);
    }

    #[test]
    fn a_filter_matching_nothing_returns_nothing_rather_than_everything() {
        // Falling back to the full list on no match is how a search box stops
        // being trusted.
        let mut v = view();
        v.filter = "zzzz".into();
        assert!(v.index().is_empty());
    }

    #[test]
    fn an_untitled_document_shows_its_name() {
        let d = Doc { name: "voice".into(), title: "  ".into(), html: String::new() };
        assert_eq!(d.label(), "voice");
    }

    #[test]
    fn the_trail_reads_in_titles_rather_than_file_names() {
        let mut v = view();
        v.open("welcome");
        v.open("keys");
        assert_eq!(v.trail(), vec!["Getting started".to_string(),
                                   "Keyboard reference".to_string()]);
    }

    #[test]
    fn an_empty_reader_is_harmless() {
        let mut v = ManpageView::default();
        assert!(v.is_empty());
        assert!(!v.open("welcome"));
        assert!(!v.back());
        assert!(v.index().is_empty());
        assert!(v.trail().is_empty());
    }
}
