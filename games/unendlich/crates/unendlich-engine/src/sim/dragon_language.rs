//! `sim::dragon_language` — a tiny symbol grammar dragons use to signal one
//! another (and that pairs with `sound_genome` to give each dragon a voice).
//!
//! A spoken symbol is a `u16`. [`encode`] packs an `intent` (what the dragon
//! means) into the high byte and a *bucketed* `mood` (how it feels, `0..1`) into
//! the low nibble; [`decode`] unpacks them, returning the intent exactly and the
//! mood as its bucket's representative value. Mood is bucketed on purpose —
//! dragons don't transmit infinite emotional precision, so nearby moods speak
//! the same symbol.
//!
//! [`similar`] scores two symbols in `0..1`: half for sharing an intent, half
//! for closeness of mood, so `similar(x, x) == 1` and symbols with different
//! intents always score below a full match. Pure and deterministic.

/// Number of mood buckets (fits in the low nibble of the symbol).
pub const MOOD_BUCKETS: u16 = 16;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Which mood bucket a `0..1` mood falls in (`0 ..= MOOD_BUCKETS-1`).
fn bucket_of(mood: f32) -> u16 {
    let scaled = (clamp01(mood) * MOOD_BUCKETS as f32) as u16;
    if scaled >= MOOD_BUCKETS {
        MOOD_BUCKETS - 1
    } else {
        scaled
    }
}

/// The representative mood at the centre of a bucket.
fn bucket_center(b: u16) -> f32 {
    (b as f32 + 0.5) / MOOD_BUCKETS as f32
}

/// Pack an intent and a mood into a spoken symbol.
pub fn encode(intent: u8, mood: f32) -> u16 {
    ((intent as u16) << 8) | bucket_of(mood)
}

/// Unpack a symbol into `(intent, mood)`. Inverse of [`encode`] up to the mood
/// bucketing (the returned mood is the bucket centre).
pub fn decode(sym: u16) -> (u8, f32) {
    let intent = (sym >> 8) as u8;
    let b = (sym & 0x00FF) % MOOD_BUCKETS;
    (intent, bucket_center(b))
}

/// Similarity of two symbols, `0..1`. Half weight on intent match, half on mood
/// closeness; identical symbols score `1.0`.
pub fn similar(a: u16, b: u16) -> f32 {
    let (ia, ma) = decode(a);
    let (ib, mb) = decode(b);
    let intent_term = if ia == ib { 1.0 } else { 0.0 };
    let mood_term = clamp01(1.0 - (ma - mb).abs());
    0.5 * intent_term + 0.5 * mood_term
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn round_trip_intent_exact_and_mood_same_bucket() {
        let mut seed = 0xD41A_u32;
        for _ in 0..3000 {
            let intent = (lcg(&mut seed) & 0xFF) as u8;
            let mood = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let (di, dm) = decode(encode(intent, mood));
            assert_eq!(di, intent);
            // decoded mood is in the same bucket as the original
            assert_eq!(bucket_of(dm), bucket_of(mood));
        }
    }

    #[test]
    fn similar_to_self_is_one() {
        let mut seed = 0x5E1F_u32;
        for _ in 0..2000 {
            let intent = (lcg(&mut seed) & 0xFF) as u8;
            let mood = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let s = encode(intent, mood);
            assert_eq!(similar(s, s), 1.0);
        }
    }

    #[test]
    fn distinct_intents_are_distinguishable() {
        // same mood, different intent → never a full match
        let a = encode(1, 0.5);
        let b = encode(2, 0.5);
        assert!(similar(a, b) < 1.0);
        assert!((similar(a, b) - 0.5).abs() < 1e-6, "got {}", similar(a, b));
    }

    #[test]
    fn mood_is_bucketed() {
        // two moods inside one 1/16-wide bucket speak the same symbol
        let s1 = encode(7, 0.01);
        let s2 = encode(7, 0.05);
        assert_eq!(s1, s2);
        // moods in different buckets differ
        let s3 = encode(7, 0.95);
        assert_ne!(s1, s3);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB011_u32;
        for _ in 0..4000 {
            let a = (lcg(&mut seed) & 0xFFFF) as u16;
            let b = (lcg(&mut seed) & 0xFFFF) as u16;
            let s = similar(a, b);
            assert!(s >= 0.0 && s <= 1.0, "similar out of range: {}", s);
            let (_, m) = decode(a);
            assert!(m >= 0.0 && m < 1.0, "mood out of range: {}", m);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xDE7E_u32;
        for _ in 0..3000 {
            let intent = (lcg(&mut seed) & 0xFF) as u8;
            let mood = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(encode(intent, mood), encode(intent, mood));
            let a = (lcg(&mut seed) & 0xFFFF) as u16;
            let b = (lcg(&mut seed) & 0xFFFF) as u16;
            assert_eq!(similar(a, b), similar(a, b));
        }
    }
}
