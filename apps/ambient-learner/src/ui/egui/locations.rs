//! locations — `LocationsPage`, ported.
//!
//! The saved places a reading can be attributed to. One is current; the rest
//! are waiting. You can add a place by name and remove one you no longer use.
//!
//! ## A location without coordinates is normal, not broken
//!
//! The QML checks `latitude !== null` before formatting, and the reason matters:
//! a place is created from a NAME and only gains coordinates when the device is
//! next there with a fix. So "Office" with no numbers is a perfectly good
//! location that simply has not been visited yet — showing an error, or
//! refusing to save it, would break the ordinary way places are created.
//!
//! ## Exactly one current, and removing it is not the same as clearing it
//!
//! Deleting the place you are standing in leaves the app with readings and
//! nowhere to attribute them. The Qt page allows it, so this does too, but the
//! result is an explicit "no current location" rather than a silently promoted
//! neighbour — promoting one would file today's readings under a place the user
//! never chose, and they would find out weeks later looking at history.

/// One saved place.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub id: i64,
    pub name: String,
    /// Absent until the device has been there with a fix. NOT an error state.
    pub coords: Option<(f64, f64)>,
    pub current: bool,
}

impl Location {
    /// `📍` for the current place, `📌` for a saved one.
    ///
    /// Two pins rather than a pin and a blank: the row still needs its leading
    /// glyph for alignment, and an empty slot reads as a rendering fault.
    pub fn glyph(&self) -> &'static str { if self.current { "📍" } else { "📌" } }

    /// Coordinates for display, or a dash.
    ///
    /// Four decimals — about eleven metres. More would imply the fix is better
    /// than a phone's, and fewer would not distinguish two ends of a street.
    pub fn coord_text(&self) -> String {
        match self.coords {
            Some((lat, lon)) => format!("{lat:.4}, {lon:.4}"),
            None => "—".into(),
        }
    }

    /// Has this place ever been visited with a fix?
    pub fn located(&self) -> bool { self.coords.is_some() }
}

/// What the page asks the backend to do. The view performs nothing itself.
#[derive(Debug, Clone, PartialEq)]
pub enum LocationAction {
    SetCurrent(i64),
    Remove(i64),
    Add(String),
}

/// The page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocationsPage {
    locations: Vec<Location>,
    /// The add form's text, empty when closed.
    pub new_name: String,
    pub adding: bool,
    /// Set by `open_add`, consumed by the view: the entry takes keyboard
    /// focus on the frame it appears. Without it the form opened with
    /// nothing focused, and on a phone that is a form with no keyboard —
    /// the first screenshot that typed into it typed into nothing.
    pub focus_pending: bool,
}

pub const EMPTY_TEXT: &str = "No saved locations.\nTap + to add one.";

impl LocationsPage {
    pub fn set(&mut self, locations: Vec<Location>) { self.locations = locations; }
    pub fn all(&self) -> &[Location] { &self.locations }
    pub fn is_empty(&self) -> bool { self.locations.is_empty() }

    /// The current place, if there is one.
    pub fn current(&self) -> Option<&Location> {
        self.locations.iter().find(|l| l.current)
    }

    /// What the header shows.
    ///
    /// Names the place, or says there is none. A blank header would be
    /// indistinguishable from a loading state.
    pub fn header(&self) -> String {
        self.current().map(|l| l.name.clone()).unwrap_or_else(|| "—".into())
    }

    pub fn open_add(&mut self) {
        self.adding = true;
        self.focus_pending = true;
        // Cleared on OPEN rather than on close, so a cancelled entry is not
        // silently resurrected the next time the form appears.
        self.new_name.clear();
    }

    pub fn cancel_add(&mut self) { self.adding = false; }

    /// Can the entry be accepted?
    ///
    /// A blank or whitespace-only name is refused: an unnamed place cannot be
    /// told apart from another unnamed place in the list, which is the one
    /// thing the list exists to do.
    pub fn can_add(&self) -> bool { !self.new_name.trim().is_empty() }

    /// Accept the add form.
    pub fn accept_add(&mut self) -> Option<LocationAction> {
        if !self.can_add() { return None; }
        let name = self.new_name.trim().to_string();
        self.adding = false;
        self.new_name.clear();
        Some(LocationAction::Add(name))
    }

    /// Tap a row.
    ///
    /// Choosing the place you are already in is a no-op rather than a
    /// round-trip: the backend would set what is already set, and the row would
    /// flash for no reason.
    pub fn choose(&self, id: i64) -> Option<LocationAction> {
        let l = self.locations.iter().find(|l| l.id == id)?;
        if l.current { return None; }
        Some(LocationAction::SetCurrent(id))
    }

    /// Remove a place.
    pub fn remove(&self, id: i64) -> Option<LocationAction> {
        self.locations.iter().find(|l| l.id == id)?;
        Some(LocationAction::Remove(id))
    }

    /// True when removing this would leave nowhere to attribute readings.
    ///
    /// The caller can warn; the Qt page still allows it, and so does this.
    /// Blocking a delete because it is inconvenient would leave the user unable
    /// to clean up a mistyped place.
    pub fn removing_current(&self, id: i64) -> bool {
        self.current().map(|l| l.id == id).unwrap_or(false)
    }
}

/// Draw the page. Returns an action for the host to perform.
pub fn locations_ui(ui: &mut egui::Ui, p: &mut LocationsPage,
                    c: &uikit_egui::PaletteColors,
                    // The scene is passed IN rather than living on the
                    // page, because `A11yScene` derives only `Default`
                    // and `LocationsPage` is `Debug + Clone + PartialEq`
                    // — putting it on the page cost the page all three.
                    //
                    // It still has to outlive the frame (a scene rebuilt
                    // each frame forgets which element had focus), so the
                    // shell owns it: same lifetime, no derive conflict.
                    a11y: &mut uikit_egui::a11y::A11yScene)
                    -> Option<LocationAction> {
    let mut act = None;
    ui.label(egui::RichText::new("Current location").small().color(c.text_weak));
    ui.heading(p.header());
    ui.separator();

    if p.is_empty() {
        ui.colored_label(c.text_weak, EMPTY_TEXT);
    } else {
        let rows: Vec<Location> = p.all().to_vec();
        egui::ScrollArea::vertical().id_salt("locations").show(ui, |ui| {
            for l in &rows {
                ui.horizontal(|ui| {
                    ui.label(l.glyph());
                    ui.vertical(|ui| {
                        // Ids are derived from the LOCATION id, not the
                        // loop index: removing a row shifts every index
                        // after it, and a positional id would move focus
                        // onto a different place than the one it was on.
                        let resp = uikit_egui::a11y::note_button(
                            a11y, l.id as u64 * 2,
                            &format!("select {}", l.name),
                            ui.add(egui::Button::new(&l.name).frame(false)));
                        if resp.clicked() {
                            act = p.choose(l.id);
                        }
                        // The coordinate line is dimmer: it is context, and a
                        // place with none must not look like a broken row.
                        ui.colored_label(c.text_weak,
                            egui::RichText::new(l.coord_text()).small());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // "✕" tells a screen reader nothing, and it is
                        // the DESTRUCTIVE control on the row — the one
                        // where guessing wrong costs a saved place.
                        let resp = uikit_egui::a11y::note_button(
                            a11y, l.id as u64 * 2 + 1,
                            &format!("remove {}", l.name),
                            ui.button("✕"));
                        if resp.clicked() { act = p.remove(l.id); }
                    });
                });
            }
        });
    }

    ui.add_space(8.0);
    if p.adding {
        ui.label("What do you call this place?");
        let resp = ui.add(egui::TextEdit::singleline(&mut p.new_name).id_salt("locations-p_new_name")
            .hint_text("e.g. Office"));
        if std::mem::take(&mut p.focus_pending) { resp.request_focus(); }
        // Enter accepts, as the QML's onAccepted does — a form you can only
        // submit by aiming at a button is a form you fight on a phone.
        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() { p.cancel_add(); }
            if ui.add_enabled(p.can_add(), egui::Button::new("Add")).clicked() || entered {
                if let Some(a) = p.accept_add() { act = Some(a); }
            }
        });
    } else if ui.button("➕ Add new location").clicked() {
        p.open_add();
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(id: i64, name: &str, current: bool, coords: Option<(f64, f64)>) -> Location {
        Location { id, name: name.into(), coords, current }
    }

    fn page() -> LocationsPage {
        let mut p = LocationsPage::default();
        p.set(vec![loc(1, "Home", true, Some((51.5074, -0.1278))),
                   loc(2, "Office", false, None)]);
        p
    }

    #[test]
    fn a_location_without_coordinates_is_normal_rather_than_broken() {
        // A place is created from a NAME and gains coordinates when the device
        // is next there with a fix. Refusing to save it, or showing an error,
        // would break the ordinary way places are created.
        let p = page();
        let office = &p.all()[1];
        assert!(!office.located());
        assert_eq!(office.coord_text(), "—");
    }

    #[test]
    fn coordinates_are_shown_to_about_eleven_metres() {
        // More decimals would imply a better fix than a phone has; fewer would
        // not tell two ends of a street apart.
        let l = loc(1, "Home", true, Some((51.50743, -0.12780)));
        assert_eq!(l.coord_text(), "51.5074, -0.1278");
    }

    #[test]
    fn the_current_place_gets_a_different_pin_not_a_blank() {
        // The row still needs its leading glyph for alignment; an empty slot
        // reads as a rendering fault.
        let p = page();
        assert_eq!(p.all()[0].glyph(), "📍");
        assert_eq!(p.all()[1].glyph(), "📌");
        assert_ne!(p.all()[0].glyph(), p.all()[1].glyph());
    }

    #[test]
    fn the_header_names_the_current_place() {
        assert_eq!(page().header(), "Home");
    }

    #[test]
    fn with_no_current_place_the_header_says_so_rather_than_going_blank() {
        // A blank header is indistinguishable from a loading state.
        let mut p = LocationsPage::default();
        p.set(vec![loc(2, "Office", false, None)]);
        assert_eq!(p.header(), "—");
        assert!(p.current().is_none());
    }

    #[test]
    fn choosing_a_different_place_asks_the_backend_to_switch() {
        assert_eq!(page().choose(2), Some(LocationAction::SetCurrent(2)));
    }

    #[test]
    fn choosing_the_place_you_are_already_in_does_nothing() {
        // The backend would set what is already set and the row would flash for
        // no reason.
        assert_eq!(page().choose(1), None);
    }

    #[test]
    fn choosing_a_place_that_is_not_there_is_refused() {
        assert_eq!(page().choose(99), None);
    }

    #[test]
    fn removing_the_current_place_is_allowed_but_flagged() {
        // Blocking it would leave the user unable to clean up a mistyped place.
        // Flagging it lets the caller warn first.
        let p = page();
        assert!(p.removing_current(1));
        assert!(!p.removing_current(2));
        assert_eq!(p.remove(1), Some(LocationAction::Remove(1)));
    }

    #[test]
    fn an_empty_list_says_what_to_do_next() {
        let p = LocationsPage::default();
        assert!(p.is_empty());
        assert!(EMPTY_TEXT.contains("Tap +"));
    }

    #[test]
    fn a_blank_name_cannot_be_added() {
        // An unnamed place cannot be told from another unnamed place, which is
        // the one thing the list exists to do.
        let mut p = page();
        p.open_add();
        assert!(!p.can_add());
        p.new_name = "   ".into();
        assert!(!p.can_add());
        assert_eq!(p.accept_add(), None);
        assert!(p.adding, "and the form stays open");
    }

    #[test]
    fn a_name_is_trimmed_before_it_is_saved() {
        // " Office " and "Office" are the same place, and only one of them
        // sorts where the user expects.
        let mut p = page();
        p.open_add();
        p.new_name = "  Office  ".into();
        assert_eq!(p.accept_add(), Some(LocationAction::Add("Office".into())));
    }

    #[test]
    fn the_form_clears_on_open_so_a_cancelled_entry_is_not_resurrected() {
        let mut p = page();
        p.open_add();
        p.new_name = "Half typed".into();
        p.cancel_add();
        p.open_add();
        assert_eq!(p.new_name, "");
        assert!(!p.can_add());
    }

    #[test]
    fn accepting_closes_the_form_and_clears_it() {
        let mut p = page();
        p.open_add();
        p.new_name = "Cabin".into();
        p.accept_add();
        assert!(!p.adding);
        assert_eq!(p.new_name, "");
    }

    #[test]
    fn the_view_performs_nothing_itself() {
        // Every interaction returns an ACTION for the host. A page that mutated
        // its own list would show a place that was never saved.
        let p = page();
        let before = p.all().to_vec();
        let _ = p.choose(2);
        let _ = p.remove(1);
        assert_eq!(p.all(), before.as_slice());
    }
}
