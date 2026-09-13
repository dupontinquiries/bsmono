//! bench_recognize — what one swipe costs, on a dictionary the size a phone
//! actually ships.
//!
//! Run:  cargo run --release --example bench_recognize
//!
//! Deliberately not a criterion harness: the house pattern in
//! `maintenance/perf/` is a self-contained thing that reproduces under plain
//! `-O`, and adding a dev-dependency to a crate that ships on a phone to
//! measure it is a poor trade.
//!
//! WHAT IT MEASURES AND WHY THAT SHAPE
//!
//! `recognize` is called once per completed gesture, on the UI thread,
//! between the finger lifting and the word appearing. So the number that
//! matters is milliseconds per call at a realistic dictionary size, not
//! throughput.
//!
//! The dictionary is generated rather than loaded so the benchmark has no
//! fixture dependency and cannot silently measure an empty file — the
//! CLAUDE.md §5b failure where a fixture-gated test returns early and passes.
//! It asserts a floor on the candidate count for the same reason: a
//! recognizer that matched nothing would look extremely fast.

use board_gestures::{GestureRecognizer, KeyHitbox, SwipePoint};
use std::collections::HashMap;
use std::time::Instant;

/// A QWERTY-shaped position map, unit-key space.
fn qwerty_positions() -> HashMap<char, (f64, f64)> {
    let rows = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
    let mut m = HashMap::new();
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            m.insert(c, (x as f64 + y as f64 * 0.3, y as f64));
        }
    }
    m
}

/// Pronounceable filler so the length buckets fill the way a real dictionary
/// fills them. A dictionary of random noise would make the edit-distance cap
/// reject almost everything and understate the cost.
fn synth_dictionary(n: usize) -> Vec<String> {
    let c = ["b", "d", "f", "g", "h", "k", "l", "m", "n", "p", "r", "s", "t", "w"];
    let v = ["a", "e", "i", "o", "u", "ai", "ea", "ou"];
    let mut out = vec![
        "hello".to_string(), "help".to_string(), "held".to_string(),
        "hell".to_string(), "hollow".to_string(), "hero".to_string(),
    ];
    let mut i = 0usize;
    while out.len() < n {
        let syl = 2 + (i % 3);
        let mut w = String::new();
        for s in 0..syl {
            w.push_str(c[(i + s * 7) % c.len()]);
            w.push_str(v[(i + s * 3) % v.len()]);
        }
        out.push(w);
        i += 1;
    }
    out
}

fn main() {
    let words = 20_000;
    let mut r = GestureRecognizer::with_dictionary(synth_dictionary(words));
    r.set_key_positions(qwerty_positions());

    // A path traced through h-e-l-l-o, the canonical swipe.
    let pos = qwerty_positions();
    let mut path = Vec::new();
    let mut t_ms = 0i64;
    for ch in "hello".chars() {
        let (x, y) = pos[&ch];
        // A few points per key, as a real finger produces.
        for k in 0..6 {
            path.push(SwipePoint { x: x * 60.0 + k as f64, y: y * 60.0 + k as f64 * 0.5, timestamp_ms: t_ms });
            t_ms += 12;
        }
    }
    let boxes: Vec<KeyHitbox> = pos
        .iter()
        .map(|(&ch, &(x, y))| KeyHitbox {
            label: ch.to_string(),
            swipe_target: true,
            x: x * 60.0,
            y: y * 60.0,
            width: 55.0,
            height: 55.0,
        })
        .collect();

    // Warm: first call touches allocator paths the steady state does not.
    let warm = r.recognize(&path, &boxes);
    assert!(
        !warm.is_empty(),
        "recognizer produced no candidates — the benchmark would be timing \
         an early return, not the scoring loop"
    );

    let iters = 200;
    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..iters {
        sink += r.recognize(&path, &boxes).len();
    }
    let el = t.elapsed();

    assert!(sink >= iters, "every iteration must produce candidates");
    println!(
        "dictionary {} words | {} iterations | {:.3} ms/recognize | top={:?}",
        words,
        iters,
        el.as_secs_f64() * 1000.0 / iters as f64,
        warm.first().map(|c| c.word.clone())
    );
}
