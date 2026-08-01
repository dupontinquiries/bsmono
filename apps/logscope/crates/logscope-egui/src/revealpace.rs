//! revealpace.rs — reveal replies in sections, not tokens, when the model is fast enough.
//!
//! ## Why per-token is the wrong default
//!
//! Token-by-token reveal made sense when generation was slow: it was proof of life. On a model
//! producing tokens faster than a person reads, it is visual noise — the eye tracks a jittering edge
//! instead of reading, and half-formed words appear and resolve.
//!
//! Revealing at a **semantic boundary** — a finished sentence, a closed code fence, a completed list
//! item — gives the reader something coherent every time the text moves.
//!
//! ## But holding back is only free when the model is fast
//!
//! On a slow model, waiting for a sentence means the screen is still for seconds and the run looks
//! frozen. So the pacing is chosen from the MEASURED token rate rather than set once: fast models get
//! sections, slow ones keep the token stream, and the switch is per-generation.
//!
//! ## The boundary that never arrives
//!
//! A 200-line code block contains no sentence end. Waiting for one holds the entire block back and
//! then dumps it, which is worse than either mode. A flush timeout bounds it, so the reader never
//! waits longer than that however the text is shaped.

/// How to reveal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pace {
    /// Every token as it arrives. Proof of life on a slow model.
    Token,
    /// Hold until a semantic boundary or the flush timeout.
    Section,
}

/// Tokens per second above which sections read better than tokens.
///
/// Reading is roughly 5 words per second, and a token is under a word — so above this the text is
/// arriving faster than it can be read, and the jitter is costing more than the immediacy buys.
pub const SECTION_ABOVE_TOK_S: f32 = 12.0;

/// Longest a section may be withheld.
///
/// Bounds the case where no boundary arrives — a long code block, a table, a run-on. The reader never
/// waits longer than this however the text is shaped.
pub const FLUSH_MS: u64 = 400;

/// Choose a pace from the measured rate.
///
/// `None` before enough tokens to measure: starting in Section on an unmeasured model would freeze the
/// first render of a slow one, which is exactly when a user is deciding whether anything is happening.
pub fn pace(tok_per_s: Option<f32>) -> Pace {
    match tok_per_s {
        Some(r) if r >= SECTION_ABOVE_TOK_S => Pace::Section,
        _ => Pace::Token,
    }
}

/// Where the text may be cut so the revealed part is coherent.
///
/// Ordered by preference: a paragraph break is a better cut than a sentence end, which is better than
/// a clause. Cutting mid-word is never a boundary — that is the artifact section mode exists to remove.
pub fn last_boundary(s: &str) -> Option<usize> {
    // A closed code fence is the strongest boundary: revealing half a code block is worse than
    // revealing half a sentence, because the reader tries to parse it.
    if let Some(i) = s.rfind("```") {
        if s[..i].matches("```").count() % 2 == 1 { return Some(i + 3); }
    }
    for pat in ["\n\n", ". ", ".\n", "! ", "? ", "\n"] {
        if let Some(i) = s.rfind(pat) { return Some(i + pat.len()); }
    }
    None
}

/// How much of `buf` to show now.
///
/// Returns the byte count. `held_ms` is how long the tail has been withheld.
pub fn reveal_upto(buf: &str, p: Pace, held_ms: u64, done: bool) -> usize {
    if done || p == Pace::Token { return buf.len(); }
    if held_ms >= FLUSH_MS { return buf.len(); }
    last_boundary(buf).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_model_keeps_the_TOKEN_stream() {
        // Holding back on a slow model means the screen is still for seconds and the run looks frozen —
        // exactly when a user is deciding whether anything is happening.
        assert_eq!(pace(Some(3.0)), Pace::Token);
        assert_eq!(pace(None), Pace::Token, "unmeasured starts safe");
        assert_eq!(pace(Some(40.0)), Pace::Section);
    }

    #[test]
    fn a_half_finished_sentence_is_withheld_in_section_mode() {
        let buf = "The first point is done. And the second is half";
        let n = reveal_upto(buf, Pace::Section, 0, false);
        assert!(buf[..n].ends_with("done. "), "cut at the sentence end, got {:?}", &buf[..n]);
        assert!(!buf[..n].contains("half"));
    }

    #[test]
    fn an_UNCLOSED_code_fence_holds_the_block_back() {
        // Revealing half a code block is worse than half a sentence: the reader tries to parse it, and
        // a partial fence renders as broken markup rather than as text.
        let buf = "Here:\n\n```rust\nfn main() {\n    let x";
        let n = reveal_upto(buf, Pace::Section, 0, false);
        assert!(!buf[..n].contains("fn main"), "the open block is withheld: {:?}", &buf[..n]);
    }

    #[test]
    fn a_CLOSED_fence_is_the_strongest_boundary() {
        let buf = "Here:\n\n```rust\nfn main() {}\n```\nand then more";
        let n = reveal_upto(buf, Pace::Section, 0, false);
        assert!(buf[..n].contains("fn main() {}"), "a closed block is safe to show");
    }

    #[test]
    fn a_boundary_that_never_arrives_FLUSHES_on_the_timeout() {
        // A 200-line code block or a table contains no sentence end. Waiting holds the whole thing back
        // and then dumps it, which is worse than either mode.
        let buf = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(reveal_upto(buf, Pace::Section, 0, false), 0, "no boundary yet: withheld");
        assert_eq!(reveal_upto(buf, Pace::Section, FLUSH_MS, false), buf.len(), "timeout: flushed");
    }

    #[test]
    fn completion_always_reveals_everything() {
        // A held tail at the end of a generation would be text that never appears — the one outcome
        // worse than jitter.
        let buf = "an unterminated thought";
        assert_eq!(reveal_upto(buf, Pace::Section, 0, true), buf.len());
    }

    #[test]
    fn token_pace_never_withholds() {
        let buf = "half a wor";
        assert_eq!(reveal_upto(buf, Pace::Token, 0, false), buf.len());
    }

    #[test]
    fn the_threshold_is_above_reading_speed() {
        // Reading is roughly 5 words per second and a token is under a word, so the switch happens when
        // text is arriving faster than it can be read — not at an arbitrary number.
        assert!(SECTION_ABOVE_TOK_S > 10.0);
    }
}
