//! Process-wide CodeMode for the cardinal-grid-words layout.
//!
//! The cardinal-grid-words layout has 5 columns where the rightmost 2
//! columns (6 cells total across the 3 letter rows) hold either:
//!   - prose word clusters ("the/a/an/this/that", "I/I'm/I've/I'll/I'd",
//!     "is/are/was/were/be", "and/but/or/so/because", "to/of/in/for/with",
//!     "have/has/had/do/did"), or
//!   - code templates for one of {C, Rust, Python}.
//!
//! The mode is process-wide because the layout function reads it
//! during `rebuild_view`, which is called from many sites — threading
//! a `CodeMode` parameter through every call would touch lots of code
//! for a single read.
//!
//! The atomic is `u8` because the variants are small and the cycle
//! is a simple modulo. AcqRel ordering isn't needed — Relaxed is
//! fine because the value only changes via UI events on the main
//! thread, and any reader sees a consistent value (no other field
//! depends on the read).

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodeMode {
    /// Prose word clusters — natural-language sentence building.
    Prose = 0,
    /// C templates: #include, control flow, printf, ptr/mem, struct.
    C = 1,
    /// Rust templates: fn/pub, let/mut, control flow, match, ?/unwrap,
    /// derive/trait.
    Rust = 2,
    /// Python templates: def/class, control flow, import, self/__init__,
    /// return/raise, print/format.
    Python = 3,
}

impl CodeMode {
    /// Short label for the cycle button. Single-char or two-char so
    /// it fits in the bottom-row toggle cell without truncation.
    pub fn short_label(self) -> &'static str {
        match self {
            CodeMode::Prose => "EN",
            CodeMode::C => "C",
            CodeMode::Rust => "Rs",
            CodeMode::Python => "Py",
        }
    }

    /// Next mode in the cycle. Independent of the atomic so callers
    /// can compute a target state without touching shared state.
    pub fn next(self) -> Self {
        match self {
            CodeMode::Prose => CodeMode::C,
            CodeMode::C => CodeMode::Rust,
            CodeMode::Rust => CodeMode::Python,
            CodeMode::Python => CodeMode::Prose,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => CodeMode::C,
            2 => CodeMode::Rust,
            3 => CodeMode::Python,
            _ => CodeMode::Prose,
        }
    }
}

static CODE_MODE: AtomicU8 = AtomicU8::new(CodeMode::Prose as u8);

/// Current mode. Read by the layout function during rebuild.
pub fn current() -> CodeMode {
    CodeMode::from_u8(CODE_MODE.load(Ordering::Relaxed))
}

/// Set the mode. Called by the UI listener on CodeLangCycle, and
/// at boot if the user persisted a non-default mode.
pub fn set(mode: CodeMode) {
    CODE_MODE.store(mode as u8, Ordering::Relaxed);
}

/// Cycle to the next mode and return what it's set to. Convenience
/// wrapper for the UI listener's CodeLangCycle handler.
pub fn cycle() -> CodeMode {
    let next = current().next();
    set(next);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_prose() {
        // Reset to a known value (other tests may have mutated).
        set(CodeMode::Prose);
        assert_eq!(current(), CodeMode::Prose);
    }

    #[test]
    fn cycle_goes_prose_c_rust_python_back() {
        set(CodeMode::Prose);
        assert_eq!(cycle(), CodeMode::C);
        assert_eq!(cycle(), CodeMode::Rust);
        assert_eq!(cycle(), CodeMode::Python);
        assert_eq!(cycle(), CodeMode::Prose);
    }

    #[test]
    fn short_labels_are_compact() {
        // Bottom-row cell is ~1 unit wide; labels must fit without
        // truncation at the typical key font size.
        for m in [CodeMode::Prose, CodeMode::C, CodeMode::Rust, CodeMode::Python] {
            assert!(
                m.short_label().chars().count() <= 3,
                "label '{}' too long",
                m.short_label()
            );
        }
        // Reset for other tests.
        set(CodeMode::Prose);
    }
}
