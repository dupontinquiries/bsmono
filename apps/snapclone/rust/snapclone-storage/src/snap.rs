//! Snap data model — matches Core::Snap in Types.hpp.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapId(pub String);

impl SnapId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for SnapId {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone)]
pub struct Snap {
    pub id:              SnapId,
    pub sender_id:       String,
    pub recipient_id:    String,
    pub content_hash:    String,
    pub local_path:      String,
    pub duration_secs:   u32,
    pub sent_at:         DateTime<Utc>,
    pub delivered_at:    Option<DateTime<Utc>>,
    pub viewed:          bool,
    // Video fields (migration 11)
    pub encoded_duration_ms: u64,
    pub thumbnail_path:  String,
    pub has_audio:       bool,
    pub is_looping:      bool,
}

impl Snap {
    pub fn is_outbound(&self, self_id: &str) -> bool {
        self.sender_id == self_id
    }
    pub fn is_delivered(&self) -> bool {
        self.delivered_at.is_some()
    }
}
