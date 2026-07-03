//! # The original kMelodizer string-matching approach
//!
//! This is a Rust port of the algorithm I wrote in school. It's preserved
//! verbatim — drop / swap / replace single-edit moves against a target word
//! set — because the *idea* is worth seeing: how do you nudge an arbitrary
//! word toward an in-vocabulary word with the smallest possible change?
//!
//! It's **not** how kRapper does fit-to-vocab in production. The real version
//! lives in `src/dict.rs` and uses phoneme-space Levenshtein over CMUdict
//! pronunciations. See `docs/learning-example.md` for the side-by-side and the
//! discussion of why string editing is a workaround for sound matching, not an
//! implementation of it.
//!
//! Run me with:
//!
//! ```bash
//! cargo run --example learning_string_matching
//! ```

use std::collections::HashSet;
use std::io::{self, BufRead, Write};

// --- the three single-edit neighbours --------------------------------------
//
// Each function asks: "of all the strings that differ from `w` by exactly one
// edit of this kind, which ones happen to be in the dictionary?" If any, the
// lowest-sorted one wins. That tie-breaker is the original kMelodizer rule.

fn drop_one(dict: &HashSet<String>, w: &str) -> Option<String> {
    // Try removing each character in turn.
    let chars: Vec<char> = w.chars().collect();
    let mut hits: Vec<String> = Vec::new();
    for i in 0..chars.len() {
        let mut c = chars.clone();
        c.remove(i);
        let candidate: String = c.into_iter().collect();
        if dict.contains(&candidate) {
            hits.push(candidate);
        }
    }
    hits.sort();
    hits.into_iter().next()
}

fn swap_adjacent(dict: &HashSet<String>, w: &str) -> Option<String> {
    // Try swapping each adjacent character pair.
    let chars: Vec<char> = w.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let mut hits: Vec<String> = Vec::new();
    for i in 0..chars.len() - 1 {
        let mut c = chars.clone();
        c.swap(i, i + 1);
        let candidate: String = c.into_iter().collect();
        if dict.contains(&candidate) {
            hits.push(candidate);
        }
    }
    hits.sort();
    hits.into_iter().next()
}

fn replace_one(dict: &HashSet<String>, w: &str) -> Option<String> {
    // Try replacing each character with every other a–z letter.
    let bytes = w.as_bytes().to_vec();
    let mut hits: Vec<String> = Vec::new();
    for i in 0..bytes.len() {
        for l in b'a'..=b'z' {
            if l == bytes[i] {
                continue;
            }
            let mut b = bytes.clone();
            b[i] = l;
            if let Ok(s) = String::from_utf8(b) {
                if dict.contains(&s) {
                    hits.push(s);
                }
            }
        }
    }
    hits.sort();
    hits.into_iter().next()
}

/// The original `kMelodizer` fallback chain: try DROP, then SWAP, then REPLACE.
/// First hit wins. Returns the matched word and which edit produced it.
fn fit_to_vocab(dict: &HashSet<String>, w: &str) -> (String, &'static str) {
    let w = w.trim().to_lowercase();
    if w.is_empty() {
        return (String::new(), "EMPTY");
    }
    if dict.contains(&w) {
        return (w, "FOUND");
    }
    if let Some(r) = drop_one(dict, &w) {
        return (r, "DROP");
    }
    if let Some(r) = swap_adjacent(dict, &w) {
        return (r, "SWAP");
    }
    if let Some(r) = replace_one(dict, &w) {
        return (r, "REPLACE");
    }
    (w, "NO MATCH")
}

// --- demo driver -----------------------------------------------------------

const SAMPLE_VOCAB: &[&str] = &[
    "hearse", "curse", "reverse", "worse", "fast", "past", "blur", "slur",
    "heart", "hurt", "love", "loved", "gave", "grave", "fur", "person",
    "around", "runaround", "paranoid", "real", "true", "first", "mind",
    "running", "drinking", "digging", "making", "leave", "woke", "told",
];

fn main() -> io::Result<()> {
    let dict: HashSet<String> = SAMPLE_VOCAB.iter().map(|s| s.to_string()).collect();

    println!("kRapper · learning example · string-edit fit-to-vocab");
    println!("vocab ({} words): {:?}", dict.len(), SAMPLE_VOCAB);
    println!();
    println!("Type a word and press Enter. Empty line quits.");
    println!("Try: 'horse' (→ swap), 'pas' (→ drop), 'persn' (→ replace),");
    println!("     'through' (→ no match — it's not character-similar to anything here)");
    println!();

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let w = line.trim();
        if w.is_empty() {
            break;
        }
        let (matched, how) = fit_to_vocab(&dict, w);
        println!("    {w:<15} → {matched:<15}  :{how}");
    }
    Ok(())
}
