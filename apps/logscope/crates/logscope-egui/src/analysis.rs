//! Real, dependency-light analysis shared across tabs. Mirrors logscope's
//! charts/analyzer passes: token frequency + a repeat histogram via XxHash64.
//! The `Doc` is the in-app log buffer that Sources fills and Peek/Tokens/Blocks read.
use std::collections::HashMap;
use std::hash::Hasher;
use twox_hash::XxHash64;

/// Sample log with deliberate repeats so the charts show real structure.
pub const SAMPLE: &str = "\
[12:01:01] GET /health 200 0.4ms
[12:01:01] GET /health 200 0.4ms
[12:01:02] GET /api/users 200 12ms
[12:01:02] WARN cache miss key=session:8842
[12:01:03] GET /health 200 0.4ms
[12:01:03] GET /api/users 200 11ms
[12:01:04] ERROR upstream timed out (110: Connection timed out)
[12:01:04] GET /health 200 0.4ms
[12:01:05] GET /api/orders 500 2ms
[12:01:05] WARN retrying upstream (1/5)
[12:01:06] GET /health 200 0.4ms
[12:01:06] ERROR worker 2841 exited on signal 11
[12:01:07] GET /health 200 0.4ms
[12:01:07] GET /api/users 200 13ms
[12:01:08] GET /health 200 0.4ms";

pub struct Doc {
    pub name: String,
    pub lines: Vec<String>,
    /// True for the built-in demo document, false once a real file is loaded.
    ///
    /// The default Doc is a bundled `sample.log`, and the chat attaches the current document to every
    /// prompt with "Use only this data; cite line patterns or lines from it." On a fresh session that
    /// instructs the model to answer from a demo file the user has never seen — so "hi there" comes back
    /// as a discussion of GET /health and a worker exiting on signal 11. The model was obeying.
    pub placeholder: bool,
}
impl Default for Doc {
    fn default() -> Self { Self { placeholder: true, ..Self::from_text("sample.log", SAMPLE) } }
}
impl Doc {
    pub fn from_text(name: &str, text: &str) -> Self {
        Self { name: name.to_string(), lines: text.lines().map(|s| s.to_string()).collect(),
               placeholder: false }
    }
    pub fn set_from_text(&mut self, name: &str, text: &str) {
        self.name = name.to_string();
        self.lines = text.lines().map(|s| s.to_string()).collect();
        // Anything loaded through a real path — peek, cargo, a file drop — is the user's data and is
        // attachable. Clearing the flag here means every load route gets that for free rather than each
        // one having to remember.
        // Anything loaded through a real path — peek, cargo, a file drop — is the user's data and is
        // attachable. Clearing here means every load route gets that for free rather than each one
        // having to remember.
        self.placeholder = false;
    }
}

/// Top-N token frequencies (alnum tokens, min length 2).
pub fn token_frequencies(lines: &[String], top: usize) -> Vec<(String, u64)> {
    let mut counts: HashMap<String, u64> = HashMap::new();
    for line in lines {
        for tok in line.split(|c: char| !c.is_alphanumeric()) {
            if tok.len() >= 2 {
                *counts.entry(tok.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<(String, u64)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(top);
    v
}

/// Collapse digit runs to '#', so timestamped/variant lines share a template
/// (e.g. "[12:01:03] GET /health 200 0.4ms" -> "[#:#:#] GET /health # #.#ms").
pub fn normalize_template(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_digit = false;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            if !prev_digit { out.push('#'); }
            prev_digit = true;
        } else {
            out.push(ch);
            prev_digit = false;
        }
    }
    out
}

/// Repeat histogram grouped by XxHash64 of the normalized template (logscope's
/// block-repeat pass, at the log-useful template granularity). Returns
/// (representative template, count) sorted by count desc.
pub fn repeated_templates(lines: &[String], top: usize) -> Vec<(String, u64)> {
    let mut by_hash: HashMap<u64, (String, u64)> = HashMap::new();
    for line in lines {
        let t = normalize_template(line);
        let mut h = XxHash64::with_seed(0);
        h.write(t.as_bytes());
        let e = by_hash.entry(h.finish()).or_insert_with(|| (t.clone(), 0));
        e.1 += 1;
    }
    let mut v: Vec<(String, u64)> = by_hash.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(top);
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_demo_document_is_VIEWABLE_but_never_attached() {
        // The bug: a fresh session that said "hi there" got an answer about GET /health and a worker
        // exiting on signal 11, because Doc::default() loads a bundled sample.log and the chat attached
        // it with "Use only this data; cite line patterns from it". The model did exactly as told,
        // about a document the user had never seen. Viewing it is fine; instructing a model that it is
        // authoritative is not.
        let d = Doc::default();
        assert!(d.placeholder, "the built-in demo must be marked");
        assert!(!d.lines.is_empty(), "and still have content — the peek and cargo pages show it");
    }

    #[test]
    fn every_real_load_route_clears_the_flag() {
        // Cleared inside set_from_text rather than at each call site, so peek, cargo and a file drop
        // all get it without having to remember.
        let mut d = Doc::default();
        d.set_from_text("prod.log", "GET / 200\n");
        assert!(!d.placeholder, "a loaded file is the user's data and is attachable");
        assert_eq!(d.name, "prod.log");
        assert!(!Doc::from_text("x.log", "a\n").placeholder, "a constructed Doc was never a placeholder");
    }

    use super::*;
    fn count_of(v: &[(String, u64)], k: &str) -> u64 {
        v.iter().find(|(t, _)| t == k).map(|(_, c)| *c).unwrap_or(0)
    }
    #[test]
    fn tokens_count_and_rank() {
        let lines = vec!["error error GET".to_string(), "GET ok ok ok".to_string()];
        let f = token_frequencies(&lines, 10);
        assert_eq!(count_of(&f, "ok"), 3);
        assert_eq!(count_of(&f, "error"), 2);
        assert_eq!(count_of(&f, "GET"), 2);
        assert_eq!(f[0].0, "ok");
    }
    #[test]
    fn templates_collapse_digits() {
        assert_eq!(normalize_template("[12:01:03] GET /x 200 0.4ms"), "[#:#:#] GET /x # #.#ms");
        let lines = vec![
            "[12:01:01] GET /x 200 1ms".to_string(),
            "[12:01:02] GET /x 200 9ms".to_string(),
            "once".to_string(),
        ];
        let h = repeated_templates(&lines, 10);
        assert_eq!(h[0].1, 2); // the two /x lines share a template
        assert_eq!(h.len(), 2);
    }
    #[test]
    fn sample_template_repeats() {
        let doc = Doc::default();
        let h = repeated_templates(&doc.lines, 40);
        assert!(h[0].1 >= 6, "the /health template should repeat >= 6x, got {}", h[0].1);
    }
}
