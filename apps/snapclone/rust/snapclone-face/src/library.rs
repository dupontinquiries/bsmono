//! Local face crop library.
//!
//! Phase 1 of the face-bandwidth project (see docs/FACE_BANDWIDTH.md).
//! Stores cropped face images on-device under labels:
//!
//!   - `Label::SelfFace`     — the user's own face (front-camera selfies)
//!   - `Label::Contact(id)`  — a face from a snap received from contact id
//!
//! The library is the data foundation for future Phase 2 (reference-
//! based delta compression) and Phase 3 (on-device personal model).
//! It has direct standalone value too: with a labeled corpus of faces
//! the user has captured or received, downstream features like photo
//! organization, lighting analysis, and composition hints become
//! possible.
//!
//! What the library does NOT do:
//!
//!   - Run inference. Crops are stored as raw image bytes; the
//!     detector that produced them lives in `detector.rs`.
//!   - Recognize identity. The label of an incoming snap comes from
//!     the snap's sender field (cryptographically authenticated by
//!     the transport), not from any face-matching logic.
//!   - Transmit anything. The library is local-only.
//!
//! Storage policies:
//!
//!   - Per-label LRU cap (default `MAX_CROPS_PER_LABEL`). When full,
//!     adding a new crop evicts the oldest.
//!   - Total byte budget (default `DEFAULT_LIBRARY_BUDGET_BYTES`).
//!     When over budget, LRU eviction across the whole library.
//!   - On-disk persistence is up to the host application — this crate
//!     provides an in-memory `InMemoryLibrary` for tests and a
//!     `Library` trait for production-backed implementations.
//!
//! Consent gating:
//!
//!   - All write operations refuse with `FaceError::ConsentDisabled`
//!     when consent is off.
//!   - All read operations return empty results when consent is off
//!     (rather than refusing — calling code doesn't need a special
//!     consent-aware path).
//!   - `wipe()` is the privacy exit-hatch, callable any time.

use crate::FaceError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default cap on stored crops per label (per-person).
///
/// 50 covers the typical "20-50 samples to get useful recognition"
/// guideline with headroom for variation (lighting, angle, expression).
/// Each crop is ~5-30KB compressed, so 50 crops × 50 contacts ≈ 75MB
/// at the high end — within the default total budget below.
pub const MAX_CROPS_PER_LABEL: usize = 50;

/// Default total library byte budget. 50MB chosen to be invisible on
/// modern phones while allowing the long-tail of contacts to be
/// represented. User can adjust via settings.
pub const DEFAULT_LIBRARY_BUDGET_BYTES: usize = 50 * 1024 * 1024;

/// What a crop is labeled with.
///
/// Wire-encoding-friendly: `to_wire`/`from_wire` produce a stable
/// string representation used as the storage key prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Label {
    /// User's own face, captured via front camera.
    SelfFace,
    /// A face from a snap received from this contact. The contact id
    /// is the cryptographic identifier used by the rest of SnapClone
    /// (typically a public-key fingerprint).
    Contact(String),
}

impl Label {
    /// Stable string form for use as a storage key.
    pub fn to_wire(&self) -> String {
        match self {
            Label::SelfFace      => "self".to_string(),
            Label::Contact(id)   => format!("contact:{}", id),
        }
    }

    /// Round-trip from `to_wire`'s output. Unknown formats return None.
    pub fn from_wire(s: &str) -> Option<Self> {
        if s == "self" { return Some(Label::SelfFace); }
        s.strip_prefix("contact:").map(|id| Label::Contact(id.to_string()))
    }
}

/// One stored crop with its provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crop {
    /// What/who this crop is of.
    pub label: Label,

    /// 32-byte stable content identifier. SHA-256 of the canonical
    /// crop bytes; the same crop content always produces the same id.
    /// Used for de-duplication on insert and for the Phase 2
    /// reference-negotiation protocol.
    pub content_hash: [u8; 32],

    /// Encoded image bytes (JPEG or PNG; format inferred from header
    /// on decode). The library never decodes — it just stores opaque
    /// bytes. Producers should encode at a quality reasonable for the
    /// downstream uses (matching, future reference frames).
    pub bytes: Vec<u8>,

    /// Width of the crop in pixels.
    pub width: u32,
    /// Height of the crop in pixels.
    pub height: u32,

    /// Unix millis when this crop was first added to the library.
    pub captured_at_ms: i64,

    /// Unix millis of the last time this crop was retrieved or matched
    /// against. Updated on every read; basis for LRU eviction.
    pub last_used_ms: i64,

    /// Optional per-crop quality score in [0.0, 1.0]. Producers may
    /// fill this in based on detector confidence × sharpness × face
    /// size. 0.0 means "unknown" not "bad" so consumers should treat
    /// 0.0 as "no quality info, use as-is."
    pub quality: f32,
}

impl Crop {
    /// Approximate storage cost. Used by budget enforcement.
    /// Includes the byte payload plus a fixed overhead estimate
    /// for the metadata (label string, hash, timestamps).
    pub fn approx_bytes(&self) -> usize {
        self.bytes.len() + 256 // metadata overhead estimate
    }
}

/// Library backend trait. Different storage backends (in-memory,
/// sled, sqlite, encrypted-file) can implement this.
///
/// All methods are `&self` (interior mutability) so a single
/// `Library` instance can be safely shared across threads via Arc.
/// Implementations should serialize access internally (Mutex,
/// RwLock, or transactional backend).
pub trait Library: Send + Sync {
    /// Add or update a crop. If a crop with the same `content_hash`
    /// already exists under the same label, the existing entry is
    /// updated (last_used_ms refreshed, quality maybe updated) rather
    /// than duplicated.
    ///
    /// Returns `Err(ConsentDisabled)` when consent is off.
    fn add(&self, crop: Crop) -> Result<(), FaceError>;

    /// Retrieve up to `limit` crops with the given label, ordered by
    /// most-recent first. Empty result when consent is off OR no
    /// crops exist.
    fn get_by_label(&self, label: &Label, limit: usize) -> Vec<Crop>;

    /// Look up a single crop by content hash. None if not found OR
    /// consent is off.
    fn get_by_hash(&self, content_hash: &[u8; 32]) -> Option<Crop>;

    /// All labels currently represented in the library, with sample
    /// counts. Useful for diagnostic UI showing "you have 47 selfies
    /// stored, and 12 photos of Alice, 3 of Bob" etc. Returns empty
    /// when consent is off.
    fn label_counts(&self) -> HashMap<Label, usize>;

    /// Total bytes currently stored.
    fn total_bytes(&self) -> usize;

    /// Wipe everything. Returns the number of crops removed.
    /// Always callable, regardless of consent state — this IS the
    /// consent-withdrawal action.
    fn wipe(&self) -> usize;

    /// Enable / disable the library. When disabled, `add` refuses
    /// and reads return empty. Disabling does NOT wipe; call wipe()
    /// for that.
    fn set_enabled(&self, enabled: bool);

    /// Whether the library is currently enabled.
    fn is_enabled(&self) -> bool;
}

/// In-memory implementation of `Library`.
///
/// Backed by a Mutex<...>. Good for tests, and viable for production
/// on devices where face crops shouldn't persist across restarts
/// (extra-privacy mode). Production builds with persistence will
/// implement `Library` against a real on-disk store.
pub struct InMemoryLibrary {
    inner: std::sync::Mutex<LibraryInner>,
}

struct LibraryInner {
    // Keyed by content_hash for fast hash-based lookup. The value
    // includes its own label so we can iterate by label too.
    by_hash: HashMap<[u8; 32], Crop>,
    enabled: bool,
    budget_bytes: usize,
}

impl InMemoryLibrary {
    /// New library with the default budget, enabled.
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_LIBRARY_BUDGET_BYTES)
    }

    /// New library with a custom byte budget.
    pub fn with_budget(budget_bytes: usize) -> Self {
        Self {
            inner: std::sync::Mutex::new(LibraryInner {
                by_hash: HashMap::new(),
                enabled: true,
                budget_bytes,
            }),
        }
    }

    /// Evict crops from a single label down to the per-label cap.
    /// Returns the number of crops removed.
    fn enforce_per_label_cap(inner: &mut LibraryInner, label: &Label) -> usize {
        let mut for_label: Vec<([u8; 32], i64)> = inner.by_hash.iter()
            .filter(|(_, c)| &c.label == label)
            .map(|(h, c)| (*h, c.last_used_ms))
            .collect();
        if for_label.len() <= MAX_CROPS_PER_LABEL { return 0; }
        for_label.sort_by_key(|(_, ts)| *ts);  // oldest first
        let to_remove = for_label.len() - MAX_CROPS_PER_LABEL;
        let mut removed = 0;
        for (h, _) in for_label.into_iter().take(to_remove) {
            inner.by_hash.remove(&h);
            removed += 1;
        }
        removed
    }

    /// Evict globally down to total budget. Returns number removed.
    fn enforce_total_budget(inner: &mut LibraryInner) -> usize {
        let total: usize = inner.by_hash.values().map(|c| c.approx_bytes()).sum();
        if total <= inner.budget_bytes { return 0; }
        let mut entries: Vec<([u8; 32], i64, usize)> = inner.by_hash.iter()
            .map(|(h, c)| (*h, c.last_used_ms, c.approx_bytes()))
            .collect();
        entries.sort_by_key(|(_, ts, _)| *ts);
        let mut current = total;
        let mut removed = 0;
        for (h, _, sz) in entries {
            if current <= inner.budget_bytes { break; }
            inner.by_hash.remove(&h);
            current = current.saturating_sub(sz);
            removed += 1;
        }
        removed
    }
}

impl Default for InMemoryLibrary {
    fn default() -> Self { Self::new() }
}

impl Library for InMemoryLibrary {
    fn add(&self, crop: Crop) -> Result<(), FaceError> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.enabled {
            return Err(FaceError::ConsentDisabled);
        }
        // De-dup: if same content_hash already present, update timestamp
        // and quality (in case the new sample has a better quality
        // score) rather than store a duplicate.
        if let Some(existing) = inner.by_hash.get_mut(&crop.content_hash) {
            existing.last_used_ms = crop.captured_at_ms.max(existing.last_used_ms);
            if crop.quality > existing.quality {
                existing.quality = crop.quality;
            }
            return Ok(());
        }
        let label = crop.label.clone();
        inner.by_hash.insert(crop.content_hash, crop);
        // Enforce per-label cap first (so the global pass sees the
        // post-cap byte usage and doesn't double-evict).
        Self::enforce_per_label_cap(&mut inner, &label);
        Self::enforce_total_budget(&mut inner);
        Ok(())
    }

    fn get_by_label(&self, label: &Label, limit: usize) -> Vec<Crop> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.enabled { return Vec::new(); }
        let now = chrono_now_ms();
        let mut matches: Vec<Crop> = inner.by_hash.values()
            .filter(|c| &c.label == label)
            .cloned()
            .collect();
        matches.sort_by_key(|c| std::cmp::Reverse(c.last_used_ms));
        matches.truncate(limit);
        // Touch — update last_used on the ones we returned.
        for c in &matches {
            if let Some(stored) = inner.by_hash.get_mut(&c.content_hash) {
                stored.last_used_ms = now;
            }
        }
        matches
    }

    fn get_by_hash(&self, content_hash: &[u8; 32]) -> Option<Crop> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.enabled { return None; }
        let now = chrono_now_ms();
        let crop = inner.by_hash.get_mut(content_hash)?;
        crop.last_used_ms = now;
        Some(crop.clone())
    }

    fn label_counts(&self) -> HashMap<Label, usize> {
        let inner = self.inner.lock().unwrap();
        if !inner.enabled { return HashMap::new(); }
        let mut counts = HashMap::new();
        for c in inner.by_hash.values() {
            *counts.entry(c.label.clone()).or_insert(0) += 1;
        }
        counts
    }

    fn total_bytes(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.by_hash.values().map(|c| c.approx_bytes()).sum()
    }

    fn wipe(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let n = inner.by_hash.len();
        inner.by_hash.clear();
        n
    }

    fn set_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.enabled = enabled;
    }

    fn is_enabled(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.enabled
    }
}

/// Minimal "now in millis" helper using std::time. Kept private so
/// tests can substitute deterministic timestamps via Crop.captured_at_ms.
fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_crop(label: Label, hash_byte: u8, ts: i64, size: usize) -> Crop {
        Crop {
            label,
            content_hash: [hash_byte; 32],
            bytes: vec![0; size],
            width: 112, height: 112,
            captured_at_ms: ts,
            last_used_ms: ts,
            quality: 0.5,
        }
    }

    #[test]
    fn label_wire_roundtrip() {
        assert_eq!(Label::from_wire(&Label::SelfFace.to_wire()), Some(Label::SelfFace));
        let alice = Label::Contact("alice-pk-abc".into());
        assert_eq!(Label::from_wire(&alice.to_wire()), Some(alice));
        assert_eq!(Label::from_wire("garbage"), None);
    }

    #[test]
    fn add_and_get_roundtrip() {
        let lib = InMemoryLibrary::new();
        lib.add(make_crop(Label::SelfFace, 1, 100, 1000)).unwrap();
        let got = lib.get_by_label(&Label::SelfFace, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].content_hash, [1; 32]);
    }

    #[test]
    fn duplicate_hash_dedups_rather_than_storing() {
        let lib = InMemoryLibrary::new();
        lib.add(make_crop(Label::SelfFace, 1, 100, 1000)).unwrap();
        lib.add(make_crop(Label::SelfFace, 1, 200, 1000)).unwrap();
        assert_eq!(lib.get_by_label(&Label::SelfFace, 10).len(), 1);
    }

    #[test]
    fn disabled_library_refuses_writes_and_yields_empty_reads() {
        let lib = InMemoryLibrary::new();
        lib.set_enabled(false);
        assert!(matches!(
            lib.add(make_crop(Label::SelfFace, 1, 100, 1000)),
            Err(FaceError::ConsentDisabled)
        ));
        // Enable, add, disable, then reads should be empty.
        lib.set_enabled(true);
        lib.add(make_crop(Label::SelfFace, 2, 200, 1000)).unwrap();
        lib.set_enabled(false);
        assert!(lib.get_by_label(&Label::SelfFace, 10).is_empty());
        assert!(lib.get_by_hash(&[2; 32]).is_none());
        assert!(lib.label_counts().is_empty());
    }

    #[test]
    fn wipe_works_when_disabled() {
        // Sequence: add while enabled, then disable, then wipe must
        // still clear everything (consent withdrawal scenario).
        let lib = InMemoryLibrary::new();
        lib.add(make_crop(Label::SelfFace, 1, 100, 1000)).unwrap();
        lib.add(make_crop(Label::SelfFace, 2, 200, 1000)).unwrap();
        lib.set_enabled(false);
        assert_eq!(lib.wipe(), 2);
    }

    #[test]
    fn per_label_cap_evicts_oldest() {
        let lib = InMemoryLibrary::new();
        // Add MAX_CROPS_PER_LABEL + 5 distinct crops under one label.
        for i in 0..(MAX_CROPS_PER_LABEL as u8 + 5) {
            lib.add(make_crop(
                Label::SelfFace, i, 100 + i as i64, 1000,
            )).unwrap();
        }
        let counts = lib.label_counts();
        assert_eq!(counts.get(&Label::SelfFace).copied(), Some(MAX_CROPS_PER_LABEL));
    }

    #[test]
    fn total_budget_evicts_across_labels() {
        // 3KB budget, four 1KB crops across two labels → one evicted.
        let lib = InMemoryLibrary::with_budget(3500);
        lib.add(make_crop(Label::SelfFace, 1, 100, 1000)).unwrap();
        lib.add(make_crop(Label::Contact("bob".into()), 2, 200, 1000)).unwrap();
        lib.add(make_crop(Label::SelfFace, 3, 300, 1000)).unwrap();
        lib.add(make_crop(Label::Contact("bob".into()), 4, 400, 1000)).unwrap();
        // Crop 1 (oldest) should be gone; total <= budget.
        let total = lib.total_bytes();
        assert!(total <= 3500, "expected total <= budget, got {}", total);
        assert!(lib.get_by_hash(&[1; 32]).is_none());
    }

    #[test]
    fn label_counts_reflects_contents() {
        let lib = InMemoryLibrary::new();
        lib.add(make_crop(Label::SelfFace, 1, 100, 500)).unwrap();
        lib.add(make_crop(Label::SelfFace, 2, 200, 500)).unwrap();
        lib.add(make_crop(Label::Contact("a".into()), 3, 300, 500)).unwrap();
        let counts = lib.label_counts();
        assert_eq!(counts.get(&Label::SelfFace).copied(), Some(2));
        assert_eq!(counts.get(&Label::Contact("a".into())).copied(), Some(1));
    }
}
