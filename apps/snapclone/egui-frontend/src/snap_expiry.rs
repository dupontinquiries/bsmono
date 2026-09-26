//! snap_expiry — how long a snap lives, and the dedup memory that bounds a
//! session. Both from `SnapRouter`.
//!
//! ## Type-specific TTL is disappearing-message behaviour, not tidiness
//!
//! *"Ephemeral text gets a much shorter, type-specific TTL than the generic
//! media retention window (disappearing-message behaviour). Default 24h."*
//!
//! A photo you were sent is a thing you might want next week; a message is a
//! conversation. Giving both the same 30-day window makes the app a chat log,
//! which is the opposite of what it is for — and giving both 24 hours would
//! delete the photos.
//!
//! Audio shares the short window with text, because a voice note **is** a
//! message that happens not to be typed.
//!
//! ## "Keep everything" is a sentinel, not a very long time
//!
//! *"Then snaps get no expiry at all (an invalid/empty expires_at, the same
//! 'never expires' sentinel that markSnapKept uses)."*
//!
//! Reusing the sentinel matters: a snap the user explicitly kept and a snap
//! under keep-everything are the same state, so one code path clears both and
//! neither can be purged by a rule that forgot about the other. Expressing
//! "never" as a far-future date would eventually arrive.
//!
//! ## The dedup cap clears wholesale, and the cost is stated
//!
//! *"Drop oldest dedup history — causes at most one redundant transfer."*
//!
//! A blunt clear rather than an LRU eviction, and the comment earns it by
//! naming the worst case. An LRU here would cost memory and bookkeeping to
//! avoid re-sending one snap, on a path where re-sending is already the
//! fallback behaviour.

/// Media kinds the expiry rule distinguishes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaType { Image, Video, Text, Audio }

impl MediaType {
    /// Does this get the short, conversational window?
    ///
    /// Audio counts: a voice note is a message that happens not to be typed,
    /// and keeping voice for a month while text vanishes in a day would be a
    /// privacy surprise rather than a feature.
    pub fn is_ephemeral(self) -> bool {
        matches!(self, MediaType::Text | MediaType::Audio)
    }
}

/// Default media retention, in days. Thirty, so the monthly export has
/// something to export.
pub const RETENTION_DAYS_DEFAULT: u32 = 30;
/// Default text/voice retention, in hours.
pub const TEXT_RETENTION_HOURS_DEFAULT: u32 = 24;
/// Bounds on the configurable text window: at least an hour, at most 30 days.
pub const TEXT_HOURS_MIN: u32 = 1;
pub const TEXT_HOURS_MAX: u32 = 720;

/// Per-peer known-hash memory before it is dropped.
pub const MAX_KNOWN_HASHES: usize = 500;

/// Retention settings as configured.
#[derive(Clone, Copy, Debug)]
pub struct Retention {
    pub days: u32,
    pub text_hours: u32,
    /// Power-user setting: nothing expires.
    pub keep_everything: bool,
}

impl Default for Retention {
    fn default() -> Self {
        Retention { days: RETENTION_DAYS_DEFAULT,
                    text_hours: TEXT_RETENTION_HOURS_DEFAULT,
                    keep_everything: false }
    }
}

impl Retention {
    /// Text window in seconds, clamped as the original clamps it.
    ///
    /// The floor stops a zero from making every message vanish on arrival; the
    /// ceiling stops "ephemeral" quietly meaning years.
    pub fn text_secs(&self) -> u64 {
        self.text_hours.clamp(TEXT_HOURS_MIN, TEXT_HOURS_MAX) as u64 * 3600
    }

    pub fn media_secs(&self) -> u64 { self.days.max(1) as u64 * 24 * 3600 }
}

/// When a snap expires.
///
/// `None` is the **never-expires sentinel** — the same state a user-kept snap
/// has, so one purge path handles both and neither can be missed by a rule that
/// forgot about the other.
pub fn expiry_at(sent_at_secs: u64, media: MediaType, r: &Retention) -> Option<u64> {
    if r.keep_everything { return None; }
    let window = if media.is_ephemeral() { r.text_secs() } else { r.media_secs() };
    Some(sent_at_secs.saturating_add(window))
}

/// Has this snap expired?
///
/// A `None` expiry never has. Written as a match rather than a comparison
/// against a far-future number, because a far-future number eventually arrives.
pub fn is_expired(expires_at: Option<u64>, now_secs: u64) -> bool {
    match expires_at { None => false, Some(t) => now_secs >= t }
}

/// The user pressed keep. Clears the expiry to the same sentinel.
pub fn mark_kept(expires_at: &mut Option<u64>) { *expires_at = None; }

/// Per-peer memory of hashes already exchanged.
///
/// Bounded by a wholesale clear rather than an LRU: the comment names the worst
/// case as *"at most one redundant transfer"*, and paying LRU bookkeeping to
/// avoid re-sending one snap is not a trade worth making on a path where
/// re-sending is already the fallback.
#[derive(Clone, Debug, Default)]
pub struct KnownHashes {
    set: std::collections::HashSet<String>,
    /// How many times the cap has been hit — worth surfacing, because a session
    /// clearing this often is one where dedup is not doing its job.
    pub clears: u32,
}

impl KnownHashes {
    pub fn len(&self) -> usize { self.set.len() }
    pub fn is_empty(&self) -> bool { self.set.is_empty() }
    pub fn contains(&self, hash: &str) -> bool { self.set.contains(hash) }

    /// Record a hash, clearing first if the cap is reached.
    pub fn insert(&mut self, hash: &str) {
        if self.set.len() >= MAX_KNOWN_HASHES {
            self.set.clear();
            self.clears += 1;
        }
        self.set.insert(hash.to_string());
    }
}

/// The capture time and kind of a stored snap, from its file name.
///
/// The app writes its snaps as `snap-<unix ms>.<ext>` (`save_snap` in the
/// binary), so the name carries the one fact retention needs. Anything else in
/// the directory -- the exported code SVG, a user's own file -- is `None` and is
/// never a purge candidate: a sweep that deletes by pattern must only ever
/// match the pattern it wrote.
pub fn stored_snap(name: &str) -> Option<(u64, MediaType)> {
    let rest = name.strip_prefix("snap-")?;
    let (ms, ext) = rest.split_once('.')?;
    if ms.is_empty() || !ms.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let media = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "ppm" | "avif" => MediaType::Image,
        "mp4" | "webm" | "mkv" | "ivf" | "y4m" => MediaType::Video,
        _ => return None,
    };
    Some((ms.parse().ok()?, media))
}

/// Which stored snaps retention says to delete now. Names not written by the
/// app are never returned; with `keep_everything` nothing is.
pub fn due_for_purge<'a>(names: impl IntoIterator<Item = &'a str>, now_secs: u64, r: &Retention) -> Vec<String> {
    names.into_iter()
        .filter(|n| stored_snap(n).is_some_and(|(ms, media)| is_expired(expiry_at(ms / 1000, media, r), now_secs)))
        .map(|n| n.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    /// Only the app's own snap files, only once they are older than the window,
    /// never with keep_everything, and never the code SVG beside them.
    #[test]
    fn the_sweep_deletes_only_expired_snaps_the_app_wrote() {
        let day = 24 * 3600u64;
        let now = 1000 * day;
        let at = |days_ago: u64| (now - days_ago * day) * 1000;
        let names = vec![
            format!("snap-{}.jpg", at(31)),
            format!("snap-{}.jpg", at(29)),
            format!("snap-{}.mp4", at(40)),
            "sc-aabbccdd.svg".to_string(),
            "snap-notanumber.jpg".to_string(),
            format!("snap-{}.txt", at(400)),
            "holiday.jpg".to_string(),
        ];
        let r = Retention { days: 30, ..Retention::default() };
        let due = due_for_purge(names.iter().map(|s| s.as_str()), now, &r);
        assert_eq!(due, vec![names[0].clone(), names[2].clone()]);
        let keep = Retention { keep_everything: true, ..r };
        assert!(due_for_purge(names.iter().map(|s| s.as_str()), now, &keep).is_empty());
        let week = Retention { days: 7, ..r };
        assert_eq!(due_for_purge(names.iter().map(|s| s.as_str()), now, &week).len(), 3, "the day count is honoured");
    }

    use super::*;

    const DAY: u64 = 24 * 3600;

    #[test]
    fn a_message_vanishes_in_a_day_and_a_photo_lasts_a_month() {
        // "Disappearing-message behaviour." Giving both the same window makes
        // the app a chat log, which is the opposite of what it is for — and
        // giving both 24 hours would delete the photos.
        let r = Retention::default();
        assert_eq!(expiry_at(0, MediaType::Text, &r), Some(DAY));
        assert_eq!(expiry_at(0, MediaType::Image, &r), Some(30 * DAY));
    }

    #[test]
    fn a_voice_note_is_a_message_not_media() {
        // Keeping voice for a month while text vanishes in a day would be a
        // privacy surprise rather than a feature.
        assert!(MediaType::Audio.is_ephemeral());
        assert!(MediaType::Text.is_ephemeral());
        assert!(!MediaType::Video.is_ephemeral());
        let r = Retention::default();
        assert_eq!(expiry_at(0, MediaType::Audio, &r), expiry_at(0, MediaType::Text, &r));
    }

    #[test]
    fn keep_everything_gives_the_never_sentinel_not_a_far_future_date() {
        // A far-future date eventually arrives. None never does.
        let r = Retention { keep_everything: true, ..Default::default() };
        assert_eq!(expiry_at(0, MediaType::Image, &r), None);
        assert_eq!(expiry_at(0, MediaType::Text, &r), None);
        assert!(!is_expired(None, u64::MAX));
    }

    #[test]
    fn a_kept_snap_reaches_the_same_state_as_keep_everything() {
        // Reusing the sentinel is what lets ONE purge path handle both, so
        // neither can be missed by a rule that forgot about the other.
        let r = Retention::default();
        let mut e = expiry_at(0, MediaType::Image, &r);
        assert!(e.is_some());
        mark_kept(&mut e);
        assert_eq!(e, expiry_at(0, MediaType::Image,
                                &Retention { keep_everything: true, ..Default::default() }));
    }

    #[test]
    fn the_text_window_is_clamped_at_both_ends() {
        // A zero would make every message vanish on arrival; an unbounded value
        // would let "ephemeral" quietly mean years.
        let zero = Retention { text_hours: 0, ..Default::default() };
        assert_eq!(zero.text_secs(), 3600, "zero hours deleted messages on arrival");
        let huge = Retention { text_hours: 100_000, ..Default::default() };
        assert_eq!(huge.text_secs(), TEXT_HOURS_MAX as u64 * 3600);
    }

    #[test]
    fn a_zero_day_media_window_still_keeps_a_day() {
        // Same reasoning on the media side: a misconfigured zero should not
        // purge photos as they arrive.
        let r = Retention { days: 0, ..Default::default() };
        assert_eq!(r.media_secs(), DAY);
    }

    #[test]
    fn expiry_is_inclusive_at_the_boundary() {
        // A snap at exactly its expiry second is expired; leaving it would make
        // the purge depend on how often the timer happens to run.
        assert!(is_expired(Some(100), 100));
        assert!(!is_expired(Some(100), 99));
    }

    #[test]
    fn a_far_future_send_time_does_not_wrap() {
        // A peer with a broken clock can send a timestamp near u64 max, and
        // wrapping would produce an expiry in the PAST — deleting the snap the
        // moment it arrived.
        let r = Retention::default();
        assert_eq!(expiry_at(u64::MAX, MediaType::Image, &r), Some(u64::MAX));
        assert!(!is_expired(expiry_at(u64::MAX, MediaType::Image, &r), 1_000));
    }

    #[test]
    fn the_dedup_memory_is_bounded() {
        // "Cap per-peer hash memory to avoid unbounded growth in long
        // sessions" — a week-long session with a chatty peer would otherwise
        // grow without limit.
        let mut k = KnownHashes::default();
        for i in 0..MAX_KNOWN_HASHES * 2 { k.insert(&format!("h{i}")); }
        assert!(k.len() <= MAX_KNOWN_HASHES);
        assert!(k.clears >= 1);
    }

    #[test]
    fn the_clear_is_wholesale_and_costs_at_most_one_redundant_transfer() {
        // The comment earns the blunt clear by naming its worst case. An LRU
        // would cost memory and bookkeeping to avoid re-sending one snap, on a
        // path where re-sending is already the fallback.
        let mut k = KnownHashes::default();
        for i in 0..MAX_KNOWN_HASHES { k.insert(&format!("h{i}")); }
        assert!(k.contains("h0"));
        k.insert("overflow");
        assert!(!k.contains("h0"), "expected a wholesale clear");
        assert_eq!(k.len(), 1);
        assert!(k.contains("overflow"), "the triggering hash must survive its own clear");
    }

    #[test]
    fn dedup_recognises_a_hash_it_has_seen() {
        // The actual job: not re-sending what the peer already has.
        let mut k = KnownHashes::default();
        k.insert("abc");
        assert!(k.contains("abc"));
        assert!(!k.contains("def"));
    }

    #[test]
    fn re_inserting_a_known_hash_does_not_grow_the_set() {
        // Otherwise a peer repeatedly announcing the same snap would trigger
        // the cap and throw away real history.
        let mut k = KnownHashes::default();
        for _ in 0..1_000 { k.insert("same"); }
        assert_eq!(k.len(), 1);
        assert_eq!(k.clears, 0);
    }
}
