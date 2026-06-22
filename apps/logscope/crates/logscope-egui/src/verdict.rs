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
pub fn extract_code(reply: &str) -> String {
    let mut blocks = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
        let body = &after[body_start..];
        if let Some(end) = body.find("```") {
            blocks.push(body[..end].to_string());
            rest = &body[end + 3..];
        } else { break; }
    }
    if blocks.is_empty() { reply.trim().to_string() } else { blocks.join("\n\n") }
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
