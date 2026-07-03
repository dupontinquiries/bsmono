//! Synchronized shared codebook — the transport/sync core for an index-based
//! voice codec. **Research scaffold**, not a working low-bitrate codec.
//!
//! This implements the genuinely-tricky, genuinely-reusable part of the
//! "build a voice corpus, send indexes into a shared map, blend the top-N"
//! idea: keeping the two endpoints' maps *identical* so an index means the
//! same thing on both sides. The parts it deliberately does NOT do — turning
//! audio into compact feature vectors, and turning feature vectors back into
//! speech — are the pluggable model hooks (see "Model hooks" below).
//!
//! ## Why the literal design needs correcting (and what this fixes)
//!
//! 1. **The map must be shared, and can't be built independently.** Each side
//!    of a call hears the *other* speaker, so two independently-built corpora
//!    would diverge — index 47 would mean different audio on each end. Here the
//!    map grows *deterministically from the wire stream*: the ENCODER is
//!    authoritative (it decides hit vs. miss); the DECODER just obeys the
//!    token. A miss carries its feature vector once (`NewEntry`), which BOTH
//!    sides append at the same next index; thereafter it's referenced by
//!    `Index`. The decoder never re-runs the nearest-neighbour search, so
//!    floating-point nondeterminism can't desync the maps.
//!
//! 2. **Match on FEATURES, not exact clips.** Raw speech waveforms essentially
//!    never repeat (pitch, timing, phase, noise all differ), so exact-clip
//!    fingerprinting hits ~never. This matches in feature space with a
//!    tolerance. The feature extractor is the model hook — and it must be a
//!    *synthesis* feature (LPC / a neural codec latent), NOT MFCC: MFCC is a
//!    recognition feature, it throws away pitch/phase/excitation and can't be
//!    inverted back to good audio.
//!
//! 3. **"Predict the next chunk" isn't precognition.** You can't transmit audio
//!    that hasn't been spoken. The bandwidth win comes from *coding the current
//!    chunk compactly* (a 2-byte index instead of a 40–80-byte Opus frame); the
//!    delay win comes from smaller packets + concealment, not from predicting
//!    the future.
//!
//! 4. **`blend_topn` is a quality knob, not a bandwidth knob.** Averaging the N
//!    nearest entries (similarity-weighted) gives smoother reconstruction than
//!    nearest-only, but you still transmit one index/query per frame — it does
//!    not by itself reduce bandwidth.
//!
//! ## When this actually saves bandwidth
//!
//! A hit saves (Opus frame − index) bytes; a miss COSTS a full feature vector
//! plus framing. Net win needs the hit rate high enough that
//! `hits·savings > misses·cost`. A per-conversation corpus that's thrown away
//! pays a cold-start penalty every call and may not net out. The dramatic wins
//! (Lyra/EnCodec-style, ~3 kbps) come from a *pre-trained / persisted* codebook
//! shipped with the app — which is the "supply the model" path. This structure
//! supports both: grow it online, or load a fixed codebook at start.
//!
//! ## Model hooks (NOT implemented here — they need your model)
//!   • `feature(frame: &[i16]) -> Vec<f32>`  — synthesis feature extraction
//!   • `synthesize(feat: &[f32]) -> Vec<i16>` — vocoder (feature → audio)
//! Wire those two in and `SharedCodebook` becomes the codebook layer of a real
//! codec. Until then this is intentionally NOT connected to the live voice path
//! (it can't produce audio yet, so connecting it would break calls).

/// Squared Euclidean distance between two equal-length feature vectors.
#[inline]
fn dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| { let d = x - y; d * d }).sum()
}

/// A wire token: either a reference to an existing shared entry (cheap — the
/// bandwidth win), or a brand-new entry carrying its feature vector (the
/// expensive case; both endpoints append it so the maps stay identical).
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// Reference to entry `idx` already present in both maps.
    Index(u32),
    /// A new feature vector. Both sides append it (subject to the cap).
    NewEntry(Vec<f32>),
}

impl Token {
    /// Rough wire cost in bytes, for bandwidth accounting in tests/tuning:
    /// an index is ~3 bytes; a new entry is the vector (4 bytes/dim) + framing.
    pub fn wire_bytes(&self) -> usize {
        match self {
            Token::Index(_) => 3,
            Token::NewEntry(v) => 2 + v.len() * 4,
        }
    }
}

/// A codebook held identically on both endpoints. Grows append-only from the
/// token stream; indexes never shift (so a cap is a no-op, not a reshuffle).
pub struct SharedCodebook {
    dim: usize,
    entries: Vec<Vec<f32>>,
    match_tol2: f32,    // squared-distance tolerance for an Index hit
    max_entries: usize, // bounded corpus; append-only, no eviction
}

impl SharedCodebook {
    /// `dim` = feature length; `match_tol` = max Euclidean distance for a hit;
    /// `max_entries` caps the corpus (it's thrown out at conversation end).
    pub fn new(dim: usize, match_tol: f32, max_entries: usize) -> Self {
        Self {
            dim,
            entries: Vec::new(),
            match_tol2: (match_tol.max(0.0)).powi(2),
            max_entries: max_entries.max(1),
        }
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn is_full(&self) -> bool { self.entries.len() >= self.max_entries }
    pub fn dim(&self) -> usize { self.dim }
    /// Read-only view (for inspection, persistence, or a pre-trained load).
    pub fn entries(&self) -> &[Vec<f32>] { &self.entries }

    /// Nearest entry to `feat`: `(index, squared_distance)`, or None if empty.
    fn nearest(&self, feat: &[f32]) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            let d = dist2(feat, e);
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i as u32, d));
            }
        }
        best
    }

    /// Append-only insert. No-op once full, so indexes are stable forever and
    /// both endpoints reach "full" at the same token → maps stay identical.
    fn push(&mut self, feat: Vec<f32>) {
        if self.entries.len() < self.max_entries {
            self.entries.push(feat);
        }
    }

    /// ENCODE one frame's features → a token, growing the map exactly as the
    /// decoder will. Hit (within tolerance) → `Index`; miss → `NewEntry`.
    pub fn encode(&mut self, feat: &[f32]) -> Token {
        debug_assert_eq!(feat.len(), self.dim, "feature dim mismatch");
        if let Some((idx, d2)) = self.nearest(feat) {
            if d2 <= self.match_tol2 {
                return Token::Index(idx);
            }
        }
        self.push(feat.to_vec());
        Token::NewEntry(feat.to_vec())
    }

    /// DECODE a token → the matched feature vector, growing the map identically
    /// to the encoder. (Feature → audio is the separate vocoder hook.)
    pub fn decode(&mut self, tok: &Token) -> Vec<f32> {
        match tok {
            Token::Index(i) => self
                .entries
                .get(*i as usize)
                .cloned()
                .unwrap_or_else(|| vec![0.0; self.dim]), // desync guard
            Token::NewEntry(f) => {
                self.push(f.clone());
                f.clone()
            }
        }
    }

    /// Interpolative reconstruction: similarity-weighted average of the N
    /// nearest entries. A smoothing/quality step (NOT a bandwidth step) — a
    /// decoder can use this instead of the nearest-only entry from `decode`.
    pub fn blend_topn(&self, feat: &[f32], n: usize) -> Vec<f32> {
        if self.entries.is_empty() || n == 0 {
            return vec![0.0; self.dim];
        }
        let mut scored: Vec<(f32, usize)> =
            self.entries.iter().enumerate().map(|(i, e)| (dist2(feat, e), i)).collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let k = n.min(scored.len());

        let mut out = vec![0.0f32; self.dim];
        let mut wsum = 0.0f32;
        for &(d2, idx) in &scored[..k] {
            let w = 1.0 / (d2 + 1.0e-6); // closer ⇒ heavier
            wsum += w;
            for (o, &e) in out.iter_mut().zip(self.entries[idx].iter()) {
                *o += w * e;
            }
        }
        if wsum > 0.0 {
            for o in out.iter_mut() { *o /= wsum; }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a feature sequence through encoder→token→decoder and assert the two
    /// maps stay bit-identical at every step. Returns the tokens produced.
    fn lockstep(seq: &[Vec<f32>], dim: usize, tol: f32, cap: usize) -> Vec<Token> {
        let mut enc = SharedCodebook::new(dim, tol, cap);
        let mut dec = SharedCodebook::new(dim, tol, cap);
        let mut tokens = Vec::new();
        for feat in seq {
            let tok = enc.encode(feat);
            let _recovered = dec.decode(&tok);
            assert_eq!(enc.entries(), dec.entries(), "maps desynced after {tok:?}");
            tokens.push(tok);
        }
        tokens
    }

    #[test]
    fn lockstep_keeps_identical_maps() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let c = vec![0.0, 0.0, 1.0];
        // mix of novel and repeated frames
        let seq = vec![a.clone(), b.clone(), a.clone(), c.clone(), b.clone(), a.clone()];
        let toks = lockstep(&seq, 3, 0.1, 64);
        // novel a,b,c → NewEntry; repeats → Index
        assert!(matches!(toks[0], Token::NewEntry(_)));
        assert!(matches!(toks[1], Token::NewEntry(_)));
        assert!(matches!(toks[2], Token::Index(0))); // a again
        assert!(matches!(toks[3], Token::NewEntry(_)));
        assert!(matches!(toks[4], Token::Index(1))); // b again
        assert!(matches!(toks[5], Token::Index(0))); // a again
    }

    #[test]
    fn repeated_feature_becomes_index_and_decodes_to_it() {
        let mut enc = SharedCodebook::new(3, 0.1, 64);
        let mut dec = SharedCodebook::new(3, 0.1, 64);
        let feat = vec![0.5, -0.5, 0.25];

        let t1 = enc.encode(&feat);
        assert_eq!(t1, Token::NewEntry(feat.clone()));
        let r1 = dec.decode(&t1);
        assert_eq!(r1, feat);

        // a slightly perturbed copy within tolerance → Index, recovers the entry
        let near = vec![0.51, -0.49, 0.25];
        let t2 = enc.encode(&near);
        assert_eq!(t2, Token::Index(0));
        let r2 = dec.decode(&t2);
        assert_eq!(r2, feat, "Index decodes to the stored entry");
    }

    #[test]
    fn novel_features_are_all_new_entries() {
        let seq: Vec<Vec<f32>> =
            (0..5).map(|i| vec![i as f32, -(i as f32), 0.0]).collect();
        let toks = lockstep(&seq, 3, 0.1, 64);
        assert!(toks.iter().all(|t| matches!(t, Token::NewEntry(_))));
    }

    #[test]
    fn cap_stops_growth_and_stays_synced() {
        // cap of 2 entries; feed 4 distinct features then revisit them
        let f = |k: f32| vec![k, 0.0];
        let seq = vec![f(0.0), f(10.0), f(20.0), f(30.0), f(0.0), f(20.0)];
        let mut enc = SharedCodebook::new(2, 0.1, 2);
        let mut dec = SharedCodebook::new(2, 0.1, 2);
        for feat in &seq {
            let tok = enc.encode(feat);
            dec.decode(&tok);
            assert_eq!(enc.entries(), dec.entries(), "desync at cap");
        }
        assert_eq!(enc.len(), 2, "append-only stops at the cap");
        assert!(enc.is_full());
        // f(0.0) is still entry 0 (indexes never shifted) → re-hits as Index(0)
        assert_eq!(enc.encode(&f(0.0)), Token::Index(0));
        // f(30.0) was never cached (cap hit) → inline NewEntry, no growth
        assert_eq!(enc.encode(&f(30.0)), Token::NewEntry(f(30.0)));
        assert_eq!(enc.len(), 2);
    }

    #[test]
    fn blend_weights_toward_nearest() {
        let mut cb = SharedCodebook::new(1, 0.0, 16);
        for v in [0.0f32, 1.0, 10.0] {
            cb.push(vec![v]);
        }
        // query near 0.0: blend of all three should land much closer to 0 than
        // a plain mean (which would be ~3.67), because weights favour the
        // nearest entries.
        let out = cb.blend_topn(&[0.1], 3);
        assert!(out[0] < 1.0, "blend should be pulled toward nearest: {}", out[0]);
        // top-1 blend is just the nearest entry
        let top1 = cb.blend_topn(&[0.1], 1);
        assert!((top1[0] - 0.0).abs() < 1.0e-6);
    }

    #[test]
    fn bandwidth_accounting_sanity() {
        // A repeated-feature stream should cost far less than all-inline once
        // the corpus warms up. (Illustrative — real gains depend on the feature
        // extractor's hit rate on actual speech.)
        let f = |k: i32| vec![k as f32, 0.0, 0.0, 0.0];
        // 3 distinct phonetic "units" repeated many times
        let mut seq = Vec::new();
        for _ in 0..30 { for u in 0..3 { seq.push(f(u)); } }
        let mut enc = SharedCodebook::new(4, 0.1, 64);
        let mut coded = 0usize;
        let mut inline = 0usize;
        for feat in &seq {
            let tok = enc.encode(feat);
            coded += tok.wire_bytes();
            inline += 2 + feat.len() * 4; // cost if we sent the vector every time
        }
        assert!(coded * 4 < inline, "codebook should beat all-inline: {coded} vs {inline}");
    }
}
