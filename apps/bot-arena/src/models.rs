//! Model registry — the managed home for proposer models.
//!
//! Two kinds live here, both presented through one graphical selector:
//!   * GGUF models (run via llama.cpp) — dropped into / imported to `models/gguf/`.
//!   * Local tinygpt models — trained from `experience.jsonl`, cached in `models/local/`.
//!
//! "Seamless" means self-contained: the proposer wrapper scripts are embedded in the
//! binary and materialized into the models dir, so every registered model's run command
//! uses stable absolute paths and needs nothing else on disk.

use std::path::{Path, PathBuf};

const PROPOSER_TINYGPT: &str = include_str!("../examples/proposer_tinygpt.py");
const PROPOSER_LLAMACPP: &str = include_str!("../examples/proposer_llamacpp.py");

pub fn models_dir() -> PathBuf {
    let d = crate::registry::data_root().join("models");
    let _ = std::fs::create_dir_all(d.join("gguf"));
    let _ = std::fs::create_dir_all(d.join("local"));
    d
}

/// Write the embedded wrapper scripts into the models dir (idempotent). Returns the
/// paths to the (tinygpt, llamacpp) wrappers.
pub fn materialize_scripts() -> (PathBuf, PathBuf) {
    let d = models_dir();
    let tg = d.join("proposer_tinygpt.py");
    let ll = d.join("proposer_llamacpp.py");
    // overwrite each launch so an upgraded binary ships fixes to the wrappers
    let _ = std::fs::write(&tg, PROPOSER_TINYGPT);
    let _ = std::fs::write(&ll, PROPOSER_LLAMACPP);
    (tg, ll)
}

#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub label: String,   // shown in the selector
    pub kind: &'static str, // "gguf" | "local"
    pub command: String, // ready-to-run proposer command (absolute paths)
}

/// Scan the models dir for everything runnable.
pub fn scan() -> Vec<ModelEntry> {
    let d = models_dir();
    let (tg_script, ll_script) = materialize_scripts();
    let bin = crate::registry::bin_dir();
    let mut out = Vec::new();

    // GGUF files (any .gguf under models/gguf/)
    if let Ok(rd) = std::fs::read_dir(d.join("gguf")) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "gguf").unwrap_or(false)).collect();
        files.sort();
        for p in files {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            out.push(ModelEntry {
                label: format!("GGUF · {name}"),
                kind: "gguf",
                command: format!("LLAMA_MODEL={} python3 {}", p.display(), ll_script.display()),
            });
        }
    }
    // Local tinygpt models (subdirs of models/local/ with model.bin + tok.bin)
    if let Ok(rd) = std::fs::read_dir(d.join("local")) {
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for p in dirs {
            let model = p.join("model.bin");
            let tok = p.join("tok.bin");
            if model.exists() && tok.exists() {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                out.push(ModelEntry {
                    label: format!("local · {name}"),
                    kind: "local",
                    command: format!("TINYGPT_BIN={} TINYGPT_TOK={} TINYGPT_MODEL={} python3 {}",
                        bin.join("tiny_gpt").display(), tok.display(), model.display(), tg_script.display()),
                });
            }
        }
    }
    out
}

/// Import a GGUF file into the registry by copying it into models/gguf/.
pub fn import_gguf(src: &Path) -> std::io::Result<PathBuf> {
    let name = src.file_name().ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidInput, "no filename"))?;
    let dst = models_dir().join("gguf").join(name);
    std::fs::copy(src, &dst)?;
    Ok(dst)
}

/// A curated, freely-downloadable GGUF — a pointer into the Hugging Face ecosystem so the
/// user can pick a known-good coder model without learning repo/quant conventions first.
/// These are pointers, not guarantees: filenames on HF can change, so a "Custom URL" path
/// is always offered alongside.
#[derive(Clone, Debug)]
pub struct CatalogEntry {
    pub label: String, // shown in the picker
    pub repo: String,  // HF repo id
    pub file: String,  // GGUF filename within the repo
    pub note: String,  // size / what it's for
}
impl CatalogEntry {
    pub fn url(&self) -> String {
        format!("https://huggingface.co/{}/resolve/main/{}", self.repo, self.file)
    }
}

/// The baked-in selection of common sources. Biased toward small, openly-downloadable,
/// code-focused models that run on a laptop GPU/CPU — the sweet spot for a tier-1 here.
pub fn catalog() -> Vec<CatalogEntry> {
    let e = |label: &str, repo: &str, file: &str, note: &str| CatalogEntry {
        label: label.into(), repo: repo.into(), file: file.into(), note: note.into() };
    vec![
        e("Qwen2.5-Coder 0.5B Instruct (Q4_K_M)", "Qwen/Qwen2.5-Coder-0.5B-Instruct-GGUF",
          "qwen2.5-coder-0.5b-instruct-q4_k_m.gguf", "~0.4 GB · tiny, fastest, weakest"),
        e("Qwen2.5-Coder 1.5B Instruct (Q4_K_M)", "Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF",
          "qwen2.5-coder-1.5b-instruct-q4_k_m.gguf", "~1.1 GB · good default tier-1"),
        e("Qwen2.5-Coder 3B Instruct (Q4_K_M)", "Qwen/Qwen2.5-Coder-3B-Instruct-GGUF",
          "qwen2.5-coder-3b-instruct-q4_k_m.gguf", "~2.0 GB · stronger, still light"),
        e("Qwen2.5-Coder 7B Instruct (Q4_K_M)", "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
          "qwen2.5-coder-7b-instruct-q4_k_m.gguf", "~4.7 GB · strong, needs more RAM/VRAM"),
        e("DeepSeek-Coder 6.7B Instruct (Q4_K_M)", "TheBloke/deepseek-coder-6.7B-instruct-GGUF",
          "deepseek-coder-6.7b-instruct.Q4_K_M.gguf", "~4.0 GB · alternative coder family"),
        e("Llama 3.2 3B Instruct (Q4_K_M)", "bartowski/Llama-3.2-3B-Instruct-GGUF",
          "Llama-3.2-3B-Instruct-Q4_K_M.gguf", "~2.0 GB · general (not code-specialized)"),
    ]
}

/// Best-effort filename from a URL's last path segment (for the Custom URL path).
pub fn filename_from_url(url: &str) -> String {
    url.split('?').next().unwrap_or(url)
        .rsplit('/').next().filter(|s| !s.is_empty())
        .unwrap_or("model.gguf").to_string()
}

/// Where a download lands while in flight (the GUI polls this for progress).
pub fn download_part_path(filename: &str) -> PathBuf {
    models_dir().join("gguf").join(format!("{filename}.part"))
}

/// Download a GGUF into the registry via curl (no heavy HTTP deps; curl/wget are ubiquitous).
/// Streams to `<file>.part` then renames on success, so a partial download never looks like a
/// real model to the scanner. Blocks — run on a background thread; the GUI polls the .part size.
pub fn download_gguf(url: &str, filename: &str) -> Result<PathBuf, String> {
    let dir = models_dir().join("gguf");
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(filename);
    let part = download_part_path(filename);
    let _ = std::fs::remove_file(&part);
    let curl = std::process::Command::new("curl")
        .args(["-L", "-f", "--retry", "3", "-o"]).arg(&part).arg(url)
        .status();
    let ok = match curl {
        Ok(s) => s.success(),
        Err(_) => { // fall back to wget if curl is missing
            std::process::Command::new("wget").args(["-O"]).arg(&part).arg(url)
                .status().map(|s| s.success()).unwrap_or(false)
        }
    };
    if !ok { let _ = std::fs::remove_file(&part); return Err("download failed (check URL / network / disk)".into()); }
    std::fs::rename(&part, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

/// Delete a cached local model directory (by its registry label or dir name).
pub fn delete_local(name: &str) -> std::io::Result<()> {
    let leaf = name.rsplit(['·', '/', ' ']).next().unwrap_or(name).trim();
    let dir = models_dir().join("local").join(leaf);
    if dir.exists() { std::fs::remove_dir_all(dir) } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such local model"))
    }
}

/// Dry-run a proposer command the way the engine will: feed it a tiny synthetic
/// context on stdin and check it emits a unified diff. Returns the diff's line count
/// on success, or a human-readable reason on failure — so a misconfigured model
/// (wrong path, missing binary, no diff) is caught before a full run, not during it.
pub fn test_proposer(command: &str) -> Result<usize, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if command.trim().is_empty() { return Err("empty command".into()); }
    let ctx = "{\"test_cmd\":\"cargo test\",\"failing\":[\"tests::add_works\"],\
        \"files\":[{\"path\":\"src/lib.rs\",\"content\":\"pub fn add(a: i32, b: i32) -> i32 { a - b }\\n#[cfg(test)]\\nmod tests { #[test] fn add_works(){} }\"}]}";
    let mut child = Command::new("sh").arg("-c").arg(command)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("could not start command: {e}"))?;
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(ctx.as_bytes()); }
    let out = child.wait_with_output().map_err(|e| format!("command failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let hint = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("non-zero exit");
        return Err(format!("command exited with error: {}", hint.trim()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.iter().any(|l| l.starts_with("--- ") || l.starts_with("diff --git")) {
        Ok(lines.len())
    } else if stdout.trim().is_empty() {
        Err("produced no output (model returned nothing for this context)".into())
    } else {
        Err("output is not a unified diff".into())
    }
}

// ===== Local model training (the "our own models" path) =====================

/// Build the tinygpt training corpus from harvested wins. Mirrors
/// examples/experience_to_corpus.py so training needs no external script.
fn build_corpus(exp: &Path, corpus_dir: &Path, repeat: usize) -> std::io::Result<usize> {
    std::fs::create_dir_all(corpus_dir)?;
    let body = std::fs::read_to_string(exp)?;
    let mut blocks = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        // minimal JSON field extraction (the records are written by us)
        let failing = json_array_field(line, "failing_before");
        let diff = json_string_field(line, "diff");
        if let Some(diff) = diff {
            if !diff.is_empty() {
                let mut names: Vec<String> = failing;
                names.sort();
                blocks.push(format!("<<<FIX failing={}>>>\n{}\n<<<END>>>\n",
                    names.join(","), diff.trim_end()));
            }
        }
    }
    if blocks.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no usable fixes in experience log"));
    }
    let n = blocks.len();
    let text = blocks.join("").repeat(repeat.max(1));
    std::fs::write(corpus_dir.join("corpus.md"), text)?;
    Ok(n)
}

/// Train a local tinygpt model from `exp` into a fresh `models/local/<name>/` dir.
/// Runs the pipeline binaries (bpe_train, tiny_gpt) as subprocesses; `log` receives
/// progress lines. Returns the new model dir on success. Intended to run on a
/// background thread (it blocks for ~a minute).
pub fn train_local(exp: &Path, name: &str, steps: usize, mut log: impl FnMut(String)) -> Result<PathBuf, String> {
    let bin = crate::registry::bin_dir();
    let bpe = bin.join("bpe_train");
    let tg = bin.join("tiny_gpt");
    if !bpe.exists() || !tg.exists() {
        return Err(format!("pipeline tools missing at {} — run `make` in pipeline/ first", bin.display()));
    }
    let out = models_dir().join("local").join(name);
    let corpus = out.join("corpus");
    let tok = out.join("tok.bin");
    let model = out.join("model.bin");
    let _ = std::fs::create_dir_all(&out);

    log("building corpus from experience.jsonl …".into());
    let n = build_corpus(exp, &corpus, 60).map_err(|e| e.to_string())?;
    log(format!("corpus: {n} distinct fix-blocks"));

    log("training tokenizer (char-level) …".into());
    run(&bpe, &[corpus.to_str().unwrap(), tok.to_str().unwrap(), "2"], &mut log)?;

    log(format!("training tinygpt ({steps} steps) — this runs in the background …"));
    run(&tg, &["train", corpus.to_str().unwrap(), tok.to_str().unwrap(), model.to_str().unwrap(),
        &steps.to_string(), "48", "2", "4", "160", "4", "1.5e-3"], &mut log)?;

    if model.exists() && tok.exists() {
        log(format!("model cached → {}", out.display()));
        Ok(out)
    } else {
        Err("training finished but produced no model".into())
    }
}

fn run(bin: &Path, args: &[&str], log: &mut impl FnMut(String)) -> Result<(), String> {
    use std::process::Command;
    let o = Command::new(bin).args(args).env("CORPUS_EXT", ".md")
        .output().map_err(|e| format!("{}: {e}", bin.display()))?;
    // surface the last line of stderr (tiny_gpt prints progress there)
    let err = String::from_utf8_lossy(&o.stderr);
    if let Some(last) = err.lines().rev().find(|l| !l.trim().is_empty()) {
        log(last.to_string());
    }
    if o.status.success() { Ok(()) } else {
        Err(format!("{} exited with {}", bin.display(), o.status))
    }
}

// --- tiny JSON field readers (records are produced by log_experience) -------
fn json_string_field(line: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let i = line.find(&pat)? + pat.len();
    let bytes = line.as_bytes();
    let mut j = i;
    let mut s = String::new();
    while j < bytes.len() {
        let c = bytes[j] as char;
        if c == '\\' && j + 1 < bytes.len() {
            let n = bytes[j + 1] as char;
            s.push(match n { 'n' => '\n', 't' => '\t', 'r' => '\r', '"' => '"', '\\' => '\\', other => other });
            j += 2; continue;
        }
        if c == '"' { break; }
        s.push(c); j += 1;
    }
    Some(s)
}

fn json_array_field(line: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\":[");
    let Some(start) = line.find(&pat) else { return Vec::new(); };
    let rest = &line[start + pat.len()..];
    let Some(end) = rest.find(']') else { return Vec::new(); };
    rest[..end].split(',').filter_map(|tok| {
        let t = tok.trim();
        if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
            Some(t[1..t.len() - 1].to_string())
        } else { None }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_scans_gguf_and_local() {
        let tmp = std::env::temp_dir().join(format!("mreg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let d = models_dir();
        let (tg, ll) = materialize_scripts();
        assert!(tg.exists() && ll.exists(), "wrapper scripts materialized");
        std::fs::write(d.join("gguf/foo.gguf"), b"x").unwrap();
        let local = d.join("local/tinygpt-1");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("model.bin"), b"x").unwrap();
        std::fs::write(local.join("tok.bin"), b"x").unwrap();
        let e = scan();
        assert!(e.iter().any(|m| m.kind == "gguf" && m.command.contains("foo.gguf")), "gguf scanned");
        assert!(e.iter().any(|m| m.kind == "local" && m.command.contains("tinygpt-1")), "local scanned");
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn train_local_produces_a_model_when_tools_present() {
        // Uses CODEBOT_ARENA_HOME (set by the caller). Skips cleanly if the pipeline
        // tools aren't built, so the suite never depends on them.
        let bin = crate::registry::bin_dir();
        if !bin.join("tiny_gpt").exists() || !bin.join("bpe_train").exists() {
            eprintln!("skip: pipeline tools not built at {}", bin.display());
            return;
        }
        let exp = crate::registry::data_root().join("experience.jsonl");
        std::fs::write(&exp,
            "{\"target\":\"/r\",\"score\":1.0,\"failing_before\":[\"tests::add_works\"],\"diff\":\"--- a/src/lib.rs\\n+++ b/src/lib.rs\\n@@ -1,2 +1,2 @@\\n-pub fn add(a: i32, b: i32) -> i32 { a - b }\\n+pub fn add(a: i32, b: i32) -> i32 { a + b }\\n #[cfg(test)]\\n\"}\n").unwrap();
        let res = train_local(&exp, "tinygpt-test", 40, |line| eprintln!("  train: {line}"));
        let dir = res.expect("training should succeed with tools present");
        assert!(dir.join("model.bin").exists() && dir.join("tok.bin").exists(), "model cached");
        assert!(scan().iter().any(|m| m.kind == "local" && m.command.contains("tinygpt-test")), "appears in registry");
    }

    #[test]
    fn test_proposer_distinguishes_good_bad_commands() {
        // emits a diff → ok with line count
        let good = "printf -- '--- a/x\\n+++ b/x\\n@@ -1 +1 @@\\n-a\\n+b\\n'";
        assert!(matches!(test_proposer(good), Ok(n) if n >= 4));
        // runs but emits no diff → error
        assert!(test_proposer("echo not a diff").is_err());
        // command fails → error mentions the failure
        assert!(test_proposer("exit 3").is_err());
        // empty → error
        assert!(test_proposer("   ").is_err());
    }

    #[test]
    fn delete_local_removes_cached_model() {
        let tmp = std::env::temp_dir().join(format!("mdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let d = models_dir();
        let m = d.join("local/tinygpt-9");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("model.bin"), b"x").unwrap();
        std::fs::write(m.join("tok.bin"), b"x").unwrap();
        assert!(scan().iter().any(|e| e.command.contains("tinygpt-9")));
        // delete by the registry label form ("local · tinygpt-9")
        delete_local("local · tinygpt-9").unwrap();
        assert!(!m.exists(), "model dir removed");
        assert!(!scan().iter().any(|e| e.command.contains("tinygpt-9")));
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn catalog_entries_have_well_formed_urls() {
        let c = catalog();
        assert!(c.len() >= 4, "several baked-in sources");
        for e in &c {
            let u = e.url();
            assert!(u.starts_with("https://huggingface.co/"), "HF resolve url: {u}");
            assert!(u.ends_with(".gguf"), "points at a gguf: {u}");
        }
        assert_eq!(filename_from_url("https://h/r/resolve/main/foo-Q4.gguf?x=1"), "foo-Q4.gguf");
    }

    #[test]
    fn download_gguf_lands_in_registry() {
        // HF is unreachable in CI/sandbox; exercise the curl plumbing against an allowed
        // domain. Skips (does not fail) if there is no network, so the suite stays green.
        let tmp = std::env::temp_dir().join(format!("dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let url = "https://raw.githubusercontent.com/KhronosGroup/Vulkan-Docs/main/README.adoc";
        match download_gguf(url, "probe.gguf") {
            Ok(p) => {
                assert!(p.exists(), "downloaded file present");
                assert!(scan().iter().any(|m| m.kind == "gguf" && m.command.contains("probe.gguf")),
                        "appears in registry scan");
                assert!(!download_part_path("probe.gguf").exists(), ".part renamed away on success");
            }
            Err(e) => eprintln!("skip: no network ({e})"),
        }
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn corpus_built_from_experience() {
        let tmp = std::env::temp_dir().join(format!("mcorp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let exp = tmp.join("experience.jsonl");
        std::fs::write(&exp,
            "{\"target\":\"/r\",\"score\":1.0,\"failing_before\":[\"t::a\"],\"diff\":\"--- a/x\\n+++ b/x\\n-1\\n+2\\n\"}\n").unwrap();
        let corpus = tmp.join("corpus");
        let n = build_corpus(&exp, &corpus, 3).unwrap();
        assert_eq!(n, 1, "one fix-block");
        let text = std::fs::read_to_string(corpus.join("corpus.md")).unwrap();
        assert!(text.contains("<<<FIX failing=t::a>>>"), "header reconstructed");
        assert!(text.contains("+2"), "diff body present");
        assert_eq!(text.matches("<<<END>>>").count(), 3, "repeated x3 for overfit");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
