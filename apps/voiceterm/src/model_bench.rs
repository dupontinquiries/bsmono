//! Per-model benchmark store for the setup assistant's "tester
//! playground". When the user runs a standard test prompt through a
//! downloaded model, we record how fast it ran (tokens/sec), how
//! much it produced, and which test prompt was used — keyed by model
//! name — so models can be compared head-to-head later.
//!
//! Persistence mirrors token_log: a JSONL file (one record per line)
//! under the config dir, with tolerant parsing and a test seam
//! (explicit-path variants) so the pure logic is unit-testable
//! without touching the real config dir.
//!
//! Records accumulate (append-only) so you can see whether a model's
//! speed changed across runs; comparison queries fold them to the
//! latest-per-model or best-per-model as needed.

use std::path::PathBuf;

/// One benchmark run of one model.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchRecord {
    /// Model name (the GGUF filename, e.g.
    /// "Llama-3.2-3B-Instruct-Q4_K_M.gguf"). The comparison key.
    pub model: String,
    /// Throughput in tokens/sec (generation speed). The headline
    /// number for "which model is fastest on this machine".
    pub tok_per_sec: f32,
    /// Total milliseconds the run took end-to-end (load excluded;
    /// generation only). Useful for latency-sensitive comparison.
    pub elapsed_ms: u32,
    /// Characters of output produced (a rough size proxy; we don't
    /// always have an exact token count from every backend).
    pub output_chars: u32,
    /// Which standard test prompt was used (a short label so runs
    /// with different prompts aren't compared as if equal).
    pub prompt_label: String,
    /// ISO-8601 timestamp of the run.
    pub ts: String,
}

/// Format a record as one JSONL line (no trailing newline).
pub fn format_record(r: &BenchRecord) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str("\"model\":");
    push_json_str(&mut out, &r.model);
    out.push_str(",\"tok_per_sec\":");
    // Keep two decimals; tok/s rarely needs more.
    out.push_str(&format!("{:.2}", r.tok_per_sec));
    out.push_str(",\"elapsed_ms\":");
    out.push_str(&r.elapsed_ms.to_string());
    out.push_str(",\"output_chars\":");
    out.push_str(&r.output_chars.to_string());
    out.push_str(",\"prompt_label\":");
    push_json_str(&mut out, &r.prompt_label);
    out.push_str(",\"ts\":");
    push_json_str(&mut out, &r.ts);
    out.push('}');
    out
}

fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one JSONL line back into a BenchRecord. Tolerant of field
/// order; returns None on any failure (corrupt/partial line).
pub fn parse_record(line: &str) -> Option<BenchRecord> {
    let line = line.trim();
    if line.is_empty() || !line.starts_with('{') { return None; }
    Some(BenchRecord {
        model: extract_str(line, "\"model\"")?,
        tok_per_sec: extract_f32(line, "\"tok_per_sec\"")?,
        elapsed_ms: extract_u32(line, "\"elapsed_ms\"")?,
        output_chars: extract_u32(line, "\"output_chars\"")?,
        // prompt_label is optional for forward-compat with older
        // records; default to empty rather than dropping the row.
        prompt_label: extract_str(line, "\"prompt_label\"").unwrap_or_default(),
        ts: extract_str(line, "\"ts\"").unwrap_or_default(),
    })
}

fn extract_str(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let colon = after.find(':')?;
    let after = &after[colon + 1..];
    let quote = after.find('"')?;
    let body = &after[quote + 1..];
    let mut out = String::new();
    let mut esc = false;
    for c in body.chars() {
        if esc {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => { /* skip the 4 hex digits crudely below */ }
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => out.push(c),
            }
            esc = false;
            continue;
        }
        if c == '\\' { esc = true; continue; }
        if c == '"' { break; }
        out.push(c);
    }
    Some(out)
}

fn extract_u32(line: &str, key: &str) -> Option<u32> {
    let raw = extract_number_str(line, key)?;
    // Numbers here are integers; truncate any stray decimal.
    let int_part = raw.split('.').next().unwrap_or("");
    int_part.parse().ok()
}

fn extract_f32(line: &str, key: &str) -> Option<f32> {
    extract_number_str(line, key)?.parse().ok()
}

/// Pull the raw numeric token (digits, '.', '-') following a key.
fn extract_number_str(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let colon = after.find(':')?;
    let after = &after[colon + 1..];
    let trimmed = after.trim_start();
    let mut out = String::new();
    for c in trimmed.chars() {
        if c.is_ascii_digit() || c == '.' || c == '-' {
            out.push(c);
        } else {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Parse every record from a JSONL blob, skipping unparseable lines.
pub fn parse_all(content: &str) -> Vec<BenchRecord> {
    content.lines().filter_map(parse_record).collect()
}

/// Reduce records to the BEST (highest tok/sec) run per model name.
/// Returns them sorted fastest-first. Ties broken by lower
/// elapsed_ms (faster wall-clock). This is the comparison view the
/// playground shows.
pub fn best_per_model(records: &[BenchRecord]) -> Vec<BenchRecord> {
    use std::collections::HashMap;
    let mut best: HashMap<&str, &BenchRecord> = HashMap::new();
    for r in records {
        match best.get(r.model.as_str()) {
            Some(prev) => {
                let better = r.tok_per_sec > prev.tok_per_sec
                    || (r.tok_per_sec == prev.tok_per_sec
                        && r.elapsed_ms < prev.elapsed_ms);
                if better {
                    best.insert(r.model.as_str(), r);
                }
            }
            None => {
                best.insert(r.model.as_str(), r);
            }
        }
    }
    let mut out: Vec<BenchRecord> = best.values().map(|r| (*r).clone()).collect();
    out.sort_by(|a, b| {
        b.tok_per_sec
            .partial_cmp(&a.tok_per_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.elapsed_ms.cmp(&b.elapsed_ms))
    });
    out
}

/// Render the best-per-model comparison as a JSON array for QML:
/// [{"model":..,"tok_per_sec":..,"elapsed_ms":..,"output_chars":..,
///   "prompt_label":..,"ts":..}, ...] fastest first.
pub fn comparison_json(records: &[BenchRecord]) -> String {
    let best = best_per_model(records);
    let mut out = String::from("[");
    for (i, r) in best.iter().enumerate() {
        if i > 0 { out.push(','); }
        out.push_str(&format_record(r));
    }
    out.push(']');
    out
}

/// Compute tokens/sec from an output token estimate and elapsed ms.
/// Guards divide-by-zero (returns 0.0 for non-positive elapsed).
pub fn compute_tok_per_sec(output_tokens: u32, elapsed_ms: u32) -> f32 {
    if elapsed_ms == 0 { return 0.0; }
    (output_tokens as f32) * 1000.0 / (elapsed_ms as f32)
}

/// Rough token estimate from character count when an exact token
/// count isn't available (~4 chars/token for English, a common
/// heuristic). Used only for the tok/sec display when the backend
/// doesn't report token counts.
pub fn estimate_tokens_from_chars(chars: u32) -> u32 {
    // Round to nearest, min 1 if there's any output at all.
    if chars == 0 { return 0; }
    ((chars as f32 / 4.0).round() as u32).max(1)
}

// ── persistence (best-effort, mirrors token_log) ──

/// Append a record to the store at `path`. Best-effort; logs to
/// stderr on failure (a benchmark write should never crash the app).
pub fn append_to(path: &PathBuf, r: &BenchRecord) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = format_record(r);
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{}", line) {
                crate::clog!("model-bench", "write failed: {}", e);
            }
        }
        Err(e) => crate::clog!("model-bench", "open failed: {}", e),
    }
}

/// Load all records from the store at `path`. Missing file → empty.
pub fn load_from(path: &PathBuf) -> Vec<BenchRecord> {
    match std::fs::read_to_string(path) {
        Ok(c) => parse_all(&c),
        Err(_) => Vec::new(),
    }
}

/// The standard test prompts the playground offers. Short, fixed, and
/// representative of web2qml's real workload (summarize + route), so
/// benchmark numbers reflect actual use, not a synthetic stress test.
pub fn test_prompts() -> &'static [(&'static str, &'static str)] {
    // (label, prompt)
    TEST_PROMPTS
}

static TEST_PROMPTS: &[(&str, &str)] = &[
    ("summary",
     "Summarize the following in one sentence: The quarterly report \
      shows revenue up 12% with strong growth in the cloud segment, \
      though hardware sales declined slightly amid supply constraints."),
    ("route",
     "Classify this voice command into one action (navigate, search, \
      or terminal): \"open the settings page\""),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(model: &str, tps: f32, ms: u32) -> BenchRecord {
        BenchRecord {
            model: model.to_string(),
            tok_per_sec: tps,
            elapsed_ms: ms,
            output_chars: 100,
            prompt_label: "summary".to_string(),
            ts: "2026-05-24T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn round_trip_format_parse() {
        let r = rec("Llama-3.2-3B-Instruct-Q4_K_M.gguf", 87.34, 1145);
        let line = format_record(&r);
        let back = parse_record(&line).expect("should parse");
        assert_eq!(back.model, r.model);
        assert!((back.tok_per_sec - r.tok_per_sec).abs() < 0.01);
        assert_eq!(back.elapsed_ms, r.elapsed_ms);
        assert_eq!(back.output_chars, r.output_chars);
        assert_eq!(back.prompt_label, r.prompt_label);
        assert_eq!(back.ts, r.ts);
    }

    #[test]
    fn parse_tolerates_garbage_lines() {
        let blob = "not json\n{bad\n\n{\"model\":\"x\",\"tok_per_sec\":10.0,\
            \"elapsed_ms\":100,\"output_chars\":40,\"prompt_label\":\"s\",\
            \"ts\":\"t\"}\n";
        let all = parse_all(blob);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].model, "x");
    }

    #[test]
    fn parse_recovers_from_crash_truncated_tail() {
        // The real corruption mode for an append-only store: the
        // process crashed partway through writing the last line.
        // Good records before it must survive; the partial is dropped.
        let good = format_record(&rec("a", 50.0, 1000));
        let store = format!("{}\n{}\n{{\"model\":\"b\",\"tok_per_sec\":4",
            good, good);
        let all = parse_all(&store);
        assert_eq!(all.len(), 2, "good records before a truncated tail must survive");
        // trailing blank lines are ignored
        let store2 = format!("{}\n\n\n", good);
        assert_eq!(parse_all(&store2).len(), 1);
    }

    #[test]
    fn parse_handles_model_name_with_special_chars() {
        // Filenames have dots and dashes; make sure they survive.
        let r = rec("Qwen2.5-7B-Instruct-Q4_K_M.gguf", 45.0, 2000);
        let back = parse_record(&format_record(&r)).unwrap();
        assert_eq!(back.model, "Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    }

    #[test]
    fn best_per_model_keeps_fastest() {
        let records = vec![
            rec("a", 50.0, 1000),
            rec("a", 80.0, 600),   // faster — should win for "a"
            rec("a", 70.0, 700),
            rec("b", 30.0, 2000),
        ];
        let best = best_per_model(&records);
        assert_eq!(best.len(), 2);
        // sorted fastest-first → a (80) before b (30)
        assert_eq!(best[0].model, "a");
        assert!((best[0].tok_per_sec - 80.0).abs() < 0.01);
        assert_eq!(best[1].model, "b");
    }

    #[test]
    fn best_per_model_tie_breaks_on_elapsed() {
        let records = vec![
            rec("a", 50.0, 1200),
            rec("a", 50.0, 900),  // same tps, faster wall-clock → wins
        ];
        let best = best_per_model(&records);
        assert_eq!(best.len(), 1);
        assert_eq!(best[0].elapsed_ms, 900);
    }

    #[test]
    fn comparison_json_wellformed() {
        let records = vec![rec("a", 50.0, 1000), rec("b", 90.0, 500)];
        let json = comparison_json(&records);
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        // fastest (b, 90) should appear before a
        let pa = json.find("\"a\"").unwrap();
        let pb = json.find("\"b\"").unwrap();
        assert!(pb < pa, "faster model should be listed first");
        // balanced braces
        assert_eq!(json.matches('{').count(), json.matches('}').count());
    }

    #[test]
    fn tok_per_sec_math() {
        // 50 tokens in 1000ms = 50 tok/s.
        assert!((compute_tok_per_sec(50, 1000) - 50.0).abs() < 0.01);
        // 30 tokens in 500ms = 60 tok/s.
        assert!((compute_tok_per_sec(30, 500) - 60.0).abs() < 0.01);
        // divide-by-zero guard.
        assert_eq!(compute_tok_per_sec(50, 0), 0.0);
    }

    #[test]
    fn token_estimate_from_chars() {
        assert_eq!(estimate_tokens_from_chars(0), 0);
        assert_eq!(estimate_tokens_from_chars(4), 1);
        assert_eq!(estimate_tokens_from_chars(400), 100);
        // tiny non-zero output still counts as >=1 token
        assert_eq!(estimate_tokens_from_chars(2), 1);
    }

    #[test]
    fn append_and_load_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("web2qml_bench_test_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        append_to(&path, &rec("m1", 42.0, 1000));
        append_to(&path, &rec("m2", 88.0, 500));
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].model, "m1");
        assert_eq!(loaded[1].model, "m2");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let path = PathBuf::from("/nonexistent/web2qml/bench/nope.jsonl");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn test_prompts_present() {
        let p = test_prompts();
        assert!(!p.is_empty());
        // labels are non-empty and prompts are non-trivial
        for (label, prompt) in p {
            assert!(!label.is_empty());
            assert!(prompt.len() > 10);
        }
    }
}
