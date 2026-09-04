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
/// Extract the source a worker meant to deliver.
///
/// Delegates the SCAN to [`crate::reply::fences`] and keeps only the POLICY:
/// prefer source blocks, fall back to non-source fenced content, fall back to
/// the raw reply. The scan used to live here as a hand-rolled next-fence
/// search, which is how it came to join an output block into the compile unit
/// and fail correct answers.
///
/// Behaviour is unchanged for every case the old parser got right, and one case
/// it got wrong now works: a four-backtick block containing three-backtick
/// blocks. CommonMark closes a fence with a run at least as long as the opener,
/// so ` ````md ` wrapping a ` ```rust ` example is ONE block whose body
/// contains the inner fence — the old scan cut it at the inner fence and
/// silently dropped the rest.
pub fn extract_code(reply: &str) -> String {
    let fences = crate::reply::fences(reply);

    // Tier 1: source blocks, joined. A reply legitimately splits across two
    // ```rust fences and both belong to one compile unit.
    let src: Vec<&str> = fences.iter().filter(|f| f.is_source()).map(|f| f.body).collect();
    if !src.is_empty() {
        return src.join("\n\n");
    }

    // Tier 2: fenced content whose tag says it is not source. A worker asked
    // for a manifest answers with one `toml` fence and nothing else, and `cmd`
    // is caller-supplied so a non-Rust checker is ordinary. Falling past this
    // to the raw reply hands the checker the prose AND the fence markers.
    let other: Vec<&str> = fences.iter().map(|f| f.body).collect();
    if !other.is_empty() {
        return other.join("\n\n");
    }

    // Tier 3: no fences at all — some workers answer with bare source.
    reply.trim().to_string()
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
        // Exit 127 is `sh`'s "command not found" — the checker binary itself is missing, not a
        // real compile failure. `sh -c` spawns fine either way, so this can't be caught by the
        // Err arm below; it has to be read off the exit code. A device with no rustc in PATH
        // (this project's actual mobile targets don't ship one) would otherwise grade every
        // snippet Fail and silently poison :best ranking / :fill keep-green with a signal that
        // was never measuring the code at all.
        Ok(o) if o.status.code() == Some(127) => Verdict::None,
        Ok(_) => Verdict::Fail,
        Err(_) => Verdict::None, // couldn't spawn `sh` itself — don't fake a verdict
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

    /// C14_TODO §21: on a device with no rustc in PATH, `sh -c "rustc …"` still spawns —
    /// it's `sh` that runs, and `sh` exits 127 ("command not found"). That used to fall into the
    /// same match arm as a genuine compile failure, so a fleet device without a Rust toolchain
    /// graded every real-code reply Fail instead of skipping the check. Uses a checker command
    /// that can't possibly exist rather than trying to actually uninstall rustc for the test.
    #[test]
    fn a_missing_checker_binary_is_none_not_fail() {
        let bogus = "logscope_verdict_test_binary_that_does_not_exist_anywhere --emit=metadata -o /dev/null";
        assert_eq!(compile("pub fn add(a:i32,b:i32)->i32{a+b}", bogus, None), Verdict::None,
            "a missing checker must not be graded as a compile failure");
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

/// The case the hand-rolled scan got wrong and the shared one does not.
    ///
    /// A worker asked to write documentation answers with a four-backtick
    /// block wrapping a three-backtick example. CommonMark says that is ONE
    /// block; the old next-fence search cut it at the inner fence and dropped
    /// everything after, so the oracle compiled a fragment.
    #[test]
    fn a_four_tick_block_is_not_cut_at_its_inner_fence() {
        let reply = "````rust\n/// Example:\n/// ```\n/// let x = 1;\n/// ```\n\
pub fn f() -> i32 { 1 }\n````\n";
        let code = extract_code(reply);
        assert!(code.contains("pub fn f"), "the source after the inner fence was lost: {code:?}");
        assert!(code.contains("let x = 1"), "the inner example was lost: {code:?}");
    }

    /// And the migration must not have changed the tier order.
    #[test]
    fn source_still_wins_and_non_source_still_rescues() {
        // Both present: source only.
        let both = extract_code("```rust\nfn a() {}\n```\n```\noutput\n```\n");
        assert!(both.contains("fn a") && !both.contains("output"));
        // Only non-source: it is the answer, without the prose.
        let only = extract_code("Here:\n```toml\n[package]\n```\n");
        assert!(only.contains("[package]") && !only.contains("Here:"));
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
