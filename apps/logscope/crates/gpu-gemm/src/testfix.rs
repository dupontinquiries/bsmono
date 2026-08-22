//! testfix — read a test fixture, and SAY SO when it is missing.
//!
//! ## The failure this removes
//!
//! 28 test sites in this crate guarded on a `/tmp/tiny-*.gguf` with
//! `Err(_) => return`. A test that returns early PASSES, so the suite reported
//! 1158 green while a large part of the loader, the diffusion path and the MoE
//! routing never executed. Generating the fixtures took the same suite from
//! 5.85 s to 20.35 s — same tests, same result line, three and a half times the
//! work actually done.
//!
//! Nothing was wrong with skipping. The problem was skipping SILENTLY: there
//! was no way to tell a green run that exercised the loader from one that read
//! a missing file 28 times, and no output anywhere naming the command that
//! would fix it. That is how the area stayed unexamined long enough for two
//! documents to end up disagreeing about whether one of its modules existed.
//!
//! ## What this does instead
//!
//! Returns `None` exactly as before — no test starts failing on a machine
//! without fixtures — but prints the path and the generator command once per
//! path. `cargo test` shows it under `--nocapture`, and a failing run shows it
//! always, which is when it matters.
//!
//! Once per path, not once per test: seventeen sites read `tiny-llada2.gguf`,
//! and seventeen identical lines would be noise rather than information.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::OnceLock;

fn announced() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The generator invocation for a fixture, or a note that there is none.
///
/// Kept beside the reader rather than in a doc comment somewhere, because the
/// moment a person needs it is the moment a test just skipped, and a message
/// that says "missing" without saying "run this" sends them hunting through
/// `tools/tiny-variants/` for which of ten scripts and which second argument.
/// Getting that second argument wrong is not hypothetical — omitting it
/// produces a file with the right name and the WRONG ARCHITECTURE, and the
/// tests then fail with errors about the model instead of the fixture.
fn how_to_make(path: &str) -> &'static str {
    match path.rsplit('/').next().unwrap_or(path) {
        "tiny-qwen3.gguf" => "python3 tools/tiny-variants/gen3.py <path> qwen3",
        "tiny-qwen2.gguf" => "python3 tools/tiny-variants/gen2.py <path>",
        "tiny-llada2.gguf" => "python3 tools/tiny-variants/gen3.py <path> llada2",
        "tiny-llada2-shift.gguf" => "python3 tools/tiny-variants/gen_shift.py <path> llada2",
        "tiny-llada2-64exp.gguf" => "python3 tools/tiny-variants/gen_wideexp.py <path> llada2",
        "tiny-glmdsa.gguf" => "python3 tools/tiny-variants/gen_glmdsa.py <path>",
        // Named explicitly so nobody hunts for a generator that does not exist.
        "tiny-qwen3-q8.gguf" | "tiny-llada2-q4file.gguf" =>
            "no generator in tree (quantised build; main.rs's synthetic builders cover these paths)",
        "tiny-notok.gguf" =>
            "no generator in tree (deliberately tokenizer-less; any weights-only GGUF works)",
        _ => "sh tools/tiny-variants/gen-all.sh",
    }
}

/// Read a fixture, announcing a miss once per path.
///
/// Drop-in for `std::fs::read(p).ok()`.
pub fn read(path: &str) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(b) => Some(b),
        Err(_) => {
            let mut seen = announced().lock().unwrap_or_else(|e| e.into_inner());
            if seen.insert(path.to_string()) {
                eprintln!(
                    "SKIPPED: fixture {path} is absent — tests reading it did nothing.\n\
                     \x20        make it:  {}\n\
                     \x20        or all:   sh tools/tiny-variants/gen-all.sh",
                    how_to_make(path)
                );
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_fixture_reads_as_none() {
        assert!(read("/tmp/definitely-not-a-fixture-9e3f.gguf").is_none());
    }

    #[test]
    fn a_present_file_reads_back() {
        let p = std::env::temp_dir().join("testfix-present-probe.bin");
        std::fs::write(&p, b"xyz").unwrap();
        assert_eq!(read(p.to_str().unwrap()).as_deref(), Some(&b"xyz"[..]));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn every_fixture_the_suite_reads_has_an_answer_for_how_to_make_it() {
        // The lookup is a match on file names, so it silently falls through to
        // the generic advice when a NEW fixture appears. Pin the ones that
        // exist: each must get specific guidance, not the catch-all.
        for name in [
            "tiny-qwen3.gguf", "tiny-qwen2.gguf", "tiny-llada2.gguf",
            "tiny-llada2-shift.gguf", "tiny-llada2-64exp.gguf", "tiny-glmdsa.gguf",
            "tiny-qwen3-q8.gguf", "tiny-llada2-q4file.gguf", "tiny-notok.gguf",
        ] {
            let advice = how_to_make(&format!("/tmp/{name}"));
            assert_ne!(advice, "sh tools/tiny-variants/gen-all.sh",
                "{name} fell through to the catch-all — add it to how_to_make");
        }
    }

    /// The generator arg that is easy to omit, pinned.
    ///
    /// Every gen3-family script defaults to the qwen2 shape and only emits the
    /// named architecture when argv[2] asks for it. Omitting it yields the
    /// right filename and the wrong model, which fails eight tests with errors
    /// that read as engine bugs.
    #[test]
    fn the_llada2_variants_carry_their_architecture_argument() {
        for name in ["tiny-llada2.gguf", "tiny-llada2-shift.gguf", "tiny-llada2-64exp.gguf"] {
            let advice = how_to_make(&format!("/tmp/{name}"));
            assert!(advice.ends_with(" llada2"),
                "{name}: advice must pass the architecture argument, got: {advice}");
        }
    }
}
