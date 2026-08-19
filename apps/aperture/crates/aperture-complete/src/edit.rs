//! edit — the request-building half of "AI reasoning assisted edits": ask a
//! provider to rewrite a selection given a natural-language instruction
//! ("make this iterative", "add bounds checking", "rewrite as a match"),
//! rather than the FIM-only "what comes next at the caret" ghost completion
//! already covers.
//!
//! Deliberately reuses [`crate::CompletionRequest`]'s envelope instead of
//! adding a parallel request type: `prefix` carries the instruction,
//! `suffix` carries the selected text, `lang` is unchanged. This is a
//! *documented wire convention*, not a coincidence of shared field names —
//! it means an edit request can go through the exact same [`crate::Provider`]
//! trait, the same [`crate::engine::Engine`] worker thread, and the same
//! generation-gate staleness handling ghost completion already has, with
//! zero new machinery. The cost of the reuse is honesty about fit:
//!
//! * [`crate::providers::CommandProvider`] is the real fit today — a
//!   subprocess reads `instruction NUL selection` on stdin exactly the way
//!   it already reads `prefix NUL suffix`, and is free to call any
//!   chat-capable backend it likes with that content. Nothing here assumes
//!   FIM semantics.
//! * [`crate::providers::OllamaProvider`] will still send this through its
//!   FIM `prefix`/`suffix` request fields, which is a poor semantic fit for
//!   an instruction ("rewrite this" is not "the code that comes after
//!   this") — expect weak results there until it grows a real chat-completion
//!   path. Not attempted here: that's a bigger, separate change (a chat
//!   endpoint has a different request/response shape than FIM), not a
//!   should-have-been-included part of this one.
//!
//! Shell-side application (a diff/review UI, accept/reject, applying the
//! result as a buffer edit) is intentionally NOT here — this module only
//! builds the request. See `TODO.md`'s aperture section for the wiring
//! this enables but doesn't yet do.

use crate::CompletionRequest;

/// Build an edit request. `budget` caps the instruction and selection each
/// (independently — an instruction shouldn't be able to starve the
/// selection out of the request, or vice versa), the same char-boundary-safe
/// tail-trim `make_request` uses for FIM context, because an instruction
/// that arrives mid-thought is at least as confusing to a model as
/// code missing its opening brace.
pub fn make_edit_request(instruction: &str, selection: &str, lang: &str, generation: u64, budget: usize) -> CompletionRequest {
    CompletionRequest {
        generation,
        prefix: take_chars(instruction, budget),
        suffix: take_chars(selection, budget),
        lang: lang.to_string(),
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_instruction_and_selection_into_the_completion_envelope() {
        let req = make_edit_request("add bounds checking", "fn get(v: &[i32], i: usize) -> i32 { v[i] }", "rust", 3, 1000);
        assert_eq!(req.generation, 3);
        assert_eq!(req.prefix, "add bounds checking");
        assert_eq!(req.suffix, "fn get(v: &[i32], i: usize) -> i32 { v[i] }");
        assert_eq!(req.lang, "rust");
    }

    #[test]
    fn budget_trims_each_field_independently_on_char_boundaries() {
        let instruction = "explain step by step 🙂 in great detail";
        let selection = "x".repeat(50);
        let req = make_edit_request(instruction, &selection, "rust", 1, 10);
        assert_eq!(req.prefix.chars().count(), 10);
        assert_eq!(req.suffix.chars().count(), 10);
        // A long selection can't starve the instruction, and vice versa —
        // each field gets its own budget, not a shared pool.
        assert!(req.prefix.chars().count() > 0);
        assert!(req.suffix.chars().count() > 0);
    }

    #[test]
    fn short_fields_are_left_untrimmed() {
        let req = make_edit_request("short", "also short", "python", 0, 1000);
        assert_eq!(req.prefix, "short");
        assert_eq!(req.suffix, "also short");
    }
}
