//! Snap model — ephemeral media messages.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct SnapId(pub String);

impl SnapId {
    pub fn new() -> Self { Self(uuid::Uuid::new_v4().to_string()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaType { #[default] Image, Video }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snap {
    pub id:            SnapId,
    pub sender_id:     String,
    pub recipient_id:  String,
    pub content_hash:  String,
    pub local_path:    String,
    pub media_type:    MediaType,
    pub duration_secs: u32,
    pub sent_at:       DateTime<Utc>,
    pub delivered_at:  Option<DateTime<Utc>>,
    pub viewed:        bool,
}

impl Snap {
    pub fn is_inbound(&self, self_id: &str) -> bool { self.recipient_id == self_id }
    pub fn is_outbound(&self, self_id: &str) -> bool { self.sender_id == self_id }
    pub fn is_pending_delivery(&self) -> bool { self.delivered_at.is_none() }
}
