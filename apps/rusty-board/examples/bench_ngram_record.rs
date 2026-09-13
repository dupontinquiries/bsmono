//! bench_ngram_record — what committing one word costs once the tables are full.
//!
//! Run:
//!   cargo run --release --no-default-features --features input-modes \
//!             --example bench_ngram_record
//!
//! `NgramModel::record` runs on every committed word, on the UI thread. Once
//! either table is over its cap it calls `evict_lowest`, which scans the whole
//! table. MAX_BIGRAMS is 5,000 and MAX_TRIGRAMS is 10,000, so the steady state
//! is the interesting case and the empty one tells you nothing — this fills
//! well past both caps before timing anything.
//!
//! Asserts the tables really are at their caps before measuring, because a
//! benchmark that silently measured an under-full table would report the fast
//! path and look fine.

use rusty_board::gestures::ngram::NgramModel;
use std::time::Instant;

fn main() {
    let m = NgramModel::empty();

    // Fill past both caps. Distinct contexts so the tables actually fill
    // rather than re-bumping a handful of keys.
    let fill = 40_000usize;
    for i in 0..fill {
        m.record(
            Some(&format!("p{}", i % 900)),
            Some(&format!("q{}", i % 700)),
            &format!("w{}", i % 1300),
        );
    }
    let (bi, tri) = (m.bigram_count(), m.trigram_count());
    assert!(
        bi >= 4_000 && tri >= 8_000,
        "tables not near their caps ({bi} bigrams, {tri} trigrams) — this \
         would be timing the pre-eviction fast path, not the steady state"
    );

    let iters = 2_000;
    let t = Instant::now();
    for i in 0..iters {
        m.record(
            Some(&format!("z{}", i % 900)),
            Some(&format!("y{}", i % 700)),
            &format!("x{}", i % 1300),
        );
    }
    let el = t.elapsed();
    println!(
        "steady state: {bi} bigrams / {tri} trigrams | {:.1} us per record",
        el.as_secs_f64() * 1e6 / iters as f64
    );
}
