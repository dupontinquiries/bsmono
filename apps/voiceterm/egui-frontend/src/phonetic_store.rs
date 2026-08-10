//! phonetic_store — the learning policy from `phonetic.rs`.
//!
//! Voiceterm learns a per-speaker vocabulary: when you accept a transcription
//! it keeps the MFCC fingerprint, and next session recognises that phrase from
//! your voice rather than a general model. The signal processing belongs with
//! the audio thread; **what is kept, and what is thrown away, is policy** — and
//! that policy is the difference between a store that gets better with use and
//! one that fills with noise.
//!
//! ## Two-tier eviction, per style then per label
//!
//! v0.1.109 raised the per-label cap from 3 to 6 *"to make room for
//! multi-style fingerprints (normal, whispered, energetic, noisy, fragmented,
//! sleepy)"*, with a per-style cap of 2 inside it. The reason both exist: a
//! user who always speaks normally would otherwise fill all six slots with one
//! style and never build a whispered fingerprint, so the phrase they say quietly
//! at night never matches.
//!
//! ## Eviction ranks by (match_count, created_at)
//!
//! *"so unused styles get pruned naturally"*. Least-matched first, oldest as
//! the tie-break. Ranking by age alone would evict a rarely-used but reliable
//! fingerprint in favour of a recent bad capture; ranking by matches alone
//! would keep a lucky early hit forever.
//!
//! ## The pitch index is why the cap could grow
//!
//! The total cap went from 200 to 1000 *"along with the pitch-index addition:
//! with O(bin_size) lookup instead of O(N) scan, we can afford a larger DB
//! without latency degradation"*. 50Hz bins, ±1 bin at query time — *"wide
//! enough to absorb intra-speaker variation (typically ±30Hz) while still
//! skipping ~80% of cross-speaker fingerprints"*.

/// Fingerprints kept per phrase, across all styles.
pub const MAX_PER_LABEL: usize = 6;
/// Fingerprints kept per phrase per style.
pub const MAX_PER_STYLE: usize = 2;
/// Total store cap.
pub const TOTAL_CAP: usize = 1000;
/// Fingerprints unmatched for this long are dropped.
pub const STALE_DAYS: u64 = 7;
/// Pitch bin width.
pub const PITCH_BIN_HZ: f32 = 50.0;
/// Bin for a pitch that could not be estimated.
pub const PITCH_BIN_UNKNOWN: u16 = u16::MAX;

/// How an utterance was spoken.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style { Normal, Whispered, Energetic, Noisy, Fragmented, Sleepy }

/// One learned fingerprint, minus the coefficients themselves.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    pub style: Style,
    pub pitch_bin: u16,
    pub match_count: u32,
    pub created_day: u64,
    pub last_matched_day: u64,
}

/// Which bin a pitch falls in.
///
/// An unestimated pitch gets its own sentinel rather than bin 0: silence and a
/// very low voice would otherwise share a bin, and every unvoiced capture would
/// be compared against the deepest speakers.
pub fn pitch_bin(hz: Option<f32>) -> u16 {
    match hz {
        Some(f) if f > 0.0 && f.is_finite() => (f / PITCH_BIN_HZ) as u16,
        _ => PITCH_BIN_UNKNOWN,
    }
}

/// Bins a query should search — its own and its neighbours.
///
/// ±1 bin is ±100Hz at the default width: enough for intra-speaker variation
/// while skipping most of the store.
pub fn candidate_bins(query: u16) -> Vec<u16> {
    if query == PITCH_BIN_UNKNOWN {
        // An unpitched query cannot narrow anything, so it must search
        // everything rather than only other unpitched entries — a whisper with
        // no detectable fundamental should still match a normal recording.
        return Vec::new();
    }
    let mut v = vec![query];
    if query > 0 { v.push(query - 1); }
    v.push(query + 1);
    v.sort_unstable();
    v
}

/// Does this entry need checking for a query in `bins`?
///
/// An empty bin list means "search everything".
pub fn in_candidates(entry: &Entry, bins: &[u16]) -> bool {
    bins.is_empty() || bins.contains(&entry.pitch_bin)
        // An unpitched ENTRY is always a candidate: it was learned from audio
        // whose fundamental could not be estimated, not from a speaker in some
        // distant register, so excluding it loses a real fingerprint.
        || entry.pitch_bin == PITCH_BIN_UNKNOWN
}

/// The learned store's bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct Store {
    entries: Vec<Entry>,
}

impl Store {
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn entries(&self) -> &[Entry] { &self.entries }

    fn worst(&self, pred: impl Fn(&Entry) -> bool) -> Option<usize> {
        // Least-matched first, oldest as the tie-break: age alone would evict a
        // rarely-used but reliable fingerprint for a recent bad capture, and
        // matches alone would keep a lucky early hit forever.
        self.entries.iter().enumerate()
            .filter(|(_, e)| pred(e))
            .min_by_key(|(_, e)| (e.match_count, e.created_day))
            .map(|(i, _)| i)
    }

    /// Learn a fingerprint, evicting as the caps require.
    pub fn add(&mut self, e: Entry) {
        let same_style = self.entries.iter()
            .filter(|x| x.label == e.label && x.style == e.style).count();
        if same_style >= MAX_PER_STYLE {
            let label = e.label.clone();
            let style = e.style;
            if let Some(i) = self.worst(|x| x.label == label && x.style == style) {
                self.entries.remove(i);
            }
        } else {
            let same_label = self.entries.iter().filter(|x| x.label == e.label).count();
            if same_label >= MAX_PER_LABEL {
                let label = e.label.clone();
                if let Some(i) = self.worst(|x| x.label == label) { self.entries.remove(i); }
            }
        }
        self.entries.push(e);
        while self.entries.len() > TOTAL_CAP {
            if let Some(i) = self.worst(|_| true) { self.entries.remove(i); } else { break; }
        }
    }

    /// Record a successful match.
    pub fn note_match(&mut self, label: &str, today: u64) {
        for e in self.entries.iter_mut().filter(|e| e.label == label) {
            e.match_count += 1;
            e.last_matched_day = today;
        }
    }

    /// Drop fingerprints unmatched for `STALE_DAYS`.
    ///
    /// Measured from the LAST MATCH rather than creation: a fingerprint used
    /// every day is not stale however old it is, and expiring by age would make
    /// the store forget the phrases you use most.
    pub fn prune_stale(&mut self, today: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            let last = e.last_matched_day.max(e.created_day);
            today.saturating_sub(last) < STALE_DAYS
        });
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, style: Style, matches: u32, day: u64) -> Entry {
        Entry { label: label.into(), style, pitch_bin: 3,
                match_count: matches, created_day: day, last_matched_day: day }
    }

    #[test]
    fn one_style_cannot_fill_every_slot_for_a_phrase() {
        // The reason a per-style cap sits inside the per-label one: a user who
        // always speaks normally would otherwise fill all six and never build a
        // whispered fingerprint, so the phrase they say quietly at night never
        // matches.
        let mut s = Store::default();
        for d in 0..6 { s.add(entry("reload", Style::Normal, 0, d)); }
        let normals = s.entries().iter()
            .filter(|e| e.label == "reload" && e.style == Style::Normal).count();
        assert_eq!(normals, MAX_PER_STYLE, "capped per style, not per label");
    }

    #[test]
    fn several_styles_share_the_per_label_budget() {
        let mut s = Store::default();
        for st in [Style::Normal, Style::Whispered, Style::Sleepy] {
            s.add(entry("reload", st, 0, 1));
            s.add(entry("reload", st, 0, 2));
        }
        assert_eq!(s.len(), MAX_PER_LABEL, "six across three styles");
    }

    #[test]
    fn eviction_drops_the_least_matched_first() {
        // "So unused styles get pruned naturally."
        let mut s = Store::default();
        s.add(entry("reload", Style::Normal, 50, 1));
        s.add(entry("reload", Style::Normal, 0, 2));
        s.add(entry("reload", Style::Normal, 0, 3));
        let kept: Vec<u32> = s.entries().iter().map(|e| e.match_count).collect();
        assert!(kept.contains(&50), "the proven one survived: {kept:?}");
    }

    #[test]
    fn age_only_breaks_a_tie_rather_than_deciding() {
        // Ranking by age alone would evict a rarely-used but reliable
        // fingerprint for a recent bad capture.
        let mut s = Store::default();
        s.add(entry("reload", Style::Normal, 3, 1));    // old but used
        s.add(entry("reload", Style::Normal, 0, 90));   // new and unproven
        s.add(entry("reload", Style::Normal, 0, 91));
        assert!(s.entries().iter().any(|e| e.match_count == 3), "the old proven one stayed");
    }

    #[test]
    fn the_total_cap_holds_across_many_labels() {
        let mut s = Store::default();
        for i in 0..(TOTAL_CAP + 50) {
            s.add(entry(&format!("phrase{i}"), Style::Normal, 0, i as u64));
        }
        assert_eq!(s.len(), TOTAL_CAP);
    }

    #[test]
    fn staleness_is_measured_from_the_last_match_not_creation() {
        // A fingerprint used every day is not stale however old it is; expiring
        // by age would make the store forget the phrases you use most.
        let mut s = Store::default();
        s.add(entry("reload", Style::Normal, 10, 0));
        s.note_match("reload", 100);
        assert_eq!(s.prune_stale(101), 0, "used yesterday");
        assert_eq!(s.prune_stale(100 + STALE_DAYS), 1, "and gone a week later");
    }

    #[test]
    fn a_never_matched_fingerprint_expires_from_its_creation() {
        // Otherwise a bad capture with no matches lives forever at
        // last_matched_day = 0.
        let mut s = Store::default();
        s.add(entry("typo", Style::Noisy, 0, 10));
        assert_eq!(s.prune_stale(11), 0);
        assert_eq!(s.prune_stale(10 + STALE_DAYS), 1);
    }

    #[test]
    fn a_backwards_clock_does_not_wipe_the_store() {
        // Saturating: a corrected clock reading earlier than a creation day
        // would otherwise underflow into an enormous age.
        let mut s = Store::default();
        s.add(entry("reload", Style::Normal, 1, 100));
        assert_eq!(s.prune_stale(50), 0);
    }

    #[test]
    fn pitch_bins_group_nearby_voices_and_separate_distant_ones() {
        // 50Hz bins: "most adult speakers have a fundamental in the 80-300Hz
        // range... a typical user touches 1-3 bins".
        assert_eq!(pitch_bin(Some(120.0)), pitch_bin(Some(140.0)), "same bin");
        assert_ne!(pitch_bin(Some(90.0)), pitch_bin(Some(260.0)));
    }

    #[test]
    fn a_query_searches_its_own_bin_and_both_neighbours() {
        // ±1 bin is ±100Hz — "wide enough to absorb intra-speaker variation
        // (typically ±30Hz) while still skipping ~80% of cross-speaker
        // fingerprints".
        assert_eq!(candidate_bins(3), vec![2, 3, 4]);
        assert_eq!(candidate_bins(0), vec![0, 1], "no bin below zero");
    }

    #[test]
    fn an_unpitched_query_searches_everything_rather_than_only_other_unpitched() {
        // A whisper with no detectable fundamental should still match a normal
        // recording of the same phrase.
        assert!(candidate_bins(PITCH_BIN_UNKNOWN).is_empty());
        let e = entry("reload", Style::Normal, 0, 0);
        assert!(in_candidates(&e, &candidate_bins(PITCH_BIN_UNKNOWN)));
    }

    #[test]
    fn an_unpitched_entry_is_always_a_candidate() {
        // It was learned from audio whose fundamental could not be estimated,
        // not from a speaker in a distant register — excluding it loses a real
        // fingerprint.
        let mut e = entry("reload", Style::Whispered, 0, 0);
        e.pitch_bin = PITCH_BIN_UNKNOWN;
        assert!(in_candidates(&e, &candidate_bins(3)));
    }

    #[test]
    fn silence_does_not_share_a_bin_with_a_deep_voice() {
        // An unestimated pitch gets a sentinel rather than bin 0, or every
        // unvoiced capture is compared against the deepest speakers.
        assert_eq!(pitch_bin(None), PITCH_BIN_UNKNOWN);
        assert_eq!(pitch_bin(Some(0.0)), PITCH_BIN_UNKNOWN);
        assert_eq!(pitch_bin(Some(f32::NAN)), PITCH_BIN_UNKNOWN);
        assert_ne!(pitch_bin(Some(40.0)), PITCH_BIN_UNKNOWN);
    }
}
