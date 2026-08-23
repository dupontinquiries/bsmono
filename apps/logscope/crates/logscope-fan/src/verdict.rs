//! verdict.rs — the compile oracle. Replace the code-block proxy (any ```fence``` = pass) with a REAL
//! pass/fail by running a compiler over a reply's code and believing the exit code. The proxy is useless on
//! small-model output — it called 19/19 hallucinated `bash` blocks a "pass" — so `:best` ranking, `:fill`
//! keep-green, and `:fa` pruning all need a verdict that reflects whether the code actually builds.
//!
//! Default checker: compile the extracted snippet as a standalone lib with `rustc` (grades self-contained
//! code; fragments and cross-crate references correctly fail). For real-crate grading, point `--check` at
//! `cargo check -p <crate>` and `--check-file` at the path to drop the code into.

use crate::bestregion::Verdict;
use std::process::Command;

/// Pull the code to compile from a reply: concatenate every ```fenced``` block (dropping the ```lang line).
/// Falls back to the whole trimmed text when there are no fences.
/// Language tags this oracle will compile. A fence with no tag, or one tagged
/// `text`/`output`/`console`, is not source — it is what the model is SHOWING
/// you, and concatenating it into the compile unit is why a correct answer
/// failed.
fn is_code_tag(tag: &str) -> bool {
    let t = tag.trim().to_ascii_lowercase();
    if t.is_empty() { return false }
    !matches!(t.as_str(),
        "text" | "output" | "console" | "sh" | "bash" | "shell" | "log" | "json"
        | "toml" | "yaml" | "yml" | "diff" | "md" | "markdown" | "plain" | "none")
}

pub fn extract_code(reply: &str) -> String {
    let mut blocks = Vec::new();
    // Fences whose tag says "not source". Kept separately rather than dropped:
    // they are the wrong thing to CONCATENATE with source, and the right thing
    // to fall back to when there is no source at all.
    let mut other: Vec<String> = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let nl = after.find('\n');
        // The fence's language tag is everything up to the newline.
        let tag = &after[..nl.unwrap_or(0)];
        let body_start = nl.map(|i| i + 1).unwrap_or(after.len());
        let body = &after[body_start..];
        if let Some(end) = body.find("```") {
            // ONLY SOURCE FENCES. This joined EVERY fenced block, and the
            // commonest shape of a model reply is the code followed by a fence
            // showing what it prints:
            //
            //     ```rust
            //     pub fn f() -> i32 { 1 }
            //     ```
            //     Running it prints:
            //     ```
            //     1
            //     ```
            //
            // The join produced `pub fn f() -> i32 { 1 }` then a bare `1`,
            // which does not compile — so the oracle failed the branch for
            // SHOWING ITS OUTPUT. Measured, not hypothesised.
            if is_code_tag(tag) {
                blocks.push(body[..end].to_string());
            } else {
                other.push(body[..end].to_string());
            }
            rest = &body[end + 3..];
        } else { break; }
    }
    // THREE TIERS, and the middle one exists because skipping non-source fences
    // introduced a regression I had to go looking for.
    //
    // A worker asked to produce a manifest answers with one `toml` fence and
    // nothing else. With only the source/prose split, that had no source
    // fences and fell all the way through to the raw reply — handing the
    // checker the prose AND the ``` markers, which is worse than the bug this
    // function was just fixed for. `cmd` is caller-supplied, so a non-Rust
    // checker is an ordinary case rather than a hypothetical.
    if !blocks.is_empty() {
        blocks.join("\n\n")
    } else if !other.is_empty() {
        // No source, but fenced content exists: it is what the worker meant to
        // deliver, whatever its tag claims.
        other.join("\n\n")
    } else {
        reply.trim().to_string()
    }
}

/// Compile `code` with shell command `cmd`, returning Pass/Fail (None if the code is empty or the checker
/// can't run). The code is written to `file` (or a temp `.rs`). If `cmd` contains `{}` it's replaced with
/// that path; if a `file` was given (a project drop-in) the cmd runs as-is; otherwise the path is appended
/// (rustc-style). Empty/whitespace code is None (nothing to grade), so it doesn't count against a branch.
pub fn compile(code: &str, cmd: &str, file: Option<&str>) -> Verdict {
    if code.trim().is_empty() { return Verdict::None; }
    let path = match file {
        Some(f) => std::path::PathBuf::from(f),
        None => std::env::temp_dir().join(format!("logscope_check_{}_{}.rs", std::process::id(), now_nanos())),
    };
    if std::fs::write(&path, code).is_err() { return Verdict::None; }
    let p = path.to_string_lossy().to_string();
    let full = if cmd.contains("{}") { cmd.replace("{}", &p) }
               else if file.is_some() { cmd.to_string() }
               else { format!("{cmd} {p}") };
    let verdict = match Command::new("sh").arg("-c").arg(&full).output() {
        Ok(o) if o.status.success() => Verdict::Pass,
        Ok(_) => Verdict::Fail,
        Err(_) => Verdict::None, // no toolchain / couldn't spawn — don't fake a verdict
    };
    if file.is_none() { let _ = std::fs::remove_file(&path); }
    verdict
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// The default snippet checker — compiles as a lib, no codegen, output discarded.
pub const DEFAULT_CHECK: &str = "rustc --crate-type lib --edition 2021 --emit=metadata -o /dev/null";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_concatenates_fenced_blocks() {
        let r = "first:\n```rust\nfn a() {}\n```\nand:\n```rust\nfn b() {}\n```";
        let c = extract_code(r);
        assert!(c.contains("fn a() {}") && c.contains("fn b() {}"));
        assert_eq!(extract_code("no fences, just prose"), "no fences, just prose");
    }

    #[test]
    fn compile_grades_real_code() {
        // these only run where rustc exists; assert the pass/fail discrimination
        assert_eq!(compile("pub fn add(a:i32,b:i32)->i32{a+b}", DEFAULT_CHECK, None), Verdict::Pass);
        assert_eq!(compile("pub fn add(a:i32,b:i32)->i32{a+}", DEFAULT_CHECK, None), Verdict::Fail);
        assert_eq!(compile("   ", DEFAULT_CHECK, None), Verdict::None, "empty code is not graded");
    }
}

#[cfg(test)]
mod fence_tests {
    use super::*;

    /// The bug: a reply that shows its own output failed to compile.
    #[test]
    fn an_output_block_is_not_concatenated_into_the_compile_unit() {
        let reply = "Here is the function:\n\n```rust\npub fn f() -> i32 { 1 }\n```\n\n\
Running it prints:\n\n```\n1\n```\n";
        let code = extract_code(reply);
        assert!(code.contains("pub fn f"), "the source must survive: {code:?}");
        assert!(!code.lines().any(|l| l.trim() == "1"),
                "the untagged output block was concatenated into the compile unit — this is why \
a correct answer was graded as a compile failure: {code:?}");
    }

    #[test]
    fn several_source_blocks_still_join() {
        let reply = "```rust\nfn a() {}\n```\nand\n```rust\nfn b() {}\n```\n";
        let code = extract_code(reply);
        assert!(code.contains("fn a") && code.contains("fn b"),
                "a reply split across two source fences must still compile as one unit: {code:?}");
    }

    #[test]
    fn non_source_tags_are_skipped_by_name() {
        for tag in ["text", "output", "console", "bash", "json", "toml", "diff"] {
            let reply = format!("```rust\nfn a() {{}}\n```\n```{tag}\nnot source\n```\n");
            let code = extract_code(&reply);
            assert!(!code.contains("not source"), "`{tag}` block leaked into the code: {code:?}");
        }
    }

/// The regression the tag exclusion introduced, found by asking what a
    /// worker producing a NON-Rust artifact would get.
    #[test]
    fn a_reply_whose_only_fence_is_non_source_yields_that_fence_not_the_prose() {
        for tag in ["toml", "json", "yaml", "diff", "text"] {
            let reply = format!("Here is the manifest:\n\n```{tag}\n[package]\nname = \"x\"\n```\n");
            let code = extract_code(&reply);
            assert!(code.contains("[package]"), "`{tag}`: content lost: {code:?}");
            assert!(!code.contains("Here is the manifest"),
                    "`{tag}`: the prose leaked into the checker: {code:?}");
            assert!(!code.contains("```"),
                    "`{tag}`: the fence markers leaked into the checker: {code:?}");
        }
    }

    /// And source still wins when both are present — the whole point of the
    /// split.
    #[test]
    fn source_wins_over_a_non_source_fence_in_the_same_reply() {
        let reply = "```rust\nfn a() {}\n```\n```toml\n[package]\n```\n";
        let code = extract_code(reply);
        assert!(code.contains("fn a"));
        assert!(!code.contains("[package]"), "the toml block must not be concatenated: {code:?}");
    }

    #[test]
    fn a_reply_with_no_fences_is_still_treated_as_code() {
        // Unchanged behaviour: some workers answer with bare source.
        assert_eq!(extract_code("fn a() {}"), "fn a() {}");
    }

    /// The regression this could cause: a reply whose ONLY fence is untagged.
    /// Before, that was the code; now it is skipped, so the fallback must catch
    /// it rather than handing the compiler an empty string.
    #[test]
    fn a_lone_untagged_fence_falls_back_rather_than_compiling_nothing() {
        let code = extract_code("```\nfn a() {}\n```");
        assert!(code.contains("fn a"),
                "an untagged sole fence must not leave the oracle with nothing to compile: {code:?}");
    }
}
