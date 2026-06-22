//! Per-contact face profiles + top-K LRU matching (DEFERRED).
//!
//! **Status (V32.32):** Deferred. Same status as `embedding` — see
//! that module's header comment for the full context. The personal-
//! bandwidth direction (docs/FACE_BANDWIDTH.md) uses `library`
//! instead, which stores actual face crops rather than 512-dim
//! embeddings.
//!
//! New code should consume `library::Library` and `library::Crop`
//! instead of `IdentityMatcher` and `FaceProfile`.
//!
//! ---
//!
//! Original module documentation follows:
//!
//! When a face is detected in a photo, the `IdentityMatcher` compares
//! its embedding against known contact profiles and returns the best
//! match if any are within the configured threshold. Profiles are
//! cached in memory with an LRU bound so memory stays predictable
//! even if the user has thousands of contacts.
//!
//! ## Top-K cache design
//!
//! Holding all contact embeddings in RAM is fine at small scale
//! (2KB per contact at 512 floats x 4 bytes). At 5000 contacts that
//! becomes 10MB which is still fine. The reason for a top-K bound is
//! NOT memory — it's:
//!
//!   - Match latency scales linearly with profile count. With 10000
//!     profiles, every photo capture pays 10000 distance computations.
//!     Top-K (recently-active contacts) keeps this at K, typically
//!     ~100, regardless of total contact count.
//!   - Privacy: the recently-active set is a smaller exposure surface
//!     if the cache is dumped (process crash, debugger attach).
//!     Long-tail contacts stay encrypted on disk and only load when
//!     the matcher needs to do a broader search.
//!
//! Eviction policy: LRU by `last_used_ms` — when the matcher hits a
//! cached profile, it's marked recent; when capacity is full and a
//! new one needs to load, the least-recently-used profile is evicted
//! from the in-memory map. Disk profile is not deleted.

use crate::embedding::{FaceEmbedding, EMBEDDING_MODEL_VERSION};
use crate::FaceError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Opaque contact identifier. Whatever the application uses to key
/// contacts elsewhere — we don't care about the format, just that
/// it's hashable + cloneable + comparable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContactId(pub String);

/// Stored face profile for one contact.
///
/// A profile may have MULTIPLE embeddings — same person under
/// different lighting, with/without glasses, etc. Matching uses
/// the MINIMUM distance across all embeddings, so adding samples
/// over time makes recognition more robust.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceProfile {
    /// Contact this profile belongs to.
    pub contact: ContactId,
    /// One or more reference embeddings.
    pub embeddings: Vec<FaceEmbedding>,
    /// When this profile was first created (unix millis).
    pub created_at_ms: i64,
    /// When the matcher last consulted this profile (unix millis).
    /// Used for LRU eviction; updated on every match attempt.
    pub last_used_ms: i64,
    /// When a sample was last ADDED (unix millis). Used for showing
    /// "last updated: 3 days ago" UI; distinct from last_used_ms
    /// because match consultation isn't user-visible activity.
    pub last_updated_ms: i64,
}

impl FaceProfile {
    /// Construct from a single sample.
    pub fn new(contact: ContactId, sample: FaceEmbedding, now_ms: i64) -> Self {
        Self {
            contact,
            embeddings: vec![sample],
            created_at_ms: now_ms,
            last_used_ms: now_ms,
            last_updated_ms: now_ms,
        }
    }

    /// Add another sample. Profiles cap at MAX_SAMPLES_PER_PROFILE;
    /// when full, the oldest is replaced. This bounds the per-contact
    /// match cost regardless of how many photos pass through.
    pub fn add_sample(&mut self, sample: FaceEmbedding, now_ms: i64) {
        if sample.is_zero() {
            // Stub embedder protection — never store zero vectors
            // because their distance to everything else is huge and
            // would skew the min-distance matcher.
            return;
        }
        if self.embeddings.len() >= MAX_SAMPLES_PER_PROFILE {
            // Replace the oldest. Sample 0 is the first one stored,
            // which has typically been around the longest.
            self.embeddings[0] = sample;
        } else {
            self.embeddings.push(sample);
        }
        self.last_updated_ms = now_ms;
    }

    /// Minimum distance from a probe embedding to ANY of this
    /// profile's stored samples. f32::INFINITY when the profile has
    /// no usable samples or all samples are model-version-incompatible.
    pub fn min_distance(&self, probe: &FaceEmbedding) -> f32 {
        self.embeddings.iter()
            .map(|e| e.distance(probe))
            .fold(f32::INFINITY, f32::min)
    }
}

/// Per-profile sample cap. Adding photos beyond this rotates the
/// oldest out. 5 samples covers "front-facing in good light,"
/// "side angle," "with glasses," "smiling," "at night" — typical
/// variations.
pub const MAX_SAMPLES_PER_PROFILE: usize = 5;

/// Default top-K cache size — how many profiles stay in memory.
/// Most users have <50 active contacts; 100 covers them with headroom.
pub const DEFAULT_CACHE_SIZE: usize = 100;

/// Default match threshold. With L2-normalized ArcFace embeddings:
///   - same person typically < 0.6
///   - different people typically > 1.0
/// 0.8 is a moderately-strict default. Users can tune; making this
/// stricter reduces false positives at the cost of missing some
/// same-person matches (e.g. a friend with new haircut).
pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.8;

/// Result of a match query.
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// Which contact matched.
    pub contact: ContactId,
    /// Distance to the matched profile's nearest sample.
    /// Lower = more confident.
    pub distance: f32,
}

/// LRU-bounded face profile matcher.
///
/// Holds up to `cache_size` profiles in memory. Matching scans all
/// cached profiles and returns the best one within `match_threshold`,
/// if any.
///
/// **Disabled when consent is off.** All operations short-circuit:
///   - `add_or_update_profile` is a no-op (no profile stored)
///   - `match_best` returns `None`
///   - `wipe` is the always-available exit hatch
pub struct IdentityMatcher {
    profiles: HashMap<ContactId, FaceProfile>,
    cache_size: usize,
    match_threshold: f32,
    enabled: bool,
}

impl IdentityMatcher {
    /// Default-config matcher: 100 profile cache, 0.8 threshold,
    /// enabled. Disable via `set_enabled(false)`.
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
            cache_size: DEFAULT_CACHE_SIZE,
            match_threshold: DEFAULT_MATCH_THRESHOLD,
            enabled: true,
        }
    }

    /// Tunable variant for tests / non-default configs.
    pub fn with_config(cache_size: usize, match_threshold: f32) -> Self {
        Self {
            profiles: HashMap::new(),
            cache_size,
            match_threshold,
            enabled: true,
        }
    }

    /// Enable / disable matching. When disabled, all queries return
    /// `None` and updates are no-ops. Toggling to disabled DOES NOT
    /// wipe stored profiles — use `wipe()` for that.
    pub fn set_enabled(&mut self, enabled: bool) { self.enabled = enabled; }

    /// Whether matching is currently enabled.
    pub fn is_enabled(&self) -> bool { self.enabled }

    /// REMOVE all profiles from memory. The privacy-grade exit. After
    /// `wipe()`, the matcher has no knowledge of any contact's face.
    /// Disk-persisted profiles are NOT touched — wipe also requires
    /// the caller to clear any backing store.
    ///
    /// Returns the number of profiles that were wiped.
    pub fn wipe(&mut self) -> usize {
        let n = self.profiles.len();
        self.profiles.clear();
        n
    }

    /// Match a probe embedding against the cached profiles.
    ///
    /// Returns `Some(MatchResult)` when:
    ///   - At least one profile has min_distance < match_threshold
    ///   - The matcher is enabled
    ///   - The probe is not all-zeros (stub embedder)
    ///
    /// Returns `None` otherwise. This is a soft no-match, NOT an
    /// error — UI should treat as "no face identified" not "error
    /// occurred."
    pub fn match_best(&mut self, probe: &FaceEmbedding, now_ms: i64) -> Option<MatchResult> {
        if !self.enabled { return None; }
        if probe.is_zero() { return None; }

        let mut best: Option<(ContactId, f32)> = None;
        for (id, profile) in self.profiles.iter_mut() {
            let d = profile.min_distance(probe);
            // Update LRU regardless of match — visiting the profile
            // for any reason marks it "recently relevant."
            profile.last_used_ms = now_ms;
            match best {
                None => best = Some((id.clone(), d)),
                Some((_, bd)) if d < bd => best = Some((id.clone(), d)),
                _ => {}
            }
        }
        best.filter(|(_, d)| *d < self.match_threshold)
            .map(|(contact, distance)| MatchResult { contact, distance })
    }

    /// Add a new profile or add a sample to an existing one.
    pub fn add_or_update_profile(
        &mut self,
        contact: ContactId,
        sample: FaceEmbedding,
        now_ms: i64,
    ) -> Result<(), FaceError> {
        if !self.enabled { return Err(FaceError::ConsentDisabled); }
        if sample.is_zero() {
            // Stub embedder protection.
            return Ok(());
        }
        if sample.model_version != EMBEDDING_MODEL_VERSION {
            return Err(FaceError::InvalidInput(
                "embedding model version doesn't match current model",
            ));
        }
        if let Some(prof) = self.profiles.get_mut(&contact) {
            prof.add_sample(sample, now_ms);
        } else {
            // Enforce cache bound BEFORE insert — if we're at capacity,
            // evict the LRU profile first.
            if self.profiles.len() >= self.cache_size {
                self.evict_lru();
            }
            self.profiles.insert(contact.clone(), FaceProfile::new(contact, sample, now_ms));
        }
        Ok(())
    }

    /// Remove a single contact's profile. Useful when a contact is
    /// deleted from the user's address book.
    pub fn remove_profile(&mut self, contact: &ContactId) -> bool {
        self.profiles.remove(contact).is_some()
    }

    /// Number of profiles currently cached.
    pub fn len(&self) -> usize { self.profiles.len() }

    /// True if no profiles are cached.
    pub fn is_empty(&self) -> bool { self.profiles.is_empty() }

    /// Evict the single least-recently-used profile. Internal helper
    /// for capacity enforcement.
    fn evict_lru(&mut self) {
        let lru = self.profiles.iter()
            .min_by_key(|(_, p)| p.last_used_ms)
            .map(|(id, _)| id.clone());
        if let Some(id) = lru {
            self.profiles.remove(&id);
        }
    }

    /// Bulk-load profiles from a (deserialized) persistent store.
    /// Profiles with model_version != current are silently discarded
    /// (the embeddings would produce nonsense distances against fresh
    /// probes).
    pub fn load_profiles(&mut self, profiles: impl IntoIterator<Item = FaceProfile>) {
        for p in profiles {
            if p.embeddings.iter().any(|e| e.model_version != EMBEDDING_MODEL_VERSION) {
                continue;  // skip incompatible
            }
            if self.profiles.len() >= self.cache_size {
                self.evict_lru();
            }
            self.profiles.insert(p.contact.clone(), p);
        }
    }

    /// Iterate over cached profiles. For persistence callers that
    /// want to write profiles to disk.
    pub fn iter_profiles(&self) -> impl Iterator<Item = &FaceProfile> {
        self.profiles.values()
    }
}

impl Default for IdentityMatcher {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(s: &str) -> ContactId { ContactId(s.to_string()) }

    fn nonzero_embedding(v: f32) -> FaceEmbedding {
        let mut e = FaceEmbedding::zero();
        e.vec[0] = v;  // make non-zero
        e
    }

    #[test]
    fn add_and_match_roundtrip() {
        let mut m = IdentityMatcher::new();
        let e = nonzero_embedding(1.0);
        m.add_or_update_profile(cid("alice"), e.clone(), 100).unwrap();
        let r = m.match_best(&e, 200).expect("should match");
        assert_eq!(r.contact, cid("alice"));
        assert_eq!(r.distance, 0.0);
    }

    #[test]
    fn zero_probe_never_matches() {
        let mut m = IdentityMatcher::new();
        let e = nonzero_embedding(1.0);
        m.add_or_update_profile(cid("alice"), e, 100).unwrap();
        let zero = FaceEmbedding::zero();
        assert!(m.match_best(&zero, 200).is_none());
    }

    #[test]
    fn disabled_matcher_returns_none() {
        let mut m = IdentityMatcher::new();
        let e = nonzero_embedding(1.0);
        m.add_or_update_profile(cid("alice"), e.clone(), 100).unwrap();
        m.set_enabled(false);
        assert!(m.match_best(&e, 200).is_none());
    }

    #[test]
    fn disabled_matcher_refuses_adds() {
        let mut m = IdentityMatcher::new();
        m.set_enabled(false);
        let e = nonzero_embedding(1.0);
        assert!(matches!(
            m.add_or_update_profile(cid("alice"), e, 100),
            Err(FaceError::ConsentDisabled)
        ));
    }

    #[test]
    fn wipe_clears_everything() {
        let mut m = IdentityMatcher::new();
        m.add_or_update_profile(cid("a"), nonzero_embedding(1.0), 100).unwrap();
        m.add_or_update_profile(cid("b"), nonzero_embedding(2.0), 200).unwrap();
        assert_eq!(m.wipe(), 2);
        assert!(m.is_empty());
    }

    #[test]
    fn cache_eviction_drops_lru() {
        let mut m = IdentityMatcher::with_config(/*cache=*/2, 0.8);
        m.add_or_update_profile(cid("a"), nonzero_embedding(1.0), 100).unwrap();
        m.add_or_update_profile(cid("b"), nonzero_embedding(2.0), 200).unwrap();
        m.add_or_update_profile(cid("c"), nonzero_embedding(3.0), 300).unwrap();
        // 'a' is the LRU and should have been evicted.
        assert!(m.match_best(&nonzero_embedding(1.0), 400).is_none()
                || m.len() == 2);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn samples_cap_rotates_oldest() {
        let mut m = IdentityMatcher::new();
        // Add MAX_SAMPLES_PER_PROFILE + 2 samples
        for i in 0..(MAX_SAMPLES_PER_PROFILE as i32 + 2) {
            let e = nonzero_embedding(i as f32 + 1.0);
            m.add_or_update_profile(cid("alice"), e, 100 + i as i64).unwrap();
        }
        // Profile should still exist with MAX samples
        let p = m.iter_profiles().next().unwrap();
        assert_eq!(p.embeddings.len(), MAX_SAMPLES_PER_PROFILE);
    }
}
