//! Contact data model — matches Core::Contact in Types.hpp.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ContactState {
    #[default]
    Pending = 0, // searched, never connected
    Known   = 1, // connected at least once
}

impl ContactState {
    pub fn from_int(i: i64) -> Self {
        match i { 1 => Self::Known, _ => Self::Pending }
    }
    pub fn to_int(&self) -> i64 {
        match self { Self::Pending => 0, Self::Known => 1 }
    }
}

/// Matches Core::Contact — every field maps directly to a users table column.
#[derive(Debug, Clone, Default)]
pub struct Contact {
    /// Stable sc-ID (e.g. "sc-32a51661") — primary key.
    pub id:           String,
    /// Display name stripped of IP/sc-ID suffix.
    pub display_name: String,
    /// User-set nickname (empty if none).
    pub nickname:     String,
    /// Notes from ContactEditorDialog.
    pub notes:        String,
    pub avatar_path:  String,
    pub vcard_uid:    String,
    pub online:       bool,
    pub state:        ContactState,
    pub last_seen:    Option<DateTime<Utc>>,
}

impl Contact {
    /// The name to display in the UI (nickname takes priority over display_name).
    pub fn display(&self) -> &str {
        if !self.nickname.is_empty() { &self.nickname } else { &self.display_name }
    }
}
