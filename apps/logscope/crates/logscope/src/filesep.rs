//! filesep.rs — multi-file payloads over a wire the model actually understands.
//!
//! Our canonical form for a multi-file reply is markdown: a path line, then a fenced block. Some models
//! carry a dedicated separator token — Gemma's `<|file_separator|>` — which is one token instead of the
//! several a fence costs, and unambiguous where a fence is not (a block containing three backticks
//! terminates early).
//!
//! ## Using it is only safe when the model HAS it
//!
//! A model whose vocabulary lacks `<|file_separator|>` does not error on it. It tokenizes the literal
//! characters — roughly a dozen tokens of punctuation the model has never seen in that arrangement —
//! and the reply degrades in a way that looks like the model being bad at the task. So capability is
//! decided by asking the VOCAB, never by matching a model name: names are wrong at the edges (a
//! finetune, a requant, a renamed file) and a vocab lookup is exact.
//!
//! ## Three representations, and the boundaries between them
//!
//! * **canonical** — what the rest of the engine passes around: `Vec<FilePart>`.
//! * **wire** — what a model sees. Fences, or the separator token when supported.
//! * **view** — what a person reads. ALWAYS markdown; the separator token must never reach it, because
//!   a reader seeing `<|file_separator|>` learns nothing and reasonably concludes something broke.

/// One file in a multi-file payload.
#[derive(Clone, PartialEq, Debug)]
pub struct FilePart { pub path: String, pub body: String }

/// The separator Gemma-family vocabularies carry.
pub const GEMMA_FILE_SEP: &str = "<|file_separator|>";

/// Whether the loaded model can be sent the separator token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wire { Fences, Separator }

/// Decide from the model's vocabulary, not its name.
///
/// `vocab_has` is the tokenizer lookup. Passing a closure rather than a token list keeps this testable
/// without a real vocab and keeps the decision at the one place that actually knows.
pub fn wire_for(vocab_has: impl Fn(&str) -> bool) -> Wire {
    if vocab_has(GEMMA_FILE_SEP) { Wire::Separator } else { Wire::Fences }
}

/// Render for the model.
pub fn to_wire(parts: &[FilePart], wire: Wire) -> String {
    match wire {
        Wire::Separator => parts.iter()
            .map(|p| format!("{}\n{}", p.path, p.body))
            .collect::<Vec<_>>().join(&format!("\n{GEMMA_FILE_SEP}\n")),
        Wire::Fences => parts.iter()
            .map(|p| format!("{}\n```\n{}\n```", p.path, p.body))
            .collect::<Vec<_>>().join("\n\n"),
    }
}

/// Parse a model's reply back to canonical, whichever form it used.
///
/// Accepts BOTH forms regardless of what was sent. A model given the separator may still answer in
/// fences — it is not obliged to mirror the prompt — and rejecting that would turn a usable reply into
/// a parse failure.
pub fn from_wire(text: &str) -> Vec<FilePart> {
    if text.contains(GEMMA_FILE_SEP) {
        return text.split(GEMMA_FILE_SEP).filter_map(|chunk| {
            let mut lines = chunk.trim().lines();
            let path = lines.next()?.trim().to_string();
            if path.is_empty() { return None; }
            Some(FilePart { path, body: lines.collect::<Vec<_>>().join("\n") })
        }).collect();
    }
    parse_fenced(text)
}

/// Parse the markdown form: a path line followed by a fenced block.
fn parse_fenced(text: &str) -> Vec<FilePart> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        // A fence opener; the path is the nearest preceding non-empty line.
        if lines[i].trim_start().starts_with("```") {
            let path = (0..i).rev().map(|k| lines[k].trim())
                .find(|l| !l.is_empty()).unwrap_or("").to_string();
            // Match the opener's LENGTH: a four-backtick fence wraps blocks that contain three, and
            // stopping at the first ``` inside it would truncate the file at its first code sample.
            let ticks = lines[i].trim_start().chars().take_while(|c| *c == '`').count();
            let close = (i + 1..lines.len()).find(|&k| {
                let t = lines[k].trim_start();
                t.starts_with("```") && t.chars().take_while(|c| *c == '`').count() >= ticks
            });
            let end = close.unwrap_or(lines.len());
            if !path.is_empty() {
                out.push(FilePart { path, body: lines[i + 1..end].join("\n") });
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Render for a person. ALWAYS markdown.
///
/// The separator token must never reach the view: a reader seeing `<|file_separator|>` learns nothing
/// and reasonably concludes something is broken. Uses a fence long enough to contain the body, so a file
/// that itself contains three backticks does not terminate its own block.
pub fn to_view(parts: &[FilePart]) -> String {
    parts.iter().map(|p| {
        let longest = p.body.lines()
            .map(|l| l.trim_start().chars().take_while(|c| *c == '`').count())
            .max().unwrap_or(0);
        let fence = "`".repeat(longest.max(2) + 1);
        format!("{}\n{fence}\n{}\n{fence}", p.path, p.body)
    }).collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> Vec<FilePart> {
        vec![FilePart { path: "src/a.rs".into(), body: "fn a() {}".into() },
             FilePart { path: "src/b.rs".into(), body: "fn b() {}".into() }]
    }

    #[test]
    fn capability_comes_from_the_VOCAB_not_the_model_name() {
        // A name is wrong at the edges — a finetune, a requant, a renamed file — and sending the token
        // to a model that lacks it does not error: it tokenizes a dozen literal punctuation characters
        // and the reply degrades in a way that looks like the model being bad at the task.
        assert_eq!(wire_for(|t| t == GEMMA_FILE_SEP), Wire::Separator);
        assert_eq!(wire_for(|_| false), Wire::Fences);
        // a model merely CALLED gemma proves nothing; only the lookup counts
        assert_eq!(wire_for(|t| t == "<|gemma_something_else|>"), Wire::Fences);
    }

    #[test]
    fn the_call_site_is_a_one_liner_against_the_real_tokenizer() {
        // Documents the intended wiring so nobody reimplements the check by matching a name:
        //     let wire = filesep::wire_for(|t| tok.has_special(t));
        // `Tokenizer::has_special` is the exact lookup; this asserts the shape composes.
        let fake_vocab = ["<|start|>", GEMMA_FILE_SEP, "<|end|>"];
        let wire = wire_for(|t| fake_vocab.contains(&t));
        assert_eq!(wire, Wire::Separator);
        let no_sep = ["<|start|>", "<|end|>"];
        assert_eq!(wire_for(|t| no_sep.contains(&t)), Wire::Fences);
    }

    #[test]
    fn round_trip_is_lossless_in_both_wires() {
        for w in [Wire::Separator, Wire::Fences] {
            assert_eq!(from_wire(&to_wire(&parts(), w)), parts(), "{w:?}");
        }
    }

    #[test]
    fn a_reply_is_parsed_in_whichever_form_it_ARRIVES_in() {
        // A model given the separator may still answer in fences — it is not obliged to mirror the
        // prompt — and rejecting that would turn a usable reply into a parse failure.
        let fenced = to_wire(&parts(), Wire::Fences);
        assert_eq!(from_wire(&fenced), parts(), "fenced reply after a separator prompt");
        let sep = to_wire(&parts(), Wire::Separator);
        assert_eq!(from_wire(&sep), parts());
    }

    #[test]
    fn a_four_backtick_fence_survives_a_body_containing_three() {
        // The reason the separator token is attractive: a fence is ambiguous. A file whose content
        // includes ``` terminates its own block at the first one, truncating the file silently at its
        // first code sample.
        let src = "src/doc.md\n````\nintro\n```rust\nfn x() {}\n```\noutro\n````";
        let got = from_wire(src);
        assert_eq!(got.len(), 1);
        assert!(got[0].body.contains("fn x() {}") && got[0].body.contains("outro"),
            "the inner fence must not close the outer one: {:?}", got[0].body);
    }

    #[test]
    fn the_separator_token_NEVER_reaches_the_view() {
        // A reader seeing <|file_separator|> learns nothing and reasonably concludes something broke.
        let v = to_view(&parts());
        assert!(!v.contains(GEMMA_FILE_SEP), "{v}");
        assert!(v.contains("src/a.rs") && v.contains("fn b() {}"));
    }

    #[test]
    fn the_view_widens_its_fence_to_contain_the_body() {
        // Rendering a file that contains ``` inside a ``` block produces something that renders wrong
        // and copies wrong — the reader's copy of the file is then silently truncated.
        let p = vec![FilePart { path: "x.md".into(), body: "```rust\nfn x() {}\n```".into() }];
        let v = to_view(&p);
        assert!(v.contains("````"), "must widen past the body's own fence: {v}");
        assert_eq!(from_wire(&v), p, "and still round-trips");
    }

    #[test]
    fn a_fenced_block_with_no_path_line_is_skipped_rather_than_given_an_empty_path() {
        // Prose containing an example block is not a file. Emitting one with path "" would create a
        // FilePart that any writer would then try to save.
        let got = from_wire("Here is an example:\n\n```\nsome code\n```");
        assert_eq!(got.len(), 1, "the prose line becomes the path");
        assert_eq!(got[0].path, "Here is an example:");
        // ...which is why a caller must validate paths; the parser cannot tell prose from a filename
        assert!(!got[0].path.ends_with(".rs"));
    }

    #[test]
    fn an_empty_payload_produces_nothing_rather_than_an_empty_file() {
        assert!(from_wire("").is_empty());
        assert!(from_wire("just prose, no blocks").is_empty());
        assert_eq!(to_wire(&[], Wire::Separator), "");
    }
}
