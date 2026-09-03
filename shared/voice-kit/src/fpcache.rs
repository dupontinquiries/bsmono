//! fpcache — skip whisper on repeated utterances. A fingerprint→transcription cache keyed on the
//! opaque 64-bit acoustic fingerprint (`audio_kit::fingerprint::fingerprint`, injected so voice-kit
//! keeps no signal dependency). Fuzzy: a new utterance whose fingerprint is within `tol` Hamming of
//! a cached one reuses that text — the transcription-time saving for duplicate words. rusty-board
//! (keyboard), voiceterm, and logscope voice all get it by wrapping their whisper [`Transcriber`].

use crate::pipeline::Transcriber;

/// A small, fuzzy fingerprint→text cache.
///
/// Entries carry the utterance LENGTH (in samples) as well, because the fingerprint is
/// time-normalised: a one-second "hi" and a three-second "hi there" can land within a few bits of
/// each other, and for a keyboard a confident wrong word is worse than a slow right one. The
/// `_len` variants gate a match on the lengths agreeing within `len_tol` (a fraction, default
/// 0.2); the plain `lookup`/`insert` keep the original behaviour (length unchecked) for callers
/// that never had one.
pub struct FingerprintCache {
    entries: Vec<(u64, u32, String)>,
    cap: usize,
    tol: u32,
    /// Fractional length tolerance for `lookup_len`; 0.0 = exact, 1.0 = anything.
    pub len_tol: f32,
}

impl FingerprintCache {
    pub fn new(cap: usize, tol: u32) -> Self {
        FingerprintCache { entries: Vec::new(), cap: cap.max(1), tol, len_tol: 0.2 }
    }

    fn len_ok(&self, a: u32, b: u32) -> bool {
        if a == 0 || b == 0 {
            return true; // an entry or query without a length is not length-checked
        }
        let (lo, hi) = (a.min(b) as f32, a.max(b) as f32);
        (hi - lo) / hi <= self.len_tol
    }

    /// `lookup`, additionally requiring the stored utterance length to agree with `len` (samples)
    /// within `len_tol`. The nearest by Hamming distance among those wins.
    pub fn lookup_len(&self, fp: u64, len: u32) -> Option<&str> {
        if fp == 0 {
            return None;
        }
        self.entries
            .iter()
            .filter(|(_, l, _)| self.len_ok(*l, len))
            .map(|(f, _, t)| ((f ^ fp).count_ones(), t))
            .filter(|(d, _)| *d <= self.tol)
            .min_by_key(|(d, _)| *d)
            .map(|(_, t)| t.as_str())
    }

    /// `insert` with the utterance length recorded, so `lookup_len` can gate on it.
    pub fn insert_len(&mut self, fp: u64, len: u32, text: String) {
        if fp == 0 || text.is_empty() {
            return;
        }
        if let Some(pos) = self
            .entries
            .iter()
            .position(|(f, l, _)| (f ^ fp).count_ones() <= self.tol && self.len_ok(*l, len))
        {
            self.entries[pos] = (fp, len, text);
            return;
        }
        self.entries.push((fp, len, text));
        if self.entries.len() > self.cap {
            self.entries.remove(0);
        }
    }

    /// Every entry, oldest first — for persisting the corpus.
    pub fn entries(&self) -> impl Iterator<Item = (u64, u32, &str)> {
        self.entries.iter().map(|(f, l, t)| (*f, *l, t.as_str()))
    }

    /// The nearest cached transcription within `tol` bits of `fp` (None if `fp` is the degenerate 0
    /// or nothing is close enough).
    pub fn lookup(&self, fp: u64) -> Option<&str> {
        self.lookup_len(fp, 0)
    }

    /// Remember `fp → text` (bounded; oldest evicted). A near-duplicate refreshes the existing entry
    /// rather than growing the table; the degenerate 0 and empty text are ignored.
    pub fn insert(&mut self, fp: u64, text: String) {
        self.insert_len(fp, 0, text)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Wraps a whisper [`Transcriber`], consulting a [`FingerprintCache`] first. On a hit (a duplicate
/// word) it returns the cached text **without running whisper**; on a miss it runs whisper once and
/// caches the result. Because it *is* a `Transcriber`, an app opts in with a one-line wrap — no
/// pipeline change. The fingerprint fn is injected (pass `audio_kit::fingerprint::fingerprint`).
pub struct CachedTranscriber<T: Transcriber, F: FnMut(&[f32]) -> u64> {
    pub inner: T,
    pub fingerprint: F,
    pub cache: FingerprintCache,
}

impl<T: Transcriber, F: FnMut(&[f32]) -> u64> CachedTranscriber<T, F> {
    pub fn new(inner: T, fingerprint: F, cap: usize, tol: u32) -> Self {
        CachedTranscriber { inner, fingerprint, cache: FingerprintCache::new(cap, tol) }
    }
}

impl<T: Transcriber, F: FnMut(&[f32]) -> u64> Transcriber for CachedTranscriber<T, F> {
    fn transcribe(&mut self, audio: &[f32]) -> Option<String> {
        let fp = (self.fingerprint)(audio);
        if let Some(hit) = self.cache.lookup(fp) {
            return Some(hit.to_string()); // duplicate → skip whisper
        }
        let text = self.inner.transcribe(audio)?;
        self.cache.insert(fp, text.clone());
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Transcriber;

    #[test]
    fn a_length_mismatch_is_not_a_hit_even_when_the_bits_agree() {
        let mut c = FingerprintCache::new(8, 4);
        c.insert_len(0b1111, 16_000, "hi".into());
        assert_eq!(c.lookup_len(0b1111, 16_000), Some("hi"), "same length: hit");
        assert_eq!(c.lookup_len(0b1111, 18_000), Some("hi"), "12% longer: within len_tol");
        assert_eq!(c.lookup_len(0b1111, 48_000), None, "three times longer: the same bits are a different utterance");
        assert_eq!(c.lookup(0b1111), Some("hi"), "the length-blind lookup still matches");
        // a different-length near-duplicate is a NEW entry, not a refresh of the old one
        c.insert_len(0b1110, 48_000, "hi there".into());
        assert_eq!(c.len(), 2);
        assert_eq!(c.lookup_len(0b1111, 48_000), Some("hi there"));
    }

    #[test]
    fn cache_exact_fuzzy_and_capacity() {
        let mut c = FingerprintCache::new(2, 4);
        c.insert(0b1111, "open mail".into());
        assert_eq!(c.lookup(0b1111), Some("open mail")); // exact
        assert_eq!(c.lookup(0b1101), Some("open mail")); // 1 bit off → within tol
        assert_eq!(c.lookup(0xFFF0), None); // hamming 16 → far miss
        assert_eq!(c.lookup(0), None); // degenerate never matches
        // capacity: two more distinct fps evict the oldest
        c.insert(0xFF00, "b".into());
        c.insert(0x00FF, "c".into());
        assert!(c.len() <= 2);
    }

    /// A whisper stub that counts how many times it actually ran.
    struct CountingWhisper {
        calls: std::rc::Rc<std::cell::Cell<u32>>,
        text: String,
    }
    impl Transcriber for CountingWhisper {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            self.calls.set(self.calls.get() + 1);
            Some(self.text.clone())
        }
    }

    #[test]
    fn duplicate_word_skips_whisper() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        // fingerprint fn: deterministic from the buffer's first sample (test controls the audio)
        let fp = |a: &[f32]| a.first().map(|x| (x * 1000.0) as i64 as u64).unwrap_or(0);
        let mut t = CachedTranscriber::new(
            CountingWhisper { calls: calls.clone(), text: "hello".into() },
            fp,
            16,
            2,
        );
        let word = vec![0.123_f32; 400]; // fp != 0

        assert_eq!(t.transcribe(&word).as_deref(), Some("hello"));
        assert_eq!(calls.get(), 1, "first time: whisper runs");
        // same word again → cache hit, whisper NOT run again
        assert_eq!(t.transcribe(&word).as_deref(), Some("hello"));
        assert_eq!(calls.get(), 1, "duplicate word: whisper skipped");

        // a genuinely different utterance still runs whisper
        let other = vec![0.777_f32; 400];
        assert_eq!(t.transcribe(&other).as_deref(), Some("hello"));
        assert_eq!(calls.get(), 2, "new word: whisper runs");
    }
}
