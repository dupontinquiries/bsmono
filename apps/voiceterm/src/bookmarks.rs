//! v0.1.386 (bookmarks): saved-URL management. Backed by a
//! flat TOML-ish file at `~/.config/web2qml/bookmarks.toml`
//! with one entry per bookmark:
//!
//! ```toml
//! [[bookmark]]
//! url = "https://example.com/article"
//! title = "Example article"
//! saved_at_ms = 1700000000000
//! tags = ["news", "tech"]
//! ```
//!
//! Append-only at the write path (each save is one new
//! `[[bookmark]]` block). Read path parses the whole file
//! into a Vec. No dedup at write time — duplicate saves
//! produce duplicate entries (intentional: the user can
//! save the same page twice with different tags or just
//! re-save to mark it as "still relevant").
//!
//! Future work: tag-based filtering, dedup-by-URL with merge,
//! sync with archive entries, voice intents for tagged
//! bookmark queries ("show news bookmarks").

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub saved_at_ms: u64,
    pub tags: Vec<String>,
}

/// Path to the bookmarks file. Mirrors `palette_dir` in
/// main.rs — `HOME/.config/web2qml/bookmarks.toml`, with
/// `/tmp` fallback if HOME isn't set. v0.1.393: was using
/// the `dirs` crate which isn't a project dep; switched to
/// the same env-based pattern the rest of the codebase
/// uses.
fn bookmarks_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = PathBuf::from(home).join(".config/web2qml");
    let _ = fs::create_dir_all(&dir);
    dir.join("bookmarks.toml")
}

/// Append a bookmark to the file. Creates the file if it
/// doesn't exist. Returns Ok(()) on success.
/// v0.1.388: result of a save_bookmark call — signals
/// whether a fresh entry was created or an existing one
/// was updated. Surface for the toast: "Bookmarked X" vs
/// "Updated tags on existing bookmark".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveResult {
    /// URL wasn't in the file; a new entry was added.
    Fresh,
    /// URL already had an entry. Tags were merged (union of
    /// old and new), saved_at_ms updated to now, title
    /// updated only if the existing title was empty (we
    /// don't clobber a user-edited title with a derived
    /// fallback).
    Merged {
        /// Number of NEW tags added by this save (existing
        /// tags that were already present aren't counted).
        new_tags_added: usize,
    },
}

pub fn save_bookmark(url: &str, title: &str, tags: &[&str]) -> Result<SaveResult, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("cannot save empty URL".to_string());
    }
    let path = bookmarks_path();
    let saved_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // v0.1.388 (dedup with merge): read existing entries,
    // check for URL match. If found, merge tags + bump
    // timestamp + rewrite the whole file. If not, append.
    //
    // Cost: a save now does a full file rewrite when
    // merging. Bookmarks files are tiny (a few KB even
    // after thousands of saves) so the cost is negligible
    // — the simpler in-place mutate-and-rewrite is
    // appropriate here. Append-only fallback for the
    // fresh-save case keeps the common path fast.
    let existing_content = fs::read_to_string(&path).unwrap_or_default();
    let mut entries = parse_bookmarks(&existing_content);

    // Look for an existing entry with the same URL.
    let existing_idx = entries.iter().position(|b| b.url == url);

    let result = match existing_idx {
        Some(idx) => {
            // Merge path. Compute the union of tags
            // (preserving existing order) so the user
            // doesn't see their tags shuffled when re-saving.
            let mut new_tags_added = 0;
            for new_tag in tags {
                let lower = new_tag.to_lowercase();
                let already = entries[idx].tags.iter()
                    .any(|t| t.to_lowercase() == lower);
                if !already {
                    entries[idx].tags.push(new_tag.to_string());
                    new_tags_added += 1;
                }
            }
            // Bump timestamp so the merged entry sorts to
            // the top of the "recent" view — re-saving
            // signals "still relevant."
            entries[idx].saved_at_ms = saved_at_ms;
            // Only update title if the existing one was
            // empty; preserves user hand-edits.
            if entries[idx].title.is_empty() && !title.is_empty() {
                entries[idx].title = title.to_string();
            }
            SaveResult::Merged { new_tags_added }
        }
        None => {
            // Fresh save. Append the new entry.
            entries.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
                saved_at_ms,
                tags: tags.iter().map(|t| t.to_string()).collect(),
            });
            SaveResult::Fresh
        }
    };

    // Rewrite the whole file. The header comment goes
    // first; entries follow in their original order
    // (the merged-update kept its position).
    let mut content = String::with_capacity(
        existing_content.len() + 512);
    content.push_str("# web2qml bookmarks — saved URLs.\n\
                      # Edit by hand or use 'save this' voice intent.\n");
    for b in &entries {
        content.push_str("\n[[bookmark]]\n");
        content.push_str(&format!("url = {}\n", quote_toml(&b.url)));
        content.push_str(&format!("title = {}\n", quote_toml(&b.title)));
        content.push_str(&format!("saved_at_ms = {}\n", b.saved_at_ms));
        if !b.tags.is_empty() {
            let escaped: Vec<String> = b.tags.iter()
                .map(|t| quote_toml(t))
                .collect();
            content.push_str(&format!("tags = [{}]\n", escaped.join(", ")));
        }
    }

    fs::write(&path, content)
        .map_err(|e| format!("write bookmarks failed: {}", e))?;
    Ok(result)
}

/// Read all bookmarks. Returns newest-first.
///
/// v0.1.726: mtime-invalidated memo. `search_suggestions` calls this on EVERY
/// URL-bar keystroke (debounced), and the old path did a full
/// `fs::read_to_string` + parse + sort each time. The prewarm comment noted
/// "memoizing is wrong; the content can change" — true for a blind memo, but a
/// correct one keys on the file's (mtime, len): we re-read only when the file
/// actually changed, otherwise serve the cached parse. A `stat` is far cheaper
/// than read+parse+sort, so per-keystroke cost drops to one metadata call.
pub fn read_bookmarks() -> Vec<Bookmark> {
    use std::sync::Mutex;
    use std::time::SystemTime;
    static CACHE: Mutex<Option<(SystemTime, u64, Vec<Bookmark>)>> = Mutex::new(None);

    let path = bookmarks_path();
    // Stat first — cheap. If unchanged since the cached read, reuse it.
    let sig = fs::metadata(&path).ok().map(|m| {
        let mtime = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        (mtime, m.len())
    });
    if let (Some((mtime, len)), Ok(guard)) = (sig, CACHE.lock()) {
        if let Some((cm, cl, ref cached)) = *guard {
            if cm == mtime && cl == len {
                return cached.clone();
            }
        }
    }
    // Miss (or file gone) → read + parse + sort, then update the memo.
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = parse_bookmarks(&content);
    out.sort_by(|a, b| b.saved_at_ms.cmp(&a.saved_at_ms));
    if let (Some((mtime, len)), Ok(mut guard)) = (sig, CACHE.lock()) {
        *guard = Some((mtime, len, out.clone()));
    }
    out
}

/// Uncached read (always hits disk) — for writers that just mutated the file
/// and must see their own write regardless of the memo, and for tests.
#[allow(dead_code)]
fn read_bookmarks_uncached() -> Vec<Bookmark> {
    let path = bookmarks_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = parse_bookmarks(&content);
    out.sort_by(|a, b| b.saved_at_ms.cmp(&a.saved_at_ms));
    out
}


/// v0.1.390: pre-warm the bookmarks file by reading it once
/// at startup so the first user-triggered render doesn't
/// pay the cold-cache fs::read_to_string cost. Discards
/// the result — the OS page cache holds the file content,
/// not us. Mirror of `recipes::prewarm_recipes()` (which
/// memoizes since recipes is compile-time data) — here the
/// content can change, so memoizing is wrong; we just warm
/// the kernel side.
///
/// Best-effort: read failures (no file yet on first launch)
/// are silently ignored.
pub fn prewarm_bookmarks() {
    let _ = read_bookmarks();
}

/// Parse bookmark entries from TOML-ish text. Hand-rolled
/// since we control the format; tolerates extra whitespace
/// and unknown fields.
fn parse_bookmarks(content: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let mut current: Option<Bookmark> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[[bookmark]]" {
            if let Some(b) = current.take() {
                if !b.url.is_empty() {
                    out.push(b);
                }
            }
            current = Some(Bookmark {
                url: String::new(),
                title: String::new(),
                saved_at_ms: 0,
                tags: Vec::new(),
            });
            continue;
        }
        let b = match current.as_mut() {
            Some(b) => b,
            None => continue,  // tolerate header lines
        };
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "url" => b.url = unquote_toml(value),
                "title" => b.title = unquote_toml(value),
                "saved_at_ms" => {
                    b.saved_at_ms = value.parse().unwrap_or(0);
                }
                "tags" => b.tags = parse_tags_array(value),
                _ => {}  // tolerate unknown fields
            }
        }
    }
    if let Some(b) = current.take() {
        if !b.url.is_empty() {
            out.push(b);
        }
    }
    out
}

/// Wrap a string in `"..."` with TOML-safe escaping.
fn quote_toml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Inverse of quote_toml. Tolerates unquoted input (treats
/// as the raw value).
fn unquote_toml(s: &str) -> String {
    let s = s.trim();
    if !s.starts_with('"') || !s.ends_with('"') || s.len() < 2 {
        return s.to_string();
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a TOML array of strings: `["a", "b"]`. Tolerates
/// extra whitespace; unknown elements skipped.
fn parse_tags_array(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Vec::new();
    }
    let inner = &s[1..s.len() - 1];
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_str = false;
    let mut prev_escape = false;
    for c in inner.chars() {
        if in_str {
            if prev_escape {
                buf.push(c);
                prev_escape = false;
            } else if c == '\\' {
                prev_escape = true;
            } else if c == '"' {
                in_str = false;
                out.push(buf.clone());
                buf.clear();
            } else {
                buf.push(c);
            }
        } else if c == '"' {
            in_str = true;
        }
        // Anything outside quotes is whitespace/commas;
        // ignored.
    }
    out
}

/// Render the bookmarks page as markdown. Same pipeline as
/// sessions, recipes — flows through `manpages::to_qml` in
/// the URL handler.
pub fn render_bookmarks() -> String {
    let entries = read_bookmarks();
    render_bookmarks_from(&entries, None)
}

/// v0.1.387 (bookmarks step 2, tag filtering): render only
/// the bookmarks matching `tag`. Returns the same page
/// structure with a "Filtered by tag: X" header note and
/// the matching entries.
pub fn render_bookmarks_filtered(tag: &str) -> String {
    let entries = read_bookmarks();
    render_bookmarks_from(&entries, Some(tag))
}

/// Render given a slice — extracted for testability.
fn render_bookmarks_from(entries: &[Bookmark], filter_tag: Option<&str>) -> String {
    let mut s = String::with_capacity(4 * 1024);
    s.push_str("# Bookmarks\n\n");

    // Apply tag filter if present. Case-insensitive match
    // against each entry's tags.
    let filtered: Vec<&Bookmark> = if let Some(tag) = filter_tag {
        let needle = tag.trim().to_lowercase();
        if needle.is_empty() {
            entries.iter().collect()
        } else {
            entries.iter().filter(|b| {
                b.tags.iter().any(|t| t.to_lowercase() == needle)
            }).collect()
        }
    } else {
        entries.iter().collect()
    };

    if let Some(tag) = filter_tag {
        let tag = tag.trim();
        if !tag.is_empty() {
            s.push_str(&format!("**Filtered by tag:** `{}`\n\n", tag));
        }
    }

    s.push_str("Pages you've saved with *\"save this\"* or *\"bookmark this\"* voice intents, or by adding entries manually to `~/.config/web2qml/bookmarks.toml`.\n\n");

    if filtered.is_empty() {
        if filter_tag.is_some() {
            s.push_str("*No bookmarks match this tag. Try a different one or view the full list at `web2qml://bookmarks`.*\n");
        } else {
            s.push_str("*No bookmarks saved yet. Say \"save this\" while viewing a page to start.*\n");
        }
        return s;
    }

    s.push_str(&format!("**{} bookmark(s)** — newest first.\n\n",
        filtered.len()));

    for (i, b) in filtered.iter().enumerate() {
        if i > 0 {
            s.push_str("\n---\n\n");
        }
        // Title as header; URL as code-inline link below.
        // Saved date in italics, tags as inline code spans.
        if !b.title.trim().is_empty() {
            s.push_str(&format!("## {}\n", b.title.trim()));
        } else {
            s.push_str(&format!("## {}\n", b.url));
        }
        s.push_str(&format!("`{}`\n\n", b.url));
        s.push_str(&format!("*Saved {}*", format_unix_ms(b.saved_at_ms)));
        if !b.tags.is_empty() {
            let tag_str: Vec<String> = b.tags.iter()
                .map(|t| format!("`{}`", t))
                .collect();
            s.push_str(&format!(" — {}", tag_str.join(" ")));
        }
        s.push_str("\n");
    }
    s
}

/// Format unix epoch ms as "YYYY-MM-DD HH:MM". Reuses the
/// same civil-from-days algorithm sessions.rs uses, since
/// chrono isn't a dep.
fn format_unix_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86400) as i64;
    let secs_today = secs % 86400;
    let hours = (secs_today / 3600) as u32;
    let minutes = ((secs_today % 3600) / 60) as u32;

    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, m, d, hours, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_toml_basic() {
        assert_eq!(quote_toml("hello"), "\"hello\"");
        assert_eq!(quote_toml("a \"b\" c"), "\"a \\\"b\\\" c\"");
    }

    #[test]
    fn unquote_toml_basic() {
        assert_eq!(unquote_toml("\"hello\""), "hello");
        assert_eq!(unquote_toml("\"a \\\"b\\\" c\""), "a \"b\" c");
        // Tolerate unquoted input.
        assert_eq!(unquote_toml("plain"), "plain");
    }

    #[test]
    fn parse_tags_array_basic() {
        assert_eq!(parse_tags_array(r#"["a", "b", "c"]"#),
            vec!["a", "b", "c"]);
        assert_eq!(parse_tags_array("[]"),
            Vec::<String>::new());
    }

    #[test]
    fn parse_tags_handles_embedded_quotes() {
        assert_eq!(parse_tags_array(r#"["a \"b\" c"]"#),
            vec!["a \"b\" c"]);
    }

    #[test]
    fn parse_bookmarks_basic() {
        let content = r#"
# header comment
[[bookmark]]
url = "https://a.com"
title = "Site A"
saved_at_ms = 1700000000000

[[bookmark]]
url = "https://b.com"
title = "Site B"
saved_at_ms = 1700000001000
tags = ["news", "tech"]
"#;
        let entries = parse_bookmarks(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].url, "https://a.com");
        assert_eq!(entries[0].title, "Site A");
        assert_eq!(entries[1].tags, vec!["news", "tech"]);
    }

    #[test]
    fn parse_bookmarks_skips_orphan_lines() {
        // Lines outside any [[bookmark]] block (other than
        // comments) should be tolerated, not crash.
        let content = r#"
url = "this should be ignored"

[[bookmark]]
url = "https://real.com"
"#;
        let entries = parse_bookmarks(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://real.com");
    }

    #[test]
    fn parse_bookmarks_skips_empty_url_entries() {
        let content = r#"
[[bookmark]]
title = "no url here"

[[bookmark]]
url = "https://real.com"
"#;
        let entries = parse_bookmarks(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://real.com");
    }

    #[test]
    fn render_empty_shows_message() {
        let out = render_bookmarks_from(&[], None);
        assert!(out.contains("No bookmarks saved"));
    }

    #[test]
    fn render_single_bookmark() {
        let b = Bookmark {
            url: "https://example.com".to_string(),
            title: "Example".to_string(),
            saved_at_ms: 1700000000000,
            tags: vec![],
        };
        let out = render_bookmarks_from(&[b], None);
        assert!(out.contains("## Example"));
        assert!(out.contains("`https://example.com`"));
        assert!(out.contains("1 bookmark"));
    }

    #[test]
    fn render_shows_tags_inline() {
        let b = Bookmark {
            url: "https://example.com".to_string(),
            title: "T".to_string(),
            saved_at_ms: 1700000000000,
            tags: vec!["news".to_string(), "tech".to_string()],
        };
        let out = render_bookmarks_from(&[b], None);
        assert!(out.contains("`news`"));
        assert!(out.contains("`tech`"));
    }

    #[test]
    fn render_falls_back_to_url_when_no_title() {
        let b = Bookmark {
            url: "https://example.com".to_string(),
            title: String::new(),
            saved_at_ms: 1700000000000,
            tags: vec![],
        };
        let out = render_bookmarks_from(&[b], None);
        // Header should be the URL since title is empty.
        assert!(out.contains("## https://example.com"));
    }

    #[test]
    fn round_trip_quote_unquote() {
        let cases = ["hello", "with \"quote\"", "tab\there",
            "newline\nhere", "back\\slash"];
        for c in &cases {
            assert_eq!(unquote_toml(&quote_toml(c)), *c);
        }
    }

    #[test]
    fn format_unix_ms_known_date() {
        // 2024-01-01 00:00:00 UTC = 1704067200000 ms
        assert_eq!(format_unix_ms(1704067200000), "2024-01-01 00:00");
    }

    // v0.1.387 (step 2): tag-based filtering tests.

    fn make_bookmark(url: &str, title: &str, tags: &[&str]) -> Bookmark {
        Bookmark {
            url: url.to_string(),
            title: title.to_string(),
            saved_at_ms: 1700000000000,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn filter_by_tag_basic() {
        let entries = vec![
            make_bookmark("https://a.com", "A", &["news"]),
            make_bookmark("https://b.com", "B", &["tech"]),
            make_bookmark("https://c.com", "C", &["news", "tech"]),
        ];
        let out = render_bookmarks_from(&entries, Some("news"));
        assert!(out.contains("Filtered by tag"));
        assert!(out.contains("`news`"));
        assert!(out.contains("https://a.com"));
        assert!(out.contains("https://c.com"));
        assert!(!out.contains("https://b.com"),
            "b.com (tech-only) should be filtered out: {}", out);
        // Count should reflect filter.
        assert!(out.contains("2 bookmark"));
    }

    #[test]
    fn filter_by_tag_case_insensitive() {
        let entries = vec![
            make_bookmark("https://a.com", "A", &["News"]),
            make_bookmark("https://b.com", "B", &["NEWS"]),
        ];
        let out = render_bookmarks_from(&entries, Some("news"));
        assert!(out.contains("2 bookmark"),
            "case-insensitive match should find both: {}", out);
    }

    #[test]
    fn filter_no_matches_shows_helpful_message() {
        let entries = vec![
            make_bookmark("https://a.com", "A", &["news"]),
        ];
        let out = render_bookmarks_from(&entries, Some("nonexistent"));
        assert!(out.contains("No bookmarks match this tag"));
        assert!(out.contains("web2qml://bookmarks"),
            "should point user back to full list");
    }

    #[test]
    fn filter_empty_tag_returns_all() {
        // Defensive: empty filter string = no filter.
        let entries = vec![
            make_bookmark("https://a.com", "A", &["news"]),
            make_bookmark("https://b.com", "B", &["tech"]),
        ];
        let out = render_bookmarks_from(&entries, Some(""));
        assert!(out.contains("https://a.com"));
        assert!(out.contains("https://b.com"));
        assert!(out.contains("2 bookmark"));
    }

    #[test]
    fn render_bookmarks_with_no_filter_unchanged() {
        // Sanity: passing None matches original behavior.
        let entries = vec![
            make_bookmark("https://a.com", "A", &["news"]),
            make_bookmark("https://b.com", "B", &["tech"]),
        ];
        let out = render_bookmarks_from(&entries, None);
        assert!(!out.contains("Filtered by"),
            "no filter header when filter is None");
        assert!(out.contains("2 bookmark"));
    }

    // v0.1.388: dedup-with-merge tests.
    //
    // These tests touch the real filesystem at the bookmarks
    // path. They MUST serialize via TEST_LOCK to avoid
    // stomping on each other, and they reset the file at
    // start so each test runs in a known-clean state. This
    // is the same pattern audit_log uses for SESSION_STATE
    // tests.

    use std::sync::Mutex;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_bookmarks_file() {
        let path = bookmarks_path();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_fresh_creates_new_entry() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        let result = save_bookmark(
            "https://fresh-test.example", "Fresh", &["a"]
        ).expect("save should succeed");
        assert_eq!(result, SaveResult::Fresh);
        let entries = read_bookmarks();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://fresh-test.example");
    }

    #[test]
    fn save_duplicate_url_merges_tags() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://dup-test.example", "First", &["a", "b"])
            .expect("first save should succeed");
        let result = save_bookmark(
            "https://dup-test.example", "Second", &["b", "c"]
        ).expect("second save should succeed");
        // "b" was already there, "c" is new → 1 new tag.
        assert_eq!(result, SaveResult::Merged { new_tags_added: 1 });
        let entries = read_bookmarks();
        // Only ONE entry — dedup worked.
        assert_eq!(entries.len(), 1);
        // Tags = union, preserving existing order then new
        // ones appended.
        assert_eq!(entries[0].tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn save_duplicate_url_no_new_tags() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://nontag.example", "T", &["a"])
            .expect("first save should succeed");
        let result = save_bookmark(
            "https://nontag.example", "T", &["a"]
        ).expect("second save should succeed");
        // Same tag — no new tags added.
        assert_eq!(result, SaveResult::Merged { new_tags_added: 0 });
    }

    #[test]
    fn save_duplicate_bumps_timestamp() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://bump.example", "T", &[])
            .expect("first save");
        let first_ts = read_bookmarks()[0].saved_at_ms;
        // Sleep briefly to guarantee the ms clock advances.
        std::thread::sleep(std::time::Duration::from_millis(5));
        save_bookmark("https://bump.example", "T", &[])
            .expect("second save");
        let second_ts = read_bookmarks()[0].saved_at_ms;
        assert!(second_ts > first_ts,
            "second save should bump timestamp: {} vs {}",
            first_ts, second_ts);
    }

    #[test]
    fn save_merge_preserves_user_edited_title() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://title-preserve.example",
            "User-edited title", &[]).expect("first save");
        // Re-save with a different title (e.g. user later
        // saves with a derived fallback title).
        save_bookmark("https://title-preserve.example",
            "different fallback", &[]).expect("second save");
        let entries = read_bookmarks();
        assert_eq!(entries[0].title, "User-edited title",
            "merge must not clobber existing title");
    }

    #[test]
    fn save_merge_fills_in_empty_title() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://empty-title.example",
            "", &[]).expect("first save");
        save_bookmark("https://empty-title.example",
            "Now has title", &[]).expect("second save");
        let entries = read_bookmarks();
        assert_eq!(entries[0].title, "Now has title",
            "empty title should be filled in on merge");
    }

    #[test]
    fn save_tag_merge_is_case_insensitive() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://case-test.example", "T", &["News"])
            .expect("first save");
        let result = save_bookmark(
            "https://case-test.example", "T", &["news", "tech"]
        ).expect("second save");
        // "news" matches "News" case-insensitively, so only
        // "tech" is genuinely new.
        assert_eq!(result, SaveResult::Merged { new_tags_added: 1 });
        let entries = read_bookmarks();
        // Existing "News" stays in original case.
        assert!(entries[0].tags.contains(&"News".to_string()));
        assert!(entries[0].tags.contains(&"tech".to_string()));
        // Doesn't add duplicate-cased "news".
        let lower_count = entries[0].tags.iter()
            .filter(|t| t.to_lowercase() == "news")
            .count();
        assert_eq!(lower_count, 1, "no case-duplicate tag");
    }

    #[test]
    fn save_distinct_urls_are_kept_separate() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_bookmarks_file();
        save_bookmark("https://a.example", "A", &[]).expect("save a");
        save_bookmark("https://b.example", "B", &[]).expect("save b");
        let entries = read_bookmarks();
        assert_eq!(entries.len(), 2,
            "distinct URLs should produce distinct entries");
    }
}
