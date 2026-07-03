//! Contact list UI contract.
//!
//! Separates "what data a contact card needs" from "how it's rendered".

use snapclone_core::Contact;

/// Presentation data for a single contact card.
/// Derived from `Contact` by the UI layer.
#[derive(Debug, Clone)]
pub struct ContactListItem {
    pub id:          String,
    pub display:     String, // nickname or display_name
    pub initial:     String, // first char for avatar circle
    pub online:      bool,
    pub last_seen:   Option<String>, // human-relative time (e.g. "5 min ago")
    pub badge_count: u32,   // unread snaps from this contact
}

impl From<&Contact> for ContactListItem {
    fn from(c: &Contact) -> Self {
        let display = c.display().to_owned();
        let initial = display.chars().next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_default();
        Self {
            id:          c.id.clone(),
            display,
            initial,
            online:      c.online,
            last_seen:   None, // caller fills in relative time
            badge_count: 0,
        }
    }
}

/// User actions available on a contact card.
#[derive(Debug, Clone)]
pub enum ContactAction {
    Tap,         // primary: open snap composer
    LongPress,   // secondary: show context menu
    Edit,        // open ContactEditorDialog
    Delete,      // delete with confirmation
    SendSnap,    // direct snap send
}

/// Contact list host. Implement for your UI framework.
pub trait ContactList {
    /// Replace the entire contact list (called at startup and on filter change).
    fn set_contacts(&mut self, items: Vec<ContactListItem>);

    /// Add or update a single contact (called on PeerConnected / ContactUpdated).
    fn upsert_contact(&mut self, item: ContactListItem);

    /// Remove a contact by id.
    fn remove_contact(&mut self, id: &str);

    /// Mark a contact as online/offline without rebuilding the list.
    fn set_online(&mut self, id: &str, online: bool);

    /// Set the unread badge count on a contact.
    fn set_badge(&mut self, id: &str, count: u32);

    /// Register a callback for contact actions.
    fn on_action(&mut self, f: Box<dyn Fn(&str, ContactAction) + Send + 'static>);

    /// Currently selected contact id (if any).
    fn selected_id(&self) -> Option<String>;
}
