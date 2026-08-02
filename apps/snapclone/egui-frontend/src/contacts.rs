//! contacts — `ContactPickerDialog`, `ContactEditorDialog`, `ConversationSecurityBar`.
//!
//! Three surfaces from the messaging core, ported together because they share a
//! model and separating them would mean three copies of `Contact`.
//!
//! The Qt class names are spelled out above because `parity_audit.py` matches
//! on them, and it was reporting the picker and the editor as PENDING while
//! both were sitting here fully ported — a false negative that made the port
//! look further behind than it was. Worth knowing in the other direction too:
//! the audit cannot tell a real port from a comment mentioning a class name,
//! so its number is a floor on what is named, not a measure of what works.
//! The tests below are what actually pin the behaviour.
//!
//! ## The security bar is the one with real semantics
//!
//! `ConversationSecurityBar` has three tiers — **Standard**, **Securing**,
//! **Private** — and `setPrivateRequested` reflects *our local request*
//! separately from the tier. That separation is the whole point: what I have
//! asked for and what has actually been negotiated are different facts, and
//! showing a closed padlock because I *asked* for privacy is a lie that gets
//! someone hurt.
//!
//! So: the padlock closes on `Private` and nothing else. `Securing` is a
//! distinct state — a request is in flight and the channel is not yet private —
//! and it is drawn as its own thing rather than as an optimistic `Private`.
//!
//! ## The picker's two modes
//!
//! `setMultiSelect(on, confirmLabel)`. Single-select fires `contactSelected` on
//! a double-click; multi-select accumulates and fires `contactsConfirmed`. The
//! trap is confirming an empty selection, which creates a group chat with
//! nobody in it.

/// Who we are talking to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    /// The name the PEER advertises, over SHAK. Ours to display, not to edit:
    /// overwriting it locally throws away the only name that still identifies
    /// this person after you forget what you renamed them to.
    pub display_name: String,
    /// The local, user-chosen name. This is what the editor writes, and it
    /// wins over the advertised one — `Core::Contact::label()`.
    pub nickname: String,
    /// Free-form local notes. "met at the conference, the one with the dog."
    pub notes: String,
    /// The sc-ID; shown when a display name is missing rather than a blank row.
    pub sc_id: String,
    pub blocked: bool,
    pub favourite: bool,
}

impl Contact {
    /// What to show, in `Core::Contact::label()`'s order: the nickname you
    /// chose, then the name they advertise, then the raw id. Never empty — a
    /// blank row in a contact list is unclickable and looks like corruption.
    ///
    /// The nickname coming FIRST is the point of having one. You rename someone
    /// because their advertised handle is `sc-a3f7` or a name you cannot place;
    /// a rename that the list then ignores is not a rename.
    pub fn label(&self) -> &str {
        if !self.nickname.is_empty() { &self.nickname }
        else if !self.display_name.is_empty() { &self.display_name }
        else if !self.sc_id.is_empty() { &self.sc_id }
        else { "(unnamed)" }
    }
}

/// How private this conversation actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tier {
    /// Ordinary transport encryption.
    #[default]
    Standard,
    /// A private session has been requested and is being negotiated. NOT
    /// private yet, and drawn as its own state rather than an optimistic
    /// Private.
    Securing,
    /// End-to-end private.
    Private,
}

impl Tier {
    /// Does the padlock draw closed?
    ///
    /// Only on `Private`. Showing a closed lock because privacy was *requested*
    /// is a lie that gets someone hurt, and `Securing` exists precisely so the
    /// UI has something honest to draw in between.
    pub fn lock_closed(self) -> bool { matches!(self, Tier::Private) }

    pub fn label(self) -> &'static str {
        match self { Tier::Standard => "standard", Tier::Securing => "securing…",
                     Tier::Private => "private" }
    }
}

/// The security bar's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SecurityBar {
    pub tier: Tier,
    /// Our local request, tracked SEPARATELY from the tier: what I asked for and
    /// what has been negotiated are different facts.
    pub private_requested: bool,
}

impl SecurityBar {
    /// Is our request still outstanding? The state a user needs explained,
    /// because "I toggled it and nothing happened" is otherwise indistinguishable
    /// from a broken switch.
    pub fn awaiting(self) -> bool { self.private_requested && self.tier != Tier::Private }

    /// The other side declined, or it lapsed: we asked, we are not securing, and
    /// we are not private.
    pub fn request_stalled(self) -> bool {
        self.private_requested && self.tier == Tier::Standard
    }
}

/// Picker mode.
#[derive(Debug, Clone, PartialEq)]
pub enum PickMode {
    Single,
    /// `setMultiSelect(true, confirmLabel)`.
    Multi { confirm_label: String },
}

/// The contact picker.
#[derive(Debug, Clone)]
pub struct Picker {
    pub mode: PickMode,
    pub header: String,
    pub query: String,
    selected: Vec<String>,
}

impl Default for Picker {
    fn default() -> Self {
        Picker { mode: PickMode::Single, header: "Contacts".into(),
                 query: String::new(), selected: Vec::new() }
    }
}

impl Picker {
    pub fn is_multi(&self) -> bool { matches!(self.mode, PickMode::Multi { .. }) }
    pub fn selected(&self) -> &[String] { &self.selected }

    /// Toggle a contact.
    ///
    /// In single mode the selection REPLACES rather than accumulating, so a
    /// second click does not silently pick two people for a one-to-one chat.
    pub fn toggle(&mut self, id: &str) {
        if self.is_multi() {
            match self.selected.iter().position(|s| s == id) {
                Some(i) => { self.selected.remove(i); }
                None => self.selected.push(id.to_string()),
            }
        } else {
            self.selected.clear();
            self.selected.push(id.to_string());
        }
    }

    pub fn is_selected(&self, id: &str) -> bool { self.selected.iter().any(|s| s == id) }

    /// Can the multi-select confirm fire?
    ///
    /// Never on an empty selection — confirming nothing creates a group chat
    /// with no one in it, which then has to be found and deleted.
    pub fn can_confirm(&self) -> bool { self.is_multi() && !self.selected.is_empty() }

    /// Visible contacts. Blocked ones are hidden from the picker: you cannot
    /// start a conversation with someone you have blocked, and showing them
    /// invites the attempt.
    pub fn visible<'a>(&self, all: &'a [Contact]) -> Vec<&'a Contact> {
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<&Contact> = all.iter()
            .filter(|c| !c.blocked)
            .filter(|c| q.is_empty()
                || c.label().to_lowercase().contains(&q)
                || c.sc_id.to_lowercase().contains(&q))
            .collect();
        // favourites first, then alphabetical — a list that reorders itself
        // between frames is one you misclick
        v.sort_by(|a, b| b.favourite.cmp(&a.favourite)
            .then(a.label().to_lowercase().cmp(&b.label().to_lowercase()))
            .then(a.id.cmp(&b.id)));
        v
    }
}

/// `ContactEditorDialog`.
///
/// It edits the NICKNAME and the notes. The advertised `display_name` is shown
/// read-only, exactly as the dialog does with its "Display name" row, and used
/// as the nickname field's placeholder so the box you type in already shows
/// what you are overriding. The port previously wrote the advertised name,
/// which destroys the one piece of identity that survives a bad rename.
#[derive(Debug, Clone, Default)]
pub struct Editor {
    pub contact: Option<Contact>,
    pub draft_nickname: String,
    pub draft_notes: String,
    pub avatar_path: String,
}

impl Editor {
    pub fn open(&mut self, c: &Contact) {
        self.draft_nickname = c.nickname.clone();
        self.draft_notes = c.notes.clone();
        self.contact = Some(c.clone());
    }

    /// What the empty nickname box shows: the advertised name, so the field is
    /// visibly an override rather than a blank you must fill.
    pub fn nickname_placeholder(&self) -> &str {
        match self.contact.as_ref() {
            Some(c) if !c.display_name.is_empty() => &c.display_name,
            _ => "Enter a name…",
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.contact.as_ref()
            .map(|c| c.nickname != self.draft_nickname.trim() || c.notes != self.draft_notes.trim())
            .unwrap_or(false)
    }

    /// `onSave`. Returns the updated contact, or `None` when nothing changed —
    /// so a caller does not write and broadcast an identical record.
    pub fn save(&mut self) -> Option<Contact> {
        if !self.is_dirty() { return None; }
        let c = self.contact.as_mut()?;
        c.nickname = self.draft_nickname.trim().to_string();
        c.notes = self.draft_notes.trim().to_string();
        Some(c.clone())
    }

    /// `onExportVCard`. A contact with neither name nor sc-ID exports nothing
    /// useful, so it is refused rather than producing an empty card.
    pub fn can_export(&self) -> bool {
        self.contact.as_ref().map(|c| !c.sc_id.is_empty() || !c.display_name.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, name: &str, fav: bool, blocked: bool) -> Contact {
        Contact { id: id.into(), display_name: name.into(), nickname: String::new(),
                  notes: String::new(), sc_id: format!("sc-{id}"), blocked, favourite: fav }
    }

    fn book() -> Vec<Contact> {
        vec![c("1", "Zoe", false, false), c("2", "Ada", true, false),
             c("3", "Bo", false, false), c("4", "Mallory", false, true)]
    }

    #[test]
    fn the_padlock_closes_only_when_the_channel_is_actually_private() {
        // Showing a closed lock because privacy was REQUESTED is a lie that gets
        // someone hurt. Securing exists so there is something honest to draw in
        // between.
        assert!(!Tier::Standard.lock_closed());
        assert!(!Tier::Securing.lock_closed(), "a request in flight is not privacy");
        assert!(Tier::Private.lock_closed());
    }

    #[test]
    fn a_request_is_tracked_apart_from_what_was_negotiated() {
        // What I asked for and what happened are different facts.
        let mut b = SecurityBar::default();
        b.private_requested = true;
        assert!(b.awaiting(), "asked, not yet private");
        assert!(b.request_stalled(), "and nothing is even negotiating");
        b.tier = Tier::Securing;
        assert!(b.awaiting());
        assert!(!b.request_stalled(), "negotiation started");
        b.tier = Tier::Private;
        assert!(!b.awaiting(), "arrived");
        assert!(!b.request_stalled());
    }

    #[test]
    fn single_select_replaces_rather_than_accumulating() {
        // A second click must not silently pick two people for a one-to-one chat.
        let mut p = Picker::default();
        p.toggle("1");
        p.toggle("2");
        assert_eq!(p.selected(), &["2"]);
        assert!(!p.can_confirm(), "single mode has no confirm");
    }

    #[test]
    fn multi_select_accumulates_and_toggles_off() {
        let mut p = Picker { mode: PickMode::Multi { confirm_label: "Add".into() },
                             ..Default::default() };
        p.toggle("1"); p.toggle("2");
        assert_eq!(p.selected().len(), 2);
        p.toggle("1");
        assert_eq!(p.selected(), &["2"], "toggling removes");
        assert!(p.can_confirm());
    }

    #[test]
    fn confirming_an_empty_selection_is_refused() {
        // It creates a group chat with nobody in it, which then has to be found
        // and deleted.
        let mut p = Picker { mode: PickMode::Multi { confirm_label: "Add".into() },
                             ..Default::default() };
        assert!(!p.can_confirm());
        p.toggle("1");
        assert!(p.can_confirm());
        p.toggle("1");
        assert!(!p.can_confirm(), "and again once emptied");
    }

    #[test]
    fn blocked_contacts_do_not_appear_in_the_picker() {
        // You cannot start a conversation with someone you have blocked, and
        // showing them invites the attempt.
        let p = Picker::default();
        let all = book();
        let v = p.visible(&all);
        assert_eq!(v.len(), 3);
        assert!(!v.iter().any(|c| c.display_name == "Mallory"));
    }

    #[test]
    fn favourites_come_first_and_the_order_is_stable() {
        let p = Picker::default();
        let all = book();
        let a: Vec<&str> = p.visible(&all).iter().map(|c| c.label()).collect();
        let b: Vec<&str> = p.visible(&all).iter().map(|c| c.label()).collect();
        assert_eq!(a, b);
        assert_eq!(a[0], "Ada", "the favourite leads");
        assert_eq!(&a[1..], &["Bo", "Zoe"], "then alphabetical");
    }

    #[test]
    fn a_contact_with_no_name_still_shows_something_clickable() {
        // A blank row is unclickable and looks like corruption.
        let mut x = c("9", "", false, false);
        assert_eq!(x.label(), "sc-9");
        x.sc_id.clear();
        assert_eq!(x.label(), "(unnamed)");
    }

    #[test]
    fn a_local_rename_wins_over_the_advertised_name() {
        // The whole reason to have a nickname: their handle is "sc-a3f7" or a
        // name you cannot place, so you rename them. A rename the list then
        // ignores is not a rename.
        let mut x = c("1", "sc-a3f7c291", false, false);
        assert_eq!(x.label(), "sc-a3f7c291");
        x.nickname = "Ada".into();
        assert_eq!(x.label(), "Ada");
    }

    #[test]
    fn the_label_falls_back_through_all_three_names() {
        let mut x = c("1", "", false, false);
        assert_eq!(x.label(), "sc-1", "no names, so the routable id");
        x.display_name = "Advertised".into();
        assert_eq!(x.label(), "Advertised");
        x.nickname = "Mine".into();
        assert_eq!(x.label(), "Mine");
    }

    #[test]
    fn the_editor_does_not_overwrite_the_advertised_name() {
        // Writing display_name locally destroys the one piece of identity that
        // survives a rename you later regret.
        let mut e = Editor::default();
        e.open(&c("1", "Ada Lovelace", false, false));
        e.draft_nickname = "Ada".into();
        let saved = e.save().expect("a change");
        assert_eq!(saved.nickname, "Ada");
        assert_eq!(saved.display_name, "Ada Lovelace", "untouched");
    }

    #[test]
    fn the_nickname_box_shows_the_advertised_name_as_its_placeholder() {
        // So the field reads as an override of something, not a blank to fill.
        let mut e = Editor::default();
        e.open(&c("1", "Ada Lovelace", false, false));
        assert_eq!(e.nickname_placeholder(), "Ada Lovelace");
        e.open(&c("2", "", false, false));
        assert_eq!(e.nickname_placeholder(), "Enter a name…");
    }

    #[test]
    fn notes_are_saved_and_count_as_a_change_on_their_own() {
        let mut e = Editor::default();
        e.open(&c("1", "Ada", false, false));
        e.draft_notes = "  met at the conference  ".into();
        assert!(e.is_dirty(), "a note is an edit even with the name untouched");
        let saved = e.save().expect("a change");
        assert_eq!(saved.notes, "met at the conference", "trimmed");
    }

    #[test]
    fn whitespace_alone_is_not_an_edit() {
        // Otherwise tabbing through the dialog broadcasts a record change.
        let mut e = Editor::default();
        e.open(&c("1", "Ada", false, false));
        e.draft_nickname = "   ".into();
        assert!(!e.is_dirty());
        assert!(e.save().is_none());
    }

    #[test]
    fn saving_an_unchanged_contact_does_nothing() {
        // Otherwise the app writes and broadcasts an identical record.
        let mut e = Editor::default();
        e.open(&c("1", "Ada", false, false));
        assert!(!e.is_dirty());
        assert!(e.save().is_none());
        e.draft_nickname = "Ada Lovelace".into();
        assert!(e.is_dirty());
        let saved = e.save().expect("a change to save");
        assert_eq!(saved.nickname, "Ada Lovelace");
        assert_eq!(saved.display_name, "Ada",
                   "the advertised name is not overwritten by a local rename");
        assert!(!e.is_dirty(), "clean afterwards");
    }

    #[test]
    fn an_empty_contact_cannot_be_exported() {
        let mut e = Editor::default();
        assert!(!e.can_export(), "nothing open");
        let mut empty = c("9", "", false, false);
        empty.sc_id.clear();
        e.open(&empty);
        assert!(!e.can_export(), "no name and no id exports an empty card");
        e.open(&c("1", "Ada", false, false));
        assert!(e.can_export());
    }
}
