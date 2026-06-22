//! v0.1.367: Gemini protocol support — archive viewer (tracer
//! bullet).
//!
//! This module handles Gemtext parsing, URL canonicalization,
//! and the local archive layout. It does NOT yet fetch over the
//! network — that ships in a follow-up once rustls is wired in.
//!
//! Architecture:
//!
//!   - `parse_gemtext` turns a raw .gmi document into a
//!     `Vec<GemtextLine>` (text, heading, link, list-item,
//!     preformatted block, quote).
//!   - `gemtext_to_markdown` re-renders into the markdown flavor
//!     web2qml's existing manpage renderer already speaks. Links
//!     come out as `[label](gemini://host/path)` and route
//!     through the existing nav pipeline.
//!   - `archive_path_for` maps a Gemini URL to a local path
//!     under `~/.config/web2qml/gemini/`. Idempotent — visiting
//!     the same URL twice writes the same file.
//!   - `load_from_archive` reads a Gemtext file off disk for
//!     rendering.
//!
//! Why an archive-first design: Gemini content is small (typical
//! capsules are < 10KB), updates infrequently, and is the kind
//! of thing power users like to keep offline. Storing every
//! visited capsule locally lets the user re-read without
//! reconnecting (or after a capsule goes dark — a real concern
//! for hobbyist-hosted content).
//!
//! Why a follow-up for live fetch: Gemini requires TLS with a
//! TOFU (Trust On First Use) trust model — system CA roots don't
//! work because most capsules ship self-signed certs. The
//! cleanest impl uses `rustls` with a custom verifier; that's a
//! larger change worth its own ship after this one proves the
//! rendering pipeline works.

use std::path::PathBuf;

/// One parsed line of Gemtext content. Order matters; lines are
/// rendered in source order.
///
/// The spec defines six line types:
///   - Plain text (default)
///   - Link `=> url [label]`
///   - Heading `# / ## / ###` (3 levels)
///   - List item `* item`
///   - Quote `> quoted`
///   - Preformatted toggle ` ``` ` (state-based)
///
/// We keep preformatted blocks as a single `Preformatted` variant
/// carrying the full body, since the renderer doesn't care about
/// inner structure.
#[derive(Clone, Debug, PartialEq)]
pub enum GemtextLine {
    Text(String),
    Heading { level: u8, text: String },
    Link { url: String, label: String },
    ListItem(String),
    Quote(String),
    /// `alt` is the optional alt-text on the opening ``` fence;
    /// `body` is everything until the closing fence, newline-
    /// joined.
    Preformatted { alt: String, body: String },
}

/// Parse a Gemtext document into structured lines.
///
/// Gemtext is line-oriented; each line stands alone (except
/// preformatted blocks, which span fences). Order is preserved.
pub fn parse_gemtext(input: &str) -> Vec<GemtextLine> {
    let mut out = Vec::new();
    let mut in_pre = false;
    let mut pre_alt = String::new();
    let mut pre_body: Vec<String> = Vec::new();

    for line in input.lines() {
        // Preformatted toggle: ``` starts or ends a block.
        if line.starts_with("```") {
            if in_pre {
                // Close the block.
                out.push(GemtextLine::Preformatted {
                    alt: std::mem::take(&mut pre_alt),
                    body: pre_body.join("\n"),
                });
                pre_body.clear();
                in_pre = false;
            } else {
                // Open. Alt-text is everything after the fence.
                pre_alt = line[3..].trim().to_string();
                in_pre = true;
            }
            continue;
        }
        if in_pre {
            pre_body.push(line.to_string());
            continue;
        }
        // Outside preformatted: classify by prefix.
        if let Some(rest) = line.strip_prefix("=>") {
            // Link line. Spec: `=> URL [label]`. URL is
            // whitespace-terminated; label is everything after.
            let rest = rest.trim_start();
            let mut parts = rest.splitn(2, char::is_whitespace);
            let url = parts.next().unwrap_or("").trim().to_string();
            let label = parts.next().unwrap_or("").trim().to_string();
            // Spec: when label is empty, render the URL itself
            // as the link text.
            let label_final = if label.is_empty() { url.clone() } else { label };
            out.push(GemtextLine::Link { url, label: label_final });
            continue;
        }
        // Headings: ###, ##, # (check most-specific first so
        // ## doesn't false-match #).
        if let Some(rest) = line.strip_prefix("###") {
            out.push(GemtextLine::Heading { level: 3, text: rest.trim().to_string() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("##") {
            out.push(GemtextLine::Heading { level: 2, text: rest.trim().to_string() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("#") {
            out.push(GemtextLine::Heading { level: 1, text: rest.trim().to_string() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("* ") {
            out.push(GemtextLine::ListItem(rest.trim().to_string()));
            continue;
        }
        if let Some(rest) = line.strip_prefix(">") {
            out.push(GemtextLine::Quote(rest.trim().to_string()));
            continue;
        }
        // Plain text — preserves blank lines for paragraph
        // separation.
        out.push(GemtextLine::Text(line.to_string()));
    }
    // Unclosed preformatted block at EOF: still emit it so the
    // user sees their content. Hostile / malformed input
    // protection.
    if in_pre {
        out.push(GemtextLine::Preformatted {
            alt: pre_alt,
            body: pre_body.join("\n"),
        });
    }
    out
}

/// Render a parsed Gemtext document as markdown for the existing
/// manpage renderer. Links use markdown link syntax so they get
/// rendered as tappable through web2qml's existing pipeline.
///
/// One quirk: Gemtext doesn't have inline links — every link is
/// its own line. We render each `Link` as a paragraph containing
/// just `[label](url)` so it stays on its own visual line.
pub fn gemtext_to_markdown(lines: &[GemtextLine]) -> String {
    let mut out = String::with_capacity(lines.len() * 64);
    for line in lines {
        match line {
            GemtextLine::Text(t) => {
                if t.trim().is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(t);
                    out.push('\n');
                }
            }
            GemtextLine::Heading { level, text } => {
                for _ in 0..*level { out.push('#'); }
                out.push(' ');
                out.push_str(text);
                out.push_str("\n\n");
            }
            GemtextLine::Link { url, label } => {
                out.push_str(&format!("[{}]({})\n\n", label, url));
            }
            GemtextLine::ListItem(text) => {
                out.push_str("- ");
                out.push_str(text);
                out.push('\n');
            }
            GemtextLine::Quote(text) => {
                out.push_str("> ");
                out.push_str(text);
                out.push('\n');
            }
            GemtextLine::Preformatted { alt: _, body } => {
                out.push_str("```\n");
                out.push_str(body);
                out.push_str("\n```\n\n");
            }
        }
    }
    out
}

/// Compute the local archive path for a Gemini URL.
///
/// Layout: `<root>/<host>/<path>.gmi`
///
/// Examples:
///   gemini://geminiprotocol.net/  →  geminiprotocol.net/index.gmi
///   gemini://example.org/notes/   →  example.org/notes/index.gmi
///   gemini://example.org/page     →  example.org/page.gmi
///
/// `root` is typically `~/.config/web2qml/gemini/`. We make it
/// caller-supplied so tests can use a /tmp path.
///
/// Returns None for malformed URLs (no host, etc.).
pub fn archive_path_for(url: &str, root: &PathBuf) -> Option<PathBuf> {
    let rest = url.strip_prefix("gemini://")?;
    // Split host from path.
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    if host.is_empty() { return None; }
    // Sanitize host (no traversal, no special chars).
    if host.contains("..") || host.contains('/') || host.contains('\\') {
        return None;
    }
    let mut p = root.clone();
    p.push(host);
    // Path handling: split into segments, sanitize each.
    if path.is_empty() || path.ends_with('/') {
        // Directory-like URL: append index.gmi.
        for seg in path.trim_end_matches('/').split('/').filter(|s| !s.is_empty()) {
            if seg.contains("..") { return None; }
            p.push(seg);
        }
        p.push("index.gmi");
    } else {
        // File-like URL.
        let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let last = segs.pop().unwrap_or("");
        if last.contains("..") { return None; }
        for seg in &segs {
            if seg.contains("..") { return None; }
            p.push(seg);
        }
        // Append .gmi if the last segment doesn't already have an
        // extension — preserves images / data files at their
        // native extension when caller stores them as-is.
        if last.contains('.') {
            p.push(last);
        } else {
            p.push(format!("{}.gmi", last));
        }
    }
    Some(p)
}

/// Load Gemtext from the local archive. Returns None if the URL
/// has no archive entry yet (user hasn't fetched it). Caller
/// renders a "not in archive" page in that case (or in a future
/// ship: triggers a live fetch).
pub fn load_from_archive(url: &str, root: &PathBuf) -> Option<String> {
    let path = archive_path_for(url, root)?;
    std::fs::read_to_string(&path).ok()
}

/// Save Gemtext to the local archive. Used by future live-fetch
/// path; for now also usable by manual import or tests.
pub fn save_to_archive(url: &str, body: &str, root: &PathBuf) -> std::io::Result<()> {
    let path = archive_path_for(url, root).ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("malformed gemini URL: {}", url),
    ))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, body)
}

/// List capsules archived locally. Returns sorted (host, count)
/// tuples — used by the launcher page to show "what you've been
/// reading."
pub fn list_archived_hosts(root: &PathBuf) -> Vec<(String, u32)> {
    let mut hosts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // Count .gmi files under this host directory.
                let count = count_gmi_files(&path);
                if count > 0 {
                    hosts.insert(name.to_string(), count);
                }
            }
        }
    }
    hosts.into_iter().collect()
}

fn count_gmi_files(dir: &std::path::Path) -> u32 {
    let mut n = 0u32;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("gmi") {
                n += 1;
            }
        } else if path.is_dir() {
            n += count_gmi_files(&path);
        }
    }
    n
}

/// Default archive root: `~/.config/web2qml/gemini/`.
pub fn default_archive_root() -> PathBuf {
    let mut p = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => PathBuf::from("/tmp"),
    };
    p.push(".config");
    p.push("web2qml");
    p.push("gemini");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Render the launcher page — `web2qml://gemini`. Lists archived
/// capsules + a small curated set of well-known capsules to
/// bootstrap from. The curated list is hardcoded for now; future
/// ship may pull from a manifest.
pub fn render_launcher(root: &PathBuf) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("# Gemini\n\n");
    out.push_str("Small-web reading. Gemtext capsules render through ");
    out.push_str("web2qml's normal page pipeline. Tap any link to navigate.\n\n");
    out.push_str("**Status**: archive-only (v0.1.367). Capsules ");
    out.push_str("must be pre-downloaded via an external Gemini ");
    out.push_str("client (e.g. `gemget`, `amfora`) into the archive ");
    out.push_str("directory. Live fetch ships in a follow-up.\n\n");

    out.push_str("## Your archived capsules\n\n");
    let archived = list_archived_hosts(root);
    if archived.is_empty() {
        out.push_str("_No capsules in your archive yet._\n\n");
        out.push_str("To populate: install a Gemini client and download some\n");
        out.push_str("capsules into `~/.config/web2qml/gemini/<host>/`. For\n");
        out.push_str("example with [gemget](https://github.com/makew0rld/gemget):\n\n");
        out.push_str("```\n");
        out.push_str("cd ~/.config/web2qml/gemini\n");
        out.push_str("mkdir -p geminiprotocol.net && cd geminiprotocol.net\n");
        out.push_str("gemget -o index.gmi gemini://geminiprotocol.net/\n");
        out.push_str("```\n\n");
    } else {
        for (host, count) in &archived {
            out.push_str(&format!(
                "- [{}](gemini://{}/) — {} page{}\n",
                host, host, count, if *count == 1 { "" } else { "s" }
            ));
        }
        out.push_str("\n");
    }

    out.push_str("## Starting points\n\n");
    out.push_str("Well-known capsules worth exploring. Tap to fetch ");
    out.push_str("(needs a populated archive in v0.1.367):\n\n");
    out.push_str("- [Project Gemini](gemini://geminiprotocol.net/) — the protocol's home\n");
    out.push_str("- [gemini.circumlunar.space](gemini://gemini.circumlunar.space/) — tilde-style community\n");
    out.push_str("- [Bortzmeyer's blog](gemini://gemini.bortzmeyer.org/) — networking + tech\n");
    out.push_str("- [Solderpunk's diary](gemini://gemini.circumlunar.space/~solderpunk/) — Gemini's co-creator\n");
    out.push_str("- [SDF](gemini://sdf.org/) — long-running shell collective\n");
    out.push_str("- [tilde.team](gemini://tilde.team/) — modern tilde community\n");
    out.push_str("\n");

    out.push_str("## About this surface\n\n");
    out.push_str("Gemini is a small, text-first internet protocol designed ");
    out.push_str("as a deliberate alternative to the modern web. No JavaScript, ");
    out.push_str("no CSS, no cookies — just structured text with links. The ");
    out.push_str("content tends toward thoughtful, hand-written writing from ");
    out.push_str("individuals rather than corporate publishers.\n\n");
    out.push_str("web2qml supports Gemini natively because the protocol's ");
    out.push_str("content shape (markdown-like text + simple links) matches ");
    out.push_str("our rendering pipeline almost exactly. No JS engine needed.\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_text() {
        let r = parse_gemtext("hello\nworld\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], GemtextLine::Text("hello".into()));
        assert_eq!(r[1], GemtextLine::Text("world".into()));
    }

    #[test]
    fn parse_link_with_label() {
        let r = parse_gemtext("=> gemini://example.org/ Example Capsule\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], GemtextLine::Link {
            url: "gemini://example.org/".into(),
            label: "Example Capsule".into(),
        });
    }

    #[test]
    fn parse_link_without_label_uses_url() {
        let r = parse_gemtext("=> gemini://example.org/\n");
        assert_eq!(r[0], GemtextLine::Link {
            url: "gemini://example.org/".into(),
            label: "gemini://example.org/".into(),
        });
    }

    #[test]
    fn parse_link_with_tabs() {
        // Spec allows tabs or spaces between => and URL.
        let r = parse_gemtext("=>\tgemini://x.org/\tLabel\n");
        assert_eq!(r[0], GemtextLine::Link {
            url: "gemini://x.org/".into(),
            label: "Label".into(),
        });
    }

    #[test]
    fn parse_headings() {
        let r = parse_gemtext("# H1\n## H2\n### H3\n");
        assert_eq!(r[0], GemtextLine::Heading { level: 1, text: "H1".into() });
        assert_eq!(r[1], GemtextLine::Heading { level: 2, text: "H2".into() });
        assert_eq!(r[2], GemtextLine::Heading { level: 3, text: "H3".into() });
    }

    #[test]
    fn parse_heading_precedence_three_before_two() {
        // ### must be detected before ## else we'd misparse.
        let r = parse_gemtext("### three\n");
        assert_eq!(r[0], GemtextLine::Heading { level: 3, text: "three".into() });
    }

    #[test]
    fn parse_list_items() {
        let r = parse_gemtext("* one\n* two\n* three\n");
        assert_eq!(r[0], GemtextLine::ListItem("one".into()));
        assert_eq!(r[1], GemtextLine::ListItem("two".into()));
        assert_eq!(r[2], GemtextLine::ListItem("three".into()));
    }

    #[test]
    fn parse_quote() {
        let r = parse_gemtext("> quoted text here\n");
        assert_eq!(r[0], GemtextLine::Quote("quoted text here".into()));
    }

    #[test]
    fn parse_preformatted_block() {
        let r = parse_gemtext("```rust\nlet x = 1;\nlet y = 2;\n```\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], GemtextLine::Preformatted {
            alt: "rust".into(),
            body: "let x = 1;\nlet y = 2;".into(),
        });
    }

    #[test]
    fn parse_preformatted_unclosed_at_eof_still_emits() {
        // Defensive: malformed input shouldn't lose content.
        let r = parse_gemtext("```\nhello\nworld\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], GemtextLine::Preformatted {
            alt: "".into(),
            body: "hello\nworld".into(),
        });
    }

    #[test]
    fn render_markdown_with_links_inline() {
        let lines = parse_gemtext(
            "# Hello\n\nWelcome.\n=> gemini://example.org/ Visit Example\n\nDone.\n"
        );
        let md = gemtext_to_markdown(&lines);
        assert!(md.contains("# Hello"));
        assert!(md.contains("[Visit Example](gemini://example.org/)"));
        assert!(md.contains("Welcome."));
        assert!(md.contains("Done."));
    }

    #[test]
    fn render_markdown_list_items() {
        let lines = parse_gemtext("* one\n* two\n");
        let md = gemtext_to_markdown(&lines);
        assert!(md.contains("- one"));
        assert!(md.contains("- two"));
    }

    #[test]
    fn archive_path_root_url() {
        let root = PathBuf::from("/tmp/test_root");
        let p = archive_path_for("gemini://example.org/", &root).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/test_root/example.org/index.gmi"));
    }

    #[test]
    fn archive_path_no_trailing_slash() {
        // gemini://example.org with no path defaults to root.
        let root = PathBuf::from("/tmp/test_root");
        let p = archive_path_for("gemini://example.org", &root).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/test_root/example.org/index.gmi"));
    }

    #[test]
    fn archive_path_subdir_with_trailing_slash() {
        let root = PathBuf::from("/tmp/r");
        let p = archive_path_for("gemini://example.org/notes/", &root).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/r/example.org/notes/index.gmi"));
    }

    #[test]
    fn archive_path_file_no_extension_adds_gmi() {
        let root = PathBuf::from("/tmp/r");
        let p = archive_path_for("gemini://example.org/page", &root).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/r/example.org/page.gmi"));
    }

    #[test]
    fn archive_path_file_with_extension_preserved() {
        // Images, archives, etc. keep their native extension.
        let root = PathBuf::from("/tmp/r");
        let p = archive_path_for("gemini://example.org/photo.png", &root).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/r/example.org/photo.png"));
    }

    #[test]
    fn archive_path_rejects_traversal() {
        let root = PathBuf::from("/tmp/r");
        assert!(archive_path_for("gemini://example.org/../etc/passwd", &root).is_none());
        assert!(archive_path_for("gemini://../bad/", &root).is_none());
    }

    #[test]
    fn archive_path_rejects_non_gemini() {
        let root = PathBuf::from("/tmp/r");
        assert!(archive_path_for("https://example.org/", &root).is_none());
        assert!(archive_path_for("gopher://example.org/", &root).is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        // Use a unique tmp dir so parallel test runs don't collide.
        let root = PathBuf::from("/tmp/web2qml_gemini_test_a");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let url = "gemini://test.local/note";
        let body = "# Test\n\nHello, world.\n";
        save_to_archive(url, body, &root).unwrap();
        let loaded = load_from_archive(url, &root).unwrap();
        assert_eq!(loaded, body);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_returns_none_for_unarchived() {
        let root = PathBuf::from("/tmp/web2qml_gemini_test_b");
        let _ = std::fs::remove_dir_all(&root);
        assert!(load_from_archive("gemini://nope.local/", &root).is_none());
    }

    #[test]
    fn list_archived_hosts_counts_files() {
        let root = PathBuf::from("/tmp/web2qml_gemini_test_c");
        let _ = std::fs::remove_dir_all(&root);
        save_to_archive("gemini://a.org/", "x", &root).unwrap();
        save_to_archive("gemini://a.org/p2", "y", &root).unwrap();
        save_to_archive("gemini://b.org/", "z", &root).unwrap();
        let hosts = list_archived_hosts(&root);
        assert_eq!(hosts.len(), 2);
        // Sorted alphabetically by host.
        assert_eq!(hosts[0], ("a.org".to_string(), 2));
        assert_eq!(hosts[1], ("b.org".to_string(), 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn render_launcher_empty_archive_shows_hint() {
        let root = PathBuf::from("/tmp/web2qml_gemini_test_d");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let md = render_launcher(&root);
        assert!(md.contains("No capsules in your archive"));
        assert!(md.contains("gemget"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn render_launcher_lists_archived() {
        let root = PathBuf::from("/tmp/web2qml_gemini_test_e");
        let _ = std::fs::remove_dir_all(&root);
        save_to_archive("gemini://example.org/", "hi", &root).unwrap();
        let md = render_launcher(&root);
        assert!(md.contains("example.org"));
        assert!(md.contains("1 page"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
