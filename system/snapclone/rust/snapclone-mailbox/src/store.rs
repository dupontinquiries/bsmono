//! MailboxStore — local cache of pending mailbox entries.
//!
//! The DHT is authoritative; this is just a local cache for:
//! - Entries we've published (track delivery status)
//! - Entries we've fetched (know what to retrieve)

use crate::entry::MailboxEntry;
use std::collections::HashMap;
use tracing::{debug, info};

#[derive(Debug, Default)]
pub struct MailboxStore {
    /// snap_id → entry
    outbox: HashMap<String, MailboxEntry>,
    /// snap_id → entry (received, pending fetch)
    inbox:  HashMap<String, MailboxEntry>,
}

impl MailboxStore {
    pub fn new() -> Self { Self::default() }

    /// Record that we published a mailbox entry for a snap.
    pub fn record_published(&mut self, entry: MailboxEntry) {
        info!("Mailbox: published {} for {}", entry.snap_id, entry.recipient_id);
        self.outbox.insert(entry.snap_id.clone(), entry);
    }

    /// Record that we found a mailbox entry for us.
    pub fn record_received(&mut self, entry: MailboxEntry) {
        info!("Mailbox: received entry {} from {}", entry.snap_id, entry.sender_id);
        self.inbox.insert(entry.snap_id.clone(), entry);
    }

    /// Mark a snap as delivered (remove from outbox).
    pub fn mark_delivered(&mut self, snap_id: &str) {
        if self.outbox.remove(snap_id).is_some() {
            debug!("Mailbox: {} delivered", snap_id);
        }
    }

    /// Mark a snap as fetched (remove from inbox).
    pub fn mark_fetched(&mut self, snap_id: &str) {
        if self.inbox.remove(snap_id).is_some() {
            debug!("Mailbox: {} fetched", snap_id);
        }
    }

    /// Pending outbox entries (published but not yet confirmed delivered).
    pub fn pending_outbox(&self) -> Vec<&MailboxEntry> {
        self.outbox.values().filter(|e| e.is_alive()).collect()
    }

    /// Inbox entries waiting to be fetched.
    pub fn pending_inbox(&self) -> Vec<&MailboxEntry> {
        self.inbox.values().filter(|e| e.is_alive()).collect()
    }

    /// Remove expired entries from both stores.
    pub fn gc(&mut self) {
        self.outbox.retain(|_, e| e.is_alive());
        self.inbox.retain(|_, e| e.is_alive());
    }
}
