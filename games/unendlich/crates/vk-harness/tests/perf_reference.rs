//! Guards the persistent perf reference table (`perf/reference.jsonl`).
//!
//! Every non-blank line must be a single JSON object carrying the documented
//! required keys, so the accumulating hardware + lavapipe rows can't silently
//! rot as new ones are appended. Dependency-free structural check (no serde):
//! we don't fully parse JSON, we assert the shape + required field presence and
//! that p50 is numeric — enough to catch a malformed paste.

use std::path::PathBuf;

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("perf/reference.jsonl")
}

/// Extract the string value of `"key":"..."` or numeric `"key":N` if present.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let i = line.find(&pat)? + pat.len();
    let rest = line[i..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(&stripped[..end])
    } else {
        // numeric / bool / object: read until , or } or end
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

#[test]
fn reference_rows_are_well_formed() {
    let text = std::fs::read_to_string(reference_path()).expect("reference.jsonl must exist");
    let mut rows = 0usize;
    let (mut hw, mut lp) = (0usize, 0usize);
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let n = lineno + 1;
        assert!(line.starts_with('{') && line.ends_with('}'), "line {n}: not a JSON object");

        let source = field(line, "source").unwrap_or_else(|| panic!("line {n}: missing source"));
        assert!(source == "hardware" || source == "lavapipe", "line {n}: bad source {source:?}");

        let date = field(line, "date").unwrap_or_else(|| panic!("line {n}: missing date"));
        assert!(date.len() == 10 && date.as_bytes()[4] == b'-', "line {n}: bad date {date:?}");

        let unit = field(line, "unit").unwrap_or_else(|| panic!("line {n}: missing unit"));
        assert!(unit == "ms" || unit == "us", "line {n}: bad unit {unit:?}");

        let p50 = field(line, "p50").unwrap_or_else(|| panic!("line {n}: missing p50"));
        p50.parse::<f64>().unwrap_or_else(|_| panic!("line {n}: p50 not numeric: {p50:?}"));

        match source {
            "hardware" => {
                field(line, "device").unwrap_or_else(|| panic!("line {n}: hardware row needs device"));
                field(line, "metric").unwrap_or_else(|| panic!("line {n}: hardware row needs metric"));
                hw += 1;
            }
            "lavapipe" => {
                field(line, "kernel").unwrap_or_else(|| panic!("line {n}: lavapipe row needs kernel"));
                lp += 1;
            }
            _ => unreachable!(),
        }
        rows += 1;
    }
    assert!(rows >= 10, "expected the seeded rows to be present, found {rows}");
    assert!(hw > 0 && lp > 0, "table should hold BOTH hardware ({hw}) and lavapipe ({lp}) rows");
    eprintln!("perf reference: {rows} rows ({hw} hardware, {lp} lavapipe) — well-formed");
}

#[test]
fn field_extractor_self_check() {
    let l = r#"{"source":"lavapipe","date":"2026-05-30","unit":"us","kernel":"k","p50":438.6,"n":64}"#;
    assert_eq!(field(l, "source"), Some("lavapipe"));
    assert_eq!(field(l, "kernel"), Some("k"));
    assert_eq!(field(l, "p50"), Some("438.6"));
    assert_eq!(field(l, "n"), Some("64"));
    assert_eq!(field(l, "missing"), None);
}
