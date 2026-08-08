//! ghost — the inline-suggestion state machine, egui-free.
//!
//! Three states and every transition is a method, so the whole lifecycle is
//! unit-tested here and the view layer only *draws*:
//!
//! ```text
//! Idle ──request──▶ Pending{gen} ──outcome(gen matches)──▶ Shown{text}
//!   ▲                   │  outcome stale/err → Idle           │
//!   └──── dismiss ──────┴──── edit that diverges ─────────────┘
//!                              (typing the ghost's own next chars instead
//!                               CONSUMES them and stays Shown)
//! ```
//!
//! The typing-through rule is the one that makes ghost text feel like an
//! instrument instead of a popup: if the suggestion is `intln!("hi");` and
//! you type `i`, the ghost doesn't flicker away and re-request — it eats the
//! `i` and keeps offering the rest.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ghost {
    Idle,
    Pending { generation: u64 },
    Shown { text: String },
}

impl Ghost {
    pub fn is_shown(&self) -> bool {
        matches!(self, Ghost::Shown { .. })
    }
    pub fn is_pending(&self) -> bool {
        matches!(self, Ghost::Pending { .. })
    }

    pub fn request_sent(&mut self, generation: u64) {
        *self = Ghost::Pending { generation };
    }

    /// Feed an engine outcome. Accepts only a success matching the buffer
    /// version we're *now* at; everything else quietly returns to Idle (an
    /// error message is the caller's to log — the loop pane does).
    pub fn on_outcome(&mut self, generation: u64, current_version: u64, result: &Result<String, String>) {
        let expected = matches!(*self, Ghost::Pending { generation: g } if g == generation);
        match result {
            Ok(text) if expected && generation == current_version && !text.is_empty() => {
                *self = Ghost::Shown { text: text.clone() };
            }
            _ => {
                if expected {
                    *self = Ghost::Idle;
                }
            }
        }
    }

    /// The user typed `typed`. Returns true when the ghost consumed it
    /// (i.e. the edit matched the suggestion's own next chars).
    pub fn on_text_inserted(&mut self, typed: &str) -> bool {
        if let Ghost::Shown { text } = self {
            if let Some(rest) = text.strip_prefix(typed) {
                if rest.is_empty() {
                    *self = Ghost::Idle; // typed the whole thing yourself
                } else {
                    *text = rest.to_string();
                }
                return true;
            }
        }
        *self = Ghost::Idle;
        false
    }

    /// Any non-matching mutation, caret move, or Escape.
    pub fn dismiss(&mut self) {
        *self = Ghost::Idle;
    }

    /// Take the full suggestion for insertion.
    pub fn accept_all(&mut self) -> Option<String> {
        match std::mem::replace(self, Ghost::Idle) {
            Ghost::Shown { text } => Some(text),
            other => {
                *self = other; // put a Pending back; taking it would drop a request
                None
            }
        }
    }

    /// Take just the next word (leading spaces + run of word chars, or a
    /// single newline+indent) — the Ctrl+→ partial accept.
    pub fn accept_word(&mut self) -> Option<String> {
        let Ghost::Shown { text } = self else { return None };
        let word = next_word(text);
        if word.is_empty() {
            return None;
        }
        let rest = text[word.len()..].to_string();
        let out = word.to_string();
        if rest.is_empty() {
            *self = Ghost::Idle;
        } else {
            *self = Ghost::Shown { text: rest };
        }
        Some(out)
    }
}

fn next_word(s: &str) -> &str {
    let b = s.as_bytes();
    if b.first() == Some(&b'\n') {
        // newline + its indentation is one "word": accepting it lands you
        // at the start of the suggested next line, ready for the next tap.
        let mut i = 1;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        return &s[..i];
    }
    let mut i = 0;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let start_alpha = i;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_alphanumeric() || c == '_' {
            i += 1;
        } else {
            break;
        }
    }
    if i == start_alpha && i < b.len() && b[i] != b'\n' {
        i += 1; // a lone punctuation char counts as the next word
    }
    &s[..i]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_pending_to_shown_gated_by_generation() {
        let mut g = Ghost::Idle;
        g.request_sent(5);
        g.on_outcome(4, 5, &Ok("stale".into()));
        assert!(g.is_pending(), "stale outcome ignored, still waiting");
        g.on_outcome(5, 6, &Ok("late".into()));
        assert_eq!(g, Ghost::Idle, "right request, but the buffer moved on");
        g.request_sent(7);
        g.on_outcome(7, 7, &Ok("println!".into()));
        assert_eq!(g, Ghost::Shown { text: "println!".into() });
    }

    #[test]
    fn typing_through_consumes_and_divergence_dismisses() {
        let mut g = Ghost::Shown { text: "intln!(\"hi\")".into() };
        assert!(g.on_text_inserted("in"));
        assert_eq!(g, Ghost::Shown { text: "tln!(\"hi\")".into() });
        assert!(!g.on_text_inserted("x"), "divergent keystroke");
        assert_eq!(g, Ghost::Idle);
    }

    #[test]
    fn word_accept_walks_the_suggestion() {
        let mut g = Ghost::Shown { text: "let mut x = 1;\n    next()".into() };
        assert_eq!(g.accept_word().unwrap(), "let");
        assert_eq!(g.accept_word().unwrap(), " mut");
        assert_eq!(g.accept_word().unwrap(), " x");
        assert_eq!(g.accept_word().unwrap(), " =");
        assert_eq!(g.accept_word().unwrap(), " 1");
        assert_eq!(g.accept_word().unwrap(), ";");
        assert_eq!(g.accept_word().unwrap(), "\n    ");
        assert_eq!(g.accept_word().unwrap(), "next");
        let mut all = Ghost::Shown { text: "()".into() };
        assert_eq!(all.accept_all().unwrap(), "()");
        assert_eq!(all, Ghost::Idle);
    }
}
