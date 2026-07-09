//! fpcache — skip whisper on repeated utterances. A fingerprint→transcription cache keyed on the
//! opaque 64-bit acoustic fingerprint (`audio_kit::fingerprint::fingerprint`, injected so voice-kit
//! keeps no signal dependency). Fuzzy: a new utterance whose fingerprint is within `tol` Hamming of
//! a cached one reuses that text — the transcription-time saving for duplicate words. rusty-board
//! (keyboard), voiceterm, and logscope voice all get it by wrapping their whisper [`Transcriber`].

use crate::pipeline::Transcriber;

/// A small, fuzzy fingerprint→text cache.
pub struct FingerprintCache {
    entries: Vec<(u64, String)>,
    cap: usize,
    tol: u32,
}

impl FingerprintCache {
    pub fn new(cap: usize, tol: u32) -> Self {
        FingerprintCache { entries: Vec::new(), cap: cap.max(1), tol }
    }

    /// The nearest cached transcription within `tol` bits of `fp` (None if `fp` is the degenerate 0
    /// or nothing is close enough).
    pub fn lookup(&self, fp: u64) -> Option<&str> {
        if fp == 0 {
            return None;
        }
        self.entries
            .iter()
            .map(|(f, t)| ((f ^ fp).count_ones(), t))
            .filter(|(d, _)| *d <= self.tol)
            .min_by_key(|(d, _)| *d)
            .map(|(_, t)| t.as_str())
    }

    /// Remember `fp → text` (bounded; oldest evicted). A near-duplicate refreshes the existing entry
    /// rather than growing the table; the degenerate 0 and empty text are ignored.
    pub fn insert(&mut self, fp: u64, text: String) {
        if fp == 0 || text.is_empty() {
            return;
        }
        if let Some(pos) = self.entries.iter().position(|(f, _)| (f ^ fp).count_ones() <= self.tol) {
            self.entries[pos] = (fp, text);
            return;
        }
        self.entries.push((fp, text));
        if self.entries.len() > self.cap {
            self.entries.remove(0);
        }
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
