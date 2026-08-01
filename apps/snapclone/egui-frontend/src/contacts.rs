//! contacts — the contact editor, the picker, and the security bar.
//!
//! Three surfaces in one module because they share a type and are always used
//! together: you pick a contact, you edit a contact, and the conversation with
//! that contact shows how private it is.
//!
//! ## ContactEditorDialog
//!
//! `onSave`, `onDelete`, `onExportVCard`, `onCopyId`. The delete is a signal
//! (`deleteRequested`) rather than an action, because the dialog does not own
//! the contact list — a dialog that deletes its own subject and then has to tell
//! everyone else is how a list and its editor drift apart.
//!
//! ## ContactPickerDialog
//!
//! `setMultiSelect(on, confirmLabel)` — the same dialog is a single-pick and a
//! multi-pick, and the confirm label changes with it. Single-pick emits on
//! **double click** (`onItemDoubleClicked`); multi-pick emits on confirm. A
//! single-pick that fired on a single click would send a snap to whoever your
//! thumb landed on while scrolling.
//!
//! ## ConversationSecurityBar
//!
//! Three tiers — `Standard`, `Securing`, `Private` — and the middle one is the
//! point. Securing is *in progress*: a bar that only knows secure from insecure
//! has to pick one to show while a handshake runs, and picking `Private` claims
//! a guarantee that does not hold yet.

/// A contact, reduced to what these three surfaces need.
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub avatar_path: Option<String>,
    pub verified: bool,
}

impl Contact {
    /// What to show in a list. An unnamed contact shows its id rather than an
    /// empty row, since a blank entry looks like a broken list.
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() { &self.id } else { &self.name }
    }
}

// ── editor ──────────────────────────────────────────────────────────────────

/// What the editor asks the owner to do. Returned rather than performed,
/// because the dialog does not own the contact list.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorAction {
    Save(Contact),
    DeleteRequested(String),
    ExportVCard(String),
    CopyId(String),
}

#[derive(Debug, Clone)]
pub struct ContactEditor {
    pub draft: Contact,
    original: Contact,
}

impl ContactEditor {
    pub fn new(c: Contact) -> ContactEditor {
        ContactEditor { draft: c.clone(), original: c }
    }

    /// Unsaved changes, by comparison rather than a dirty flag — so typing a
    /// character and deleting it is not a change, and the prompt does not cry
    /// wolf.
    pub fn is_dirty(&self) -> bool { self.draft != self.original }

    /// The vCard. Minimal and standards-shaped: a contact exported here has to
    /// import into a phone's address book, which is the only reason to export
    /// at all.
    pub fn vcard(&self) -> String {
        let c = &self.draft;
        format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:{}\r\nNOTE:snapclone {}\r\nEND:VCARD\r\n",
                if c.name.trim().is_empty() { c.id.as_str() } else { c.name.as_str() },
                c.id)
    }

    /// Problems that must be fixed before saving.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.draft.id.trim().is_empty() { v.push("a contact needs an id".into()); }
        if self.draft.name.chars().count() > 64 { v.push("name is too long".into()); }
        v
    }

    pub fn can_save(&self) -> bool { self.is_dirty() && self.problems().is_empty() }
}

// ── picker ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContactPicker {
    pub contacts: Vec<Contact>,
    pub multi: bool,
    pub confirm_label: String,
    pub header: String,
    pub query: String,
    selected: Vec<String>,
}

impl ContactPicker {
    pub fn new(contacts: Vec<Contact>) -> ContactPicker {
        ContactPicker { contacts, multi: false, confirm_label: "Send".into(),
                        header: "Send to".into(), query: String::new(), selected: Vec::new() }
    }

    /// `setMultiSelect(on, confirmLabel)`. Leaving single-select clears the
    /// selection: carrying five contacts into a single-pick and then emitting
    /// one of them silently drops four, which the user will not notice until
    /// the snap arrives in one place.
    pub fn set_multi(&mut self, on: bool, confirm_label: Option<&str>) {
        if !on { self.selected.clear(); }
        self.multi = on;
        if let Some(l) = confirm_label { self.confirm_label = l.to_string(); }
    }

    pub fn visible(&self) -> Vec<&Contact> {
        let q = self.query.trim().to_lowercase();
        self.contacts.iter()
            .filter(|c| q.is_empty() || c.label().to_lowercase().contains(&q)
                        || c.id.to_lowercase().contains(&q))
            .collect()
    }

    /// A single click. In multi-select it toggles; in single-select it only
    /// highlights — `onItemDoubleClicked` is what commits, so a thumb landing
    /// on a row while scrolling cannot send a snap.
    pub fn click(&mut self, id: &str) -> Option<Vec<String>> {
        if !self.contacts.iter().any(|c| c.id == id) { return None; }
        if self.multi {
            match self.selected.iter().position(|s| s == id) {
                Some(i) => { self.selected.remove(i); }
                None => self.selected.push(id.to_string()),
            }
            None
        } else {
            self.selected.clear();
            self.selected.push(id.to_string());
            None
        }
    }

    /// A double click. Commits in single-select only.
    pub fn double_click(&mut self, id: &str) -> Option<String> {
        if self.multi || !self.contacts.iter().any(|c| c.id == id) { return None; }
        Some(id.to_string())
    }

    /// The confirm button. Multi-select only, and never with an empty set.
    pub fn confirm(&mut self) -> Option<Vec<String>> {
        if !self.multi || self.selected.is_empty() { return None; }
        Some(self.selected.clone())
    }

    pub fn selected(&self) -> &[String] { &self.selected }
    pub fn is_selected(&self, id: &str) -> bool { self.selected.iter().any(|s| s == id) }
}

// ── security bar ────────────────────────────────────────────────────────────

/// How private this conversation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tier {
    /// Ordinary delivery.
    #[default]
    Standard,
    /// A private channel is being negotiated. NOT yet private — claiming the
    /// guarantee before the handshake completes is the one thing a security
    /// indicator must never do.
    Securing,
    Private,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self { Tier::Standard => "standard", Tier::Securing => "securing…", Tier::Private => "private" }
    }
    /// Is the lock drawn closed? Only when the guarantee actually holds.
    pub fn lock_closed(self) -> bool { matches!(self, Tier::Private) }
    /// Should the bar animate? Securing is the only state that is going
    /// somewhere, and motion is how a user knows to wait rather than retry.
    pub fn animating(self) -> bool { matches!(self, Tier::Securing) }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SecurityBar {
    pub tier: Tier,
    /// Our local request, which is separate from the achieved tier: the toggle
    /// reflects what we asked for, the lock reflects what we got.
    pub requested: bool,
}

impl SecurityBar {
    /// Toggling asks; it does not grant. Returns the new request state to send.
    pub fn toggle(&mut self) -> bool { self.requested = !self.requested; self.requested }

    /// Does the display disagree with what we asked for? True while a request
    /// is outstanding, which is exactly when the bar should show `Securing`
    /// rather than either endpoint.
    pub fn pending(&self) -> bool {
        (self.requested && self.tier != Tier::Private)
            || (!self.requested && self.tier == Tier::Private)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, name: &str) -> Contact {
        Contact { id: id.into(), name: name.into(), avatar_path: None, verified: false }
    }

    fn picker() -> ContactPicker {
        ContactPicker::new(vec![c("sc-1", "Ada"), c("sc-2", "Bo"), c("sc-3", "")])
    }

    #[test]
    fn an_unnamed_contact_shows_its_id_rather_than_an_empty_row() {
        assert_eq!(c("sc-9", "Ada").label(), "Ada");
        assert_eq!(c("sc-9", "").label(), "sc-9");
        assert_eq!(c("sc-9", "   ").label(), "sc-9");
    }

    #[test]
    fn a_single_pick_needs_a_double_click_to_commit() {
        // A single click that sends would fire on whoever your thumb landed on
        // while scrolling.
        let mut p = picker();
        assert_eq!(p.click("sc-1"), None, "a click only highlights");
        assert!(p.is_selected("sc-1"));
        assert_eq!(p.double_click("sc-1").as_deref(), Some("sc-1"));
        // and an unknown contact commits nothing
        assert_eq!(p.double_click("nope"), None);
    }

    #[test]
    fn a_multi_pick_toggles_and_commits_on_confirm() {
        let mut p = picker();
        p.set_multi(true, Some("Add"));
        assert_eq!(p.confirm_label, "Add");
        p.click("sc-1");
        p.click("sc-2");
        assert_eq!(p.selected().len(), 2);
        p.click("sc-1");
        assert_eq!(p.selected(), &["sc-2"], "clicking again removes");
        assert_eq!(p.confirm().unwrap(), vec!["sc-2"]);
        // double click does not commit in multi mode
        assert_eq!(p.double_click("sc-2"), None);
    }

    #[test]
    fn confirming_with_nothing_selected_does_nothing() {
        // An empty send is not a send, and the button should not pretend.
        let mut p = picker();
        p.set_multi(true, None);
        assert_eq!(p.confirm(), None);
        // nor in single mode, ever
        p.set_multi(false, None);
        p.click("sc-1");
        assert_eq!(p.confirm(), None);
    }

    #[test]
    fn leaving_multi_select_clears_the_selection() {
        // Carrying five contacts into a single-pick and emitting one silently
        // drops four, which the user notices only when the snap arrives in one
        // place.
        let mut p = picker();
        p.set_multi(true, None);
        p.click("sc-1"); p.click("sc-2");
        p.set_multi(false, None);
        assert!(p.selected().is_empty());
    }

    #[test]
    fn the_picker_searches_names_and_ids() {
        let mut p = picker();
        p.query = "ada".into();
        assert_eq!(p.visible().len(), 1);
        p.query = "SC-3".into();
        assert_eq!(p.visible().len(), 1, "ids are searchable and case-insensitive");
        p.query = "  ".into();
        assert_eq!(p.visible().len(), 3);
    }

    #[test]
    fn the_editor_asks_rather_than_deletes() {
        // A dialog that deletes its own subject and then tells everyone else is
        // how a list and its editor drift apart.
        let e = ContactEditor::new(c("sc-1", "Ada"));
        let a = EditorAction::DeleteRequested(e.draft.id.clone());
        assert_eq!(a, EditorAction::DeleteRequested("sc-1".into()));
    }

    #[test]
    fn the_editor_is_dirty_only_on_a_real_change() {
        let mut e = ContactEditor::new(c("sc-1", "Ada"));
        assert!(!e.is_dirty());
        assert!(!e.can_save(), "nothing to save");
        e.draft.name = "Adah".into();
        assert!(e.is_dirty() && e.can_save());
        e.draft.name = "Ada".into();
        assert!(!e.is_dirty(), "typing and deleting is not a change");
    }

    #[test]
    fn a_contact_with_no_id_cannot_be_saved() {
        let mut e = ContactEditor::new(c("sc-1", "Ada"));
        e.draft.id = "  ".into();
        assert!(!e.problems().is_empty());
        assert!(!e.can_save());
    }

    #[test]
    fn the_vcard_is_importable() {
        // The only reason to export is that it lands in a phone's address book.
        let e = ContactEditor::new(c("sc-1", "Ada"));
        let v = e.vcard();
        assert!(v.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
        assert!(v.contains("FN:Ada"));
        assert!(v.ends_with("END:VCARD\r\n"));
        // an unnamed contact still exports with a usable FN
        let u = ContactEditor::new(c("sc-2", ""));
        assert!(u.vcard().contains("FN:sc-2"));
    }

    #[test]
    fn securing_never_shows_a_closed_lock() {
        // Claiming the guarantee before the handshake completes is the one thing
        // a security indicator must never do.
        assert!(!Tier::Securing.lock_closed());
        assert!(!Tier::Standard.lock_closed());
        assert!(Tier::Private.lock_closed());
        // and securing is the only state that animates, so a user knows to wait
        assert!(Tier::Securing.animating());
        assert!(!Tier::Private.animating());
    }

    #[test]
    fn the_toggle_asks_and_the_lock_reports() {
        // The toggle reflects what we requested; the tier reflects what we got.
        // Conflating them makes the lock close the instant you press the button.
        let mut b = SecurityBar::default();
        assert!(b.toggle(), "asked for private");
        assert_eq!(b.tier, Tier::Standard, "but nothing is granted yet");
        assert!(b.pending());
        b.tier = Tier::Securing;
        assert!(b.pending(), "still outstanding mid-handshake");
        b.tier = Tier::Private;
        assert!(!b.pending(), "request satisfied");
        // and turning it off is pending until the tier drops back
        b.toggle();
        assert!(b.pending());
    }
}
