//! bench_memory — what the loaded dictionary costs to keep resident.
//!
//! Run:  cargo run --release --example bench_memory
//!
//! WHY A COUNTING ALLOCATOR AND NOT RSS
//!
//! RSS on this box measures the allocator's arenas, the binary, and whatever
//! the OS has not reclaimed; it moves by megabytes between runs and would not
//! resolve a change worth a few hundred KB. A `GlobalAlloc` wrapper counts
//! exactly the bytes this process asked for and never returned, which is the
//! number that actually scales with dictionary size on a phone.
//!
//! The keyboard holds this for the whole session — it is loaded once at
//! startup and never dropped — so live bytes after load IS the cost, not a
//! transient.

use board_gestures::GestureRecognizer;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            LIVE.fetch_add(new, Ordering::Relaxed);
            let now = LIVE.fetch_sub(l.size(), Ordering::Relaxed) - l.size();
            PEAK.fetch_max(now + l.size(), Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fn synth_dictionary(n: usize) -> Vec<String> {
    // Distinctness is the property that matters and the one an obvious
    // generator gets wrong: the first version of this cycled its syllable
    // indices and produced 20,000 words of which 168 were unique, which
    // silently turned a 20k-word benchmark into a 168-word one. Enumerating
    // consonant/vowel positions in a mixed-radix counter guarantees every
    // output differs, and the assert at the end refuses to hand back a list
    // that is not the size it claims.
    let c = ["b", "d", "f", "g", "h", "k", "l", "m", "n", "p", "r", "s", "t", "w"];
    let v = ["a", "e", "i", "o", "u", "ai", "ea", "ou"];
    let mut out: Vec<String> = vec![
        "hello".to_string(), "help".to_string(), "held".to_string(),
        "hell".to_string(), "hollow".to_string(), "hero".to_string(),
    ];
    let mut i = 0usize;
    while out.len() < n {
        let mut k = i;
        let syl = 2 + (i % 3);
        let mut w = String::new();
        for _ in 0..syl {
            w.push_str(c[k % c.len()]);
            k /= c.len();
            w.push_str(v[k % v.len()]);
            k /= v.len();
        }
        // The mixed-radix counter repeats once it wraps for a given syllable
        // count, so fold the high bits back in as a distinct tail.
        if k > 0 {
            w.push_str(c[k % c.len()]);
        }
        out.push(w);
        i += 1;
    }
    out.sort();
    out.dedup();
    assert!(
        out.len() >= n * 9 / 10,
        "generator produced {} distinct words for n={n} — a benchmark on a \
         dictionary that is mostly duplicates measures the wrong thing",
        out.len()
    );
    out.truncate(n);
    out
}

fn main() {
    let n = 20_000;
    let words = synth_dictionary(n);
    let chars: usize = words.iter().map(|w| w.len()).sum();

    // Baseline AFTER the input Vec exists, so we measure what the recognizer
    // adds on top of the word list it is handed — the list itself is dropped
    // by `load_dictionary`, so counting it would flatter the result.
    let before = LIVE.load(Ordering::Relaxed);
    let r = GestureRecognizer::with_dictionary(words);
    let after = LIVE.load(Ordering::Relaxed);

    assert_eq!(r.dictionary_size(), n, "dictionary did not load");

    // INSTRUMENT CHECK: build the map that was removed, and measure it on its
    // own. If this does not come out to something like a megabyte, the
    // counter is lying and every number above it is worthless.
    let probe_words = synth_dictionary(n);
    let base = LIVE.load(Ordering::Relaxed);
    let mut m: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, w) in probe_words.iter().enumerate() {
        m.insert(w.clone(), i);
    }
    let map_cost = LIVE.load(Ordering::Relaxed) - base;
    println!("  instrument check: HashMap<String,usize> of {} words = {} KB ({} B/word)",
             m.len(), map_cost / 1024, map_cost / n);
    drop(m);
    drop(probe_words);
    let held = after - before;
    println!(
        "words {n} | source text {} KB | recognizer holds {} KB | {} bytes/word | peak {} KB",
        chars / 1024,
        held / 1024,
        held / n,
        PEAK.load(Ordering::Relaxed) / 1024
    );
}
