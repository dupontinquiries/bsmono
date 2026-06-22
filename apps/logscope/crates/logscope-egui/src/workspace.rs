//! workspace.rs — turn a model reply into real files. Parses fenced blocks (and unified
//! diffs) into artifacts, materializes them into a temp workspace, and bundles them into a
//! .zip — all dependency-free (a tiny store-only zip writer + CRC32, no `zip` crate).
//!
//! Path safety: model-supplied paths are sanitized; absolute paths and `..` traversal are
//! rejected so a reply can never write outside its workspace.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { File, Patch }

#[derive(Clone, Debug)]
pub struct Artifact { pub path: String, pub content: String, pub kind: Kind, pub fragments: usize }

fn lang_ext(info: &str) -> &str {
    match info {
        "rust" | "rs" => "rs", "python" | "py" => "py", "javascript" | "js" => "js",
        "typescript" | "ts" => "ts", "toml" => "toml", "json" => "json", "yaml" | "yml" => "yaml",
        "bash" | "sh" | "shell" => "sh", "c" => "c", "cpp" | "c++" => "cpp", "go" => "go",
        "html" => "html", "css" => "css", "md" | "markdown" => "md", "sql" => "sql",
        _ => "txt",
    }
}

/// First whitespace token that looks like a path (has a `/` or a `.ext`); else None.
fn path_in(info: &str) -> Option<String> {
    info.split_whitespace().find(|t| t.contains('/') || (t.contains('.') && !t.starts_with('.')))
        .map(|s| s.to_string())
}

/// Reject absolute paths and `..` traversal; normalize to a safe relative path.
pub fn safe_rel(path: &str) -> Option<String> {
    let p = Path::new(path);
    if p.is_absolute() { return None; }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            Normal(c) => clean.push(c),
            CurDir => {}
            ParentDir | RootDir | Prefix(_) => return None,
        }
    }
    let s = clean.to_string_lossy().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Extract file/patch artifacts from a model reply. Recognizes:
///   * a ```` ```lang path/to/file.ext ```` info string (path anywhere in the info)
///   * a `// file: path` or `# file: path` hint line directly before a fence
///   * ```` ```diff ```` / ```` ```patch ```` or a body that looks like a unified diff → Patch
/// Anonymous blocks get a generated name from the language hint.
pub fn parse_artifacts(text: &str) -> Vec<Artifact> {
    let lines: Vec<&str> = text.lines().collect();
    let mut arts: Vec<Artifact> = Vec::new();
    let mut pending: Option<String> = None;
    // The previous block's (path, ext). An anonymous block of the same language is treated as a
    // CONTINUATION of it — this is what stitches a file split across a stop/start (warm-resume)
    // boundary, where the model reopens a ```lang fence mid-file and would otherwise yield two
    // "file1.rs"/"file2.rs" fragments instead of one whole file.
    let mut prev: Option<(String, String)> = None;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(p) = t.strip_prefix("// file:").or_else(|| t.strip_prefix("# file:")) {
            pending = safe_rel(p.trim());
            i += 1;
            continue;
        }
        if let Some(info) = t.strip_prefix("```") {
            let info = info.trim().to_string();
            let mut body = String::new();
            i += 1;
            // Consume body until the closing fence. A NEW opening fence (```lang) before any close
            // means the block was cut open at a stop/start and the model reopened it — stop here and
            // let the outer loop re-process that fence as the next (continuation) block.
            while i < lines.len() {
                let li = lines[i].trim();
                if li == "```" { i += 1; break; }            // normal close — consume it
                if li.starts_with("```") { break; }          // reopened fence — leave it for next block
                body.push_str(lines[i]);
                body.push('\n');
                i += 1;
            }
            let looks_diff = matches!(info.as_str(), "diff" | "patch")
                || body.starts_with("--- ") || body.contains("\n@@ ") || body.contains("\n@@");
            let kind = if looks_diff { Kind::Patch } else { Kind::File };
            let lang = lang_ext(&info).to_string(); // extension implied by the language tag
            // Path resolution, in order of confidence:
            //   explicit info path → `// file:` hint → continuation of the previous same-extension file
            //   → a stable per-language name (so repeated anonymous blocks of one language coalesce).
            let path = path_in(&info).and_then(|p| safe_rel(&p))
                .or_else(|| pending.take())
                .or_else(|| match (&prev, kind) {
                    (Some((pp, pe)), Kind::File) if *pe == lang => Some(pp.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| match kind {
                    Kind::Patch => format!("changes{}.patch", arts.len() + 1),
                    Kind::File => format!("snippet.{lang}"),
                });
            if !body.trim().is_empty() {
                let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                // Would this block coalesce into an existing artifact at the same path? Only do so if
                // it's an ADDITIVE continuation (a file cut across a stop/start, or new items). If the
                // new block REDEFINES a top-level item already present (e.g. a second complete `fn
                // main`), it's an alternative version, not a continuation — keep it as its own file so
                // we never concatenate two programs into one broken file.
                let existing = arts.iter().position(|a| a.path == path);
                let redefines = existing.map_or(false, |idx| redefines_item(&arts[idx].content, &body));
                if let (Some(idx), false) = (existing, redefines) {
                    let a = &mut arts[idx];
                    if !a.content.ends_with('\n') { a.content.push('\n'); }
                    a.content.push_str(&body);
                    a.fragments += 1;
                    prev = Some((path.clone(), ext));
                } else {
                    let path = if existing.is_some() { unique_path(&arts, &path) } else { path };
                    let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                    prev = Some((path.clone(), ext));
                    arts.push(Artifact { path, content: body, kind, fragments: 1 });
                }
            }
            continue;
        }
        i += 1;
    }
    arts
}

/// True if `new_body` defines a top-level item whose (kind, name) already appears in `existing` — the
/// signal that the new block is an alternative version, not an additive continuation. Rust-aware
/// (reuses the merge scanner); for languages it doesn't parse it finds no items → treated as additive.
fn redefines_item(existing: &str, new_body: &str) -> bool {
    let old = crate::merge::scan_items(existing);
    if old.is_empty() { return false; }
    let new = crate::merge::scan_items(new_body);
    new.iter().any(|n| !n.name.is_empty() && old.iter().any(|o| o.kind == n.kind && o.name == n.name))
}

/// A path not already used by `arts`, by inserting `-2`, `-3`, … before the extension.
fn unique_path(arts: &[Artifact], base: &str) -> String {
    if !arts.iter().any(|a| a.path == base) { return base.to_string(); }
    let p = Path::new(base);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("snippet");
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).map(|d| format!("{}/", d.display())).unwrap_or_default();
    for n in 2.. {
        let cand = format!("{parent}{stem}-{n}{ext}");
        if !arts.iter().any(|a| a.path == cand) { return cand; }
    }
    unreachable!()
}
pub fn write_workspace(arts: &[Artifact]) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("logscope-ws-{stamp:x}"));
    std::fs::create_dir_all(&dir)?;
    write_to_dir(arts, &dir)?;
    Ok(dir)
}

/// Write artifacts into an existing (persistent) directory; returns how many were written.
/// Empty-content artifacts are skipped so the model can never leave behind stub files.
pub fn write_to_dir(arts: &[Artifact], dir: &std::path::Path) -> std::io::Result<usize> {
    let mut written = 0;
    for a in arts {
        if a.content.trim().is_empty() { continue; }      // never write empty files
        if let Some(rel) = safe_rel(&a.path) {
            let full = dir.join(rel);
            if let Some(parent) = full.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(&full, a.content.as_bytes())?;
            written += 1;
        }
    }
    Ok(written)
}

// ── dependency-free store-only ZIP writer ────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Build a valid (uncompressed/"stored") ZIP from `(name, bytes)` entries. Readable by any
/// standard unzip; no compression keeps it dependency-free and is fine for source bundles.
pub fn zip_store(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut offsets = Vec::new();
    for (name, data) in files {
        offsets.push(out.len() as u32);
        let crc = crc32(data);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // local file header sig
        out.extend_from_slice(&20u16.to_le_bytes());          // version needed
        out.extend_from_slice(&0u16.to_le_bytes());           // flags
        out.extend_from_slice(&0u16.to_le_bytes());           // method = store
        out.extend_from_slice(&0u16.to_le_bytes());           // mod time
        out.extend_from_slice(&0x21u16.to_le_bytes());        // mod date (1980-01-01)
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());           // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
    }
    let cd_start = out.len() as u32;
    for (i, (name, data)) in files.iter().enumerate() {
        let crc = crc32(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // central dir header sig
        central.extend_from_slice(&20u16.to_le_bytes());          // version made by
        central.extend_from_slice(&20u16.to_le_bytes());          // version needed
        central.extend_from_slice(&0u16.to_le_bytes());           // flags
        central.extend_from_slice(&0u16.to_le_bytes());           // method
        central.extend_from_slice(&0u16.to_le_bytes());           // time
        central.extend_from_slice(&0x21u16.to_le_bytes());        // date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());           // extra
        central.extend_from_slice(&0u16.to_le_bytes());           // comment
        central.extend_from_slice(&0u16.to_le_bytes());           // disk no
        central.extend_from_slice(&0u16.to_le_bytes());           // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes());           // external attrs
        central.extend_from_slice(&offsets[i].to_le_bytes());     // local header offset
        central.extend_from_slice(name.as_bytes());
    }
    let cd_len = central.len() as u32;
    let n = files.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // end of central directory
    out.extend_from_slice(&0u16.to_le_bytes());           // disk
    out.extend_from_slice(&0u16.to_le_bytes());           // cd start disk
    out.extend_from_slice(&n.to_le_bytes());              // entries this disk
    out.extend_from_slice(&n.to_le_bytes());              // total entries
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());           // comment len
    out
}

/// Bundle artifacts into a zip written next to the workspace; returns the zip path.
pub fn zip_artifacts(arts: &[Artifact], dir: &Path) -> std::io::Result<PathBuf> {
    let files: Vec<(String, Vec<u8>)> = arts.iter()
        .filter_map(|a| safe_rel(&a.path).map(|p| (p, a.content.clone().into_bytes())))
        .collect();
    let zip = dir.join("bundle.zip");
    std::fs::write(&zip, zip_store(&files))?;
    Ok(zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_block_and_patch() {
        let txt = "Here:\n```rust src/lib.rs\npub fn x() {}\n```\nand a patch:\n```diff\n--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n```";
        let arts = parse_artifacts(txt);
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].path, "src/lib.rs");
        assert_eq!(arts[0].kind, Kind::File);
        assert!(arts[0].content.contains("pub fn x()"));
        assert_eq!(arts[1].kind, Kind::Patch);
    }

    #[test]
    fn file_hint_and_anonymous_naming() {
        let arts = parse_artifacts("// file: a/b.toml\n```toml\nk = 1\n```\n```python\nprint(1)\n```");
        assert_eq!(arts[0].path, "a/b.toml");
        assert_eq!(arts[1].path, "snippet.py"); // anonymous, different language -> its own file
        assert_eq!(arts.len(), 2);
    }

    #[test]
    fn stitches_reopened_fence_across_stopstart() {
        // A file cut mid-fence at a stop, then the model reopens ```rust on continue.
        let txt = "```rust\nfn main() {\n```rust\n    let x = 1;\n}\n```";
        let arts = parse_artifacts(txt);
        assert_eq!(arts.len(), 1, "the two halves should merge into one file");
        assert_eq!(arts[0].fragments, 2);
        assert!(arts[0].content.contains("fn main()") && arts[0].content.contains("let x = 1"));
    }

    #[test]
    fn anonymous_continuation_joins_named_file() {
        // main.rs, then an anonymous rust block on continue -> same file, not a new fragment.
        let txt = "```rust main.rs\nfn a() {}\n```\nmore:\n```rust\nfn b() {}\n```";
        let arts = parse_artifacts(txt);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].path, "main.rs");
        assert_eq!(arts[0].fragments, 2);
        assert!(arts[0].content.contains("fn a()") && arts[0].content.contains("fn b()"));
    }

    #[test]
    fn distinct_named_files_stay_separate() {
        let txt = "```rust src/a.rs\nfn a() {}\n```\n```rust src/b.rs\nfn b() {}\n```";
        let arts = parse_artifacts(txt);
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].fragments, 1);
        assert_eq!(arts[1].fragments, 1);
    }

    #[test]
    fn two_complete_versions_dont_concatenate() {
        // The reported bug: a reply with two complete anonymous programs (both define `fn main`) was
        // joined into ONE file with two `fn main`. They must stay separate.
        let txt = "first:\n```rust\nfn main() { let a = 1; }\n```\nrevised:\n```rust\nfn main() { let b = 2; }\n```";
        let arts = parse_artifacts(txt);
        assert_eq!(arts.len(), 2, "two complete programs must be two files, not one");
        assert_eq!(arts.iter().filter(|a| a.content.matches("fn main").count() > 1).count(), 0,
            "no single artifact may contain two `fn main`");
        assert!(arts[0].path != arts[1].path, "the second gets a unique name: {:?}", arts.iter().map(|a| &a.path).collect::<Vec<_>>());
    }

    #[test]
    fn rejects_path_traversal() {
        assert_eq!(safe_rel("../etc/passwd"), None);
        assert_eq!(safe_rel("/etc/passwd"), None);
        assert_eq!(safe_rel("src/./main.rs").as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn zip_is_valid_per_system_unzip() {
        let files = vec![("a.txt".to_string(), b"hello".to_vec()),
                         ("dir/b.txt".to_string(), b"world!".to_vec())];
        let bytes = zip_store(&files);
        assert_eq!(&bytes[..4], &0x0403_4b50u32.to_le_bytes()); // PK\x03\x04
        let path = std::env::temp_dir().join("logscope_zip_selftest.zip");
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(o) = std::process::Command::new("unzip").arg("-t").arg(&path).output() {
            assert!(o.status.success(), "unzip -t failed: {}", String::from_utf8_lossy(&o.stderr));
        }
    }
}
