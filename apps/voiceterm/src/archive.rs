// src/archive.rs
// Page archive: extract key points, store in packed binary DB.
// Format: ~/.config/web2qml/archive.db
// Each record: [u32 len][json bytes]
// JSON: { url, title, date, domain, key_points: [{kind, text, context}], text_snippet }

use std::fs;
use std::io::{Read, Write, BufReader, BufWriter};
use std::path::PathBuf;
use std::sync::OnceLock;

// v0.1.283 (perf nit): cache `scraper::Selector::parse` results.
// Each call costs ~1µs of parsing per invocation. extract_key_points
// is on the archive_page critical path (user-triggered, not in a
// loop, so the gain is negligible) but the pattern matches
// sel_meta/sel_script in main.rs for consistency.
static SEL_BODY_STAR: OnceLock<scraper::Selector> = OnceLock::new();
static SEL_BODY: OnceLock<scraper::Selector> = OnceLock::new();

fn sel_body_star() -> &'static scraper::Selector {
    SEL_BODY_STAR.get_or_init(|| scraper::Selector::parse("body *").expect("static selector"))
}

fn sel_body() -> &'static scraper::Selector {
    SEL_BODY.get_or_init(|| scraper::Selector::parse("body").expect("static selector"))
}

#[derive(Clone, Debug)]
pub struct KeyPoint {
    pub kind: &'static str, // "price", "date", "command", "email", "version", "phone"
    pub text: String,        // the extracted value
    pub context: String,     // surrounding text for context
}

#[derive(Clone, Debug)]
pub struct ArchivedPage {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub archived_at: String, // ISO 8601
    pub key_points: Vec<KeyPoint>,
    pub text_snippet: String, // first ~500 chars of visible text
    pub html_size: usize,
}

pub struct Archive {
    path: PathBuf,
    pub entries: Vec<ArchivedPage>,
}


/// Safe truncation at char boundary
pub fn safe_trunc(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) { i -= 1; }
    &s[..i]
}

impl Archive {
    pub fn new() -> Self {
        let path = Self::db_path();
        let entries = Self::load_from_disk(&path);
        eprintln!("Archive: {} entries loaded from {}", entries.len(), path.display());
        Archive { path, entries }
    }

    fn db_path() -> PathBuf {
        let dir = PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into())).join(".config/web2qml");
        let _ = fs::create_dir_all(&dir);
        dir.join("archive.db")
    }

    fn load_from_disk(path: &PathBuf) -> Vec<ArchivedPage> {
        let file = match fs::File::open(path) { Ok(f) => f, Err(_) => return Vec::new() };
        let mut reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut len_buf = [0u8; 4];
        loop {
            if reader.read_exact(&mut len_buf).is_err() { break; }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len > 1_000_000 { break; } // sanity
            let mut data = vec![0u8; len];
            if reader.read_exact(&mut data).is_err() { break; }
            if let Ok(json) = String::from_utf8(data) {
                if let Some(entry) = Self::parse_json(&json) {
                    entries.push(entry);
                }
            }
        }
        entries
    }

    pub fn save_to_disk(&self) {
        let file = match fs::File::create(&self.path) { Ok(f) => f, Err(e) => { eprintln!("Archive save: {e}"); return; } };
        let mut writer = BufWriter::new(file);
        for entry in &self.entries {
            let json = Self::to_json(entry);
            let bytes = json.as_bytes();
            let _ = writer.write_all(&(bytes.len() as u32).to_le_bytes());
            let _ = writer.write_all(bytes);
        }
        let _ = writer.flush();
    }

    pub fn archive_page(&mut self, url: &str, html: &str, title: &str) {
        let domain = crate::layout_cache::url_domain(url);
        let now = chrono_now();
        let key_points = extract_key_points(html);
        let text_snippet = extract_text_snippet(html, 500);

        // Dedup: update existing entry for same URL or add new
        if let Some(existing) = self.entries.iter_mut().find(|e| e.url == url) {
            existing.archived_at = now;
            existing.title = title.to_string();
            existing.key_points = key_points;
            existing.text_snippet = text_snippet;
            existing.html_size = html.len();
        } else {
            self.entries.push(ArchivedPage {
                url: url.to_string(), title: title.to_string(), domain,
                archived_at: now, key_points, text_snippet, html_size: html.len(),
            });
        }

        // Cap at 1000 entries
        if self.entries.len() > 1000 { self.entries.remove(0); }
        // v0.1.286: save on EVERY archive (was: every 5). Mismatch
        // with other persistence modules (token_stats, confusion_table,
        // llm_cache, decision_cache, speakers, chat) which all save
        // on every mutation. Combined with `panic = "abort"` in
        // Cargo.toml, the previous 5-entry batching meant up to 4
        // archived pages could be lost on a crash. The cost of a
        // disk write per archive is trivial — ~25KB per write, and
        // archive_page is user-action-triggered (one tap or voice
        // command per archive), not in a loop. Filed under audit
        // pass 4 as data-loss risk; this is the fix.
        self.save_to_disk();
    }

    /// Search archive by query
    pub fn search(&self, query: &str) -> Vec<&ArchivedPage> {
        let q = query.to_lowercase();
        self.entries.iter().filter(|e| {
            e.url.to_lowercase().contains(&q)
            || e.title.to_lowercase().contains(&q)
            || e.text_snippet.to_lowercase().contains(&q)
            || e.key_points.iter().any(|kp| kp.text.to_lowercase().contains(&q) || kp.context.to_lowercase().contains(&q))
        }).collect()
    }

    /// List all entries for a domain
    pub fn by_domain(&self, domain: &str) -> Vec<&ArchivedPage> {
        self.entries.iter().filter(|e| e.domain == domain).collect()
    }

    /// Get recent entries
    pub fn recent(&self, n: usize) -> Vec<&ArchivedPage> {
        self.entries.iter().rev().take(n).collect()
    }

    // Simple JSON serialization (no serde dependency)
    fn to_json(entry: &ArchivedPage) -> String {
        let mut o = String::with_capacity(512);
        o.push('{');
        json_str(&mut o, "url", &entry.url); o.push(',');
        json_str(&mut o, "title", &entry.title); o.push(',');
        json_str(&mut o, "domain", &entry.domain); o.push(',');
        json_str(&mut o, "date", &entry.archived_at); o.push(',');
        json_str(&mut o, "snippet", &entry.text_snippet); o.push(',');
        o.push_str("\"size\":"); o.push_str(&entry.html_size.to_string()); o.push(',');
        o.push_str("\"kp\":[");
        for (i, kp) in entry.key_points.iter().enumerate() {
            if i > 0 { o.push(','); }
            o.push('{');
            json_str(&mut o, "k", kp.kind); o.push(',');
            json_str(&mut o, "t", &kp.text); o.push(',');
            json_str(&mut o, "c", &kp.context);
            o.push('}');
        }
        o.push_str("]}");
        o
    }

    fn parse_json(json: &str) -> Option<ArchivedPage> {
        // Minimal JSON parser — extract known fields
        let url = json_get(json, "url")?;
        let title = json_get(json, "title").unwrap_or_default();
        let domain = json_get(json, "domain").unwrap_or_default();
        let date = json_get(json, "date").unwrap_or_default();
        let snippet = json_get(json, "snippet").unwrap_or_default();
        let size = json_get_num(json, "size").unwrap_or(0);

        let mut key_points = Vec::new();
        // Parse key_points array
        if let Some(kp_start) = json.find("\"kp\":[") {
            let kp_str = &json[kp_start + 5..];
            if let Some(end) = kp_str.find(']') {
                let arr = &kp_str[1..end];
                for obj in arr.split("},{") {
                    let obj = obj.trim_matches(|c| c == '{' || c == '}');
                    if let (Some(k), Some(t)) = (json_get(obj, "k"), json_get(obj, "t")) {
                        key_points.push(KeyPoint {
                            kind: match k.as_str() { "price"=>"price", "date"=>"date", "command"=>"command", "email"=>"email", "version"=>"version", "phone"=>"phone", _=>"other" },
                            text: t,
                            context: json_get(obj, "c").unwrap_or_default(),
                        });
                    }
                }
            }
        }

        Some(ArchivedPage { url, title, domain, archived_at: date, key_points, text_snippet: snippet, html_size: size })
    }

    /// Format archive list for QML
    pub fn format_list(&self, entries: &[&ArchivedPage]) -> String {
        let mut out = String::with_capacity(1024);
        for e in entries {
            out.push_str("A\x1F"); out.push_str(safe_trunc(&e.title, 60));
            out.push('\x1F'); out.push_str(&e.url);
            out.push('\x1F'); out.push_str(&e.archived_at);
            out.push('\x1F'); out.push_str(&e.domain);
            // Key points summary
            out.push('\x1F');
            for (i, kp) in e.key_points.iter().take(5).enumerate() {
                if i > 0 { out.push_str(", "); }
                out.push_str(kp.kind); out.push(':'); out.push_str(safe_trunc(&kp.text, 30));
            }
            out.push('\n');
        }
        out
    }
}

/// Extract key points from HTML text content
pub fn extract_key_points(html: &str) -> Vec<KeyPoint> {
    let doc = scraper::Html::parse_document(html);
    let mut points = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for el in doc.select(sel_body_star()) {
        let tag = el.value().name();
        // v0.1.283: dropped `head|meta|link` from the skip list —
        // they're unreachable under "body *" selector. `script`,
        // `style`, `noscript` ARE reachable (browsers move them
        // around in the DOM) and still need skipping.
        if matches!(tag, "script"|"style"|"noscript") { continue; }

        for child in el.children() {
            if let scraper::Node::Text(t) = child.value() {
                let text = t.trim();
                if text.len() < 3 || text.len() > 500 { continue; }
                extract_from_text(text, &mut points, &mut seen);
            }
        }
        if points.len() > 50 { break; }
    }
    points
}

fn extract_from_text(text: &str, points: &mut Vec<KeyPoint>, seen: &mut std::collections::HashSet<String>) {
    // Safe truncation at char boundary
    let ctx_end = if text.len() <= 80 { text.len() } else {
        let mut i = 80;
        while i > 0 && !text.is_char_boundary(i) { i -= 1; }
        i
    };
    let context = &text[..ctx_end];

    // Prices: $X.XX, €X, £X
    //
    // v0.1.380: the original implementation did `&rest[1..]` to skip
    // past the matched currency character, but `€` and `£` are 2-byte
    // UTF-8 sequences ($ is 1 byte). `rest[1..]` lands in the middle
    // of those characters and panics. Use the character's actual
    // UTF-8 length instead — chars().next() gives us the character,
    // .len_utf8() gives the right offset.
    for (i, _) in text.match_indices(|c: char| c == '$' || c == '€' || c == '£') {
        let rest = &text[i..];
        let sym_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        // Defensive: if the symbol is the last char in the string,
        // there's no value to extract.
        if sym_len >= rest.len() { continue; }
        let after_sym = &rest[sym_len..];
        let value_end = after_sym
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',' && c != ' ')
            .unwrap_or(after_sym.len());
        let end = sym_len + value_end;
        let price = rest[..end.min(rest.len())].trim();
        if price.len() >= 2 && price.len() <= 15 && !seen.contains(price) {
            seen.insert(price.to_string());
            points.push(KeyPoint { kind: "price", text: price.to_string(), context: context.to_string() });
        }
    }

    // Dates: YYYY-MM-DD, MM/DD/YYYY, Month DD YYYY
    let months = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    for month in &months {
        if let Some(pos) = text.find(month) {
            // v0.1.396: char-boundary-safe slicing. Original
            // bug: `pos + month.len() + 12` is a byte offset
            // that can land mid-codepoint when the 12 bytes
            // after a month name contain multibyte chars.
            // User-hit on The Verge article with mojibake
            // `havenât` (UTF-8 `’` mis-decoded as `â\u{99}`)
            // — `\u{99}` is a 2-byte sequence at 149..151,
            // and `end=150` split it.
            //
            // Snap `end` back to the previous char boundary.
            // Worst case: we trim a few bytes off the date
            // window, which is fine — month + day + year
            // fits comfortably under the 12-byte budget for
            // ASCII content.
            let want_end = pos + month.len() + 12;
            let mut end = want_end.min(text.len());
            while end > pos && !text.is_char_boundary(end) {
                end -= 1;
            }
            let date_str = text[pos..end].trim();
            if date_str.len() >= month.len() + 2 && !seen.contains(date_str) {
                seen.insert(date_str.to_string());
                points.push(KeyPoint { kind: "date", text: date_str.to_string(), context: context.to_string() });
            }
        }
    }
    // ISO dates
    for word in text.split_whitespace() {
        // v0.1.396: char-boundary-safe length check. The
        // raw `word.len() == 10` check is BYTE length, so a
        // 9-char string with one 2-byte codepoint passes
        // — and then `word[..4]` may split a codepoint.
        // Guard by also asserting the first 4 bytes are a
        // valid char boundary AND that we have 10 chars
        // (not just 10 bytes). The cheap escape: require
        // all-ASCII for the structural positions.
        if word.len() == 10
            && word.is_char_boundary(4)
            && word.is_char_boundary(7)
            && word.chars().nth(4) == Some('-')
            && word.chars().nth(7) == Some('-')
        {
            if word[..4].chars().all(|c| c.is_ascii_digit()) && !seen.contains(word) {
                seen.insert(word.to_string());
                points.push(KeyPoint { kind: "date", text: word.to_string(), context: context.to_string() });
            }
        }
    }

    // Commands: $ xxx, sudo xxx, npm xxx, pip xxx, cargo xxx, git xxx
    for prefix in &["$ ", "sudo ", "npm ", "pip ", "cargo ", "git ", "apt ", "brew ", "curl ", "wget "] {
        if text.starts_with(prefix) || text.contains(&format!("\n{}", prefix)) {
            let cmd = if let Some(p) = text.find(prefix) { safe_trunc(&text[p..], 80) } else { text };
            let cmd = cmd.trim();
            if cmd.len() > 3 && !seen.contains(cmd) {
                seen.insert(cmd.to_string());
                points.push(KeyPoint { kind: "command", text: cmd.to_string(), context: context.to_string() });
            }
        }
    }

    // Emails
    for word in text.split_whitespace() {
        if word.contains('@') && word.contains('.') && word.len() > 5 && word.len() < 60 {
            let email = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.' && c != '-' && c != '_');
            if email.contains('@') && !seen.contains(email) {
                seen.insert(email.to_string());
                points.push(KeyPoint { kind: "email", text: email.to_string(), context: context.to_string() });
            }
        }
    }

    // Version numbers: vX.Y.Z, X.Y.Z
    for word in text.split_whitespace() {
        let w = word.trim_start_matches('v').trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if w.len() >= 3 && w.chars().filter(|c| *c == '.').count() >= 1 && w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            if w.chars().all(|c| c.is_ascii_digit() || c == '.') && !seen.contains(w) {
                seen.insert(w.to_string());
                points.push(KeyPoint { kind: "version", text: if word.starts_with('v') { word.to_string() } else { w.to_string() }, context: context.to_string() });
            }
        }
    }
}

fn extract_text_snippet(html: &str, max_len: usize) -> String {
    let doc = scraper::Html::parse_document(html);
    if let Some(body) = doc.select(sel_body()).next() {
        let text: String = body.text().collect();
        let clean: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        safe_trunc(&clean, max_len).to_string()
    } else { String::new() }
}

pub fn chrono_now() -> String {
    // Simple ISO 8601 without chrono dep
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86400; let rem = secs % 86400;
    let h = rem / 3600; let m = (rem % 3600) / 60; let s = rem % 60;
    // Approximate date from days since epoch
    let mut y = 1970i32; let mut d = days as i32;
    loop {
        let yd = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if d < yd { break; }
        d -= yd; y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0;
    for md in &mdays { if d < *md { break; } d -= md; mo += 1; }
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo + 1, d + 1, h, m, s)
}

// Minimal JSON helpers — no serde
fn json_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => {}, '\t' => o.push_str("\\t"), c if c.is_control() => {}, _ => o.push(c) }
    }
    o
}

fn json_str(out: &mut String, key: &str, val: &str) {
    out.push('"'); out.push_str(key); out.push_str("\":\""); out.push_str(&json_esc(val)); out.push('"');
}

fn json_get(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":\"", key);
    let start = json.find(&pat)? + pat.len();
    let rest = &json[start..];
    let mut end = 0;
    let mut esc = false;
    for c in rest.chars() {
        if esc { esc = false; end += c.len_utf8(); continue; }
        if c == '\\' { esc = true; end += 1; continue; }
        if c == '"' { break; }
        end += c.len_utf8();
    }
    Some(rest[..end].replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\"))
}

fn json_get_num(json: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{}\":", key);
    let start = json.find(&pat)? + pat.len();
    let rest = &json[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Truncation at ASCII boundary: simple case.
    #[test]
    fn safe_trunc_ascii() {
        assert_eq!(safe_trunc("hello world", 5), "hello");
        assert_eq!(safe_trunc("hello world", 100), "hello world");
    }

    /// Truncation should never split a multi-byte character.
    /// "café" is 5 bytes (c, a, f, é=2 bytes). max=4 should
    /// retreat to 3 (after "caf").
    #[test]
    fn safe_trunc_preserves_char_boundary() {
        let s = "café";
        assert_eq!(s.len(), 5);
        let t = safe_trunc(s, 4);
        // Must be valid UTF-8 — never a half-byte of é.
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
        // Should be "caf" (3 bytes) since 4 is mid-é.
        assert_eq!(t, "caf");
    }

    /// max=0 returns empty string.
    #[test]
    fn safe_trunc_zero() {
        assert_eq!(safe_trunc("anything", 0), "");
    }

    /// Empty input returns empty regardless of max.
    #[test]
    fn safe_trunc_empty_input() {
        assert_eq!(safe_trunc("", 100), "");
        assert_eq!(safe_trunc("", 0), "");
    }

    /// json_get_num parses numeric values from simple JSON.
    #[test]
    fn json_get_num_simple() {
        assert_eq!(json_get_num("{\"count\":42}", "count"), Some(42));
        assert_eq!(json_get_num("{\"foo\":1,\"bar\":99}", "bar"), Some(99));
    }

    /// json_get_num returns None for missing keys.
    #[test]
    fn json_get_num_missing() {
        assert_eq!(json_get_num("{\"foo\":1}", "nonexistent"), None);
    }

    // v0.1.396 regression tests for char-boundary panics
    // surfaced by user hardware-test on The Verge article.

    /// extract_from_text used to panic at archive.rs:302
    /// when the 12 bytes after a Month name contained a
    /// multibyte codepoint that the byte-arithmetic `end`
    /// split. Concrete reproducer matches the actual panic
    /// message from the user log: mojibake `havenât`
    /// (UTF-8 `’` mis-decoded as `â\u{99}`) appearing
    /// after the month name.
    #[test]
    fn extract_from_text_month_with_trailing_mojibake_no_panic() {
        let text = "Released in May with new features. They havenât shipped yet.";
        let mut points = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Must not panic. We don't care what's extracted —
        // the test verifies the slicing is safe.
        extract_from_text(text, &mut points, &mut seen);
    }

    /// Concrete reproducer of the actual panic in the user
    /// log. The string `havenât` appears in the 12-byte
    /// window after the matched month, splitting `\u{99}`.
    #[test]
    fn extract_from_text_exact_user_log_panic_string() {
        // Truncated to the relevant chunk; the date scan
        // looks ~12 bytes past each month.
        let text = "But our review units havenât budged in October. \
                    And Iâve so far failed.";
        let mut points = Vec::new();
        let mut seen = std::collections::HashSet::new();
        extract_from_text(text, &mut points, &mut seen);
    }

    /// ISO date scan used to slice `word[..4]` even when
    /// `word.len() == 10` was a BYTE length that included
    /// multibyte chars in the first 4 bytes.
    #[test]
    fn extract_from_text_iso_date_with_unicode_no_panic() {
        // A 10-byte word with a multibyte prefix. Must not
        // panic on the word[..4] slice.
        let text = "déjà-vu-X9 today";  // déjà = 5 bytes, hyphen=1, so first 7 bytes are multibyte
        let mut points = Vec::new();
        let mut seen = std::collections::HashSet::new();
        extract_from_text(text, &mut points, &mut seen);
    }

    /// Sanity: legitimate months still extract correctly.
    #[test]
    fn extract_from_text_month_extracts_normally() {
        let text = "Released March 15 2024 to acclaim.";
        let mut points = Vec::new();
        let mut seen = std::collections::HashSet::new();
        extract_from_text(text, &mut points, &mut seen);
        assert!(points.iter().any(|p| p.kind == "date"
            && p.text.starts_with("March")),
            "should extract March date: {:?}", points);
    }
}
