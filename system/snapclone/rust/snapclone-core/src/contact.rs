//! Contact model — the single source of truth for contact representation.
//!
//! All UI layers (Qt, egui, web) read this type. Any UI-specific presentation
//! (avatar image loading, badge animations) is added by the UI layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContactState {
    #[default]
    Pending, // sc-ID known, never connected
    Known,   // connected at least once, contact persisted
}

/// A peer contact. All fields are value-semantic — no Rc/Arc, no handles.
/// The UI renders from this; mutations go through AppCommand::UpdateContact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contact {
    /// Stable sc-ID ("sc-32a51661") — primary key everywhere
    pub id:           String,
    pub display_name: String,
    pub nickname:     String,   // user-set; empty = use display_name
    pub notes:        String,
    pub avatar_path:  String,
    pub online:       bool,
    pub state:        ContactState,
    pub last_seen:    Option<DateTime<Utc>>,
}

impl Contact {
    /// The name to show in the UI. Nickname takes priority.
    pub fn display(&self) -> &str {
        if !self.nickname.is_empty() { &self.nickname } else { &self.display_name }
    }

    /// True if this contact can receive a snap right now.
    pub fn is_reachable(&self) -> bool {
        self.online && self.state == ContactState::Known
    }
}
