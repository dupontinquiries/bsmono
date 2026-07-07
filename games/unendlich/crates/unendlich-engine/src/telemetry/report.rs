//! Session report — the single structured line written to `events.jsonl` when
//! a session ends (or on a periodic flush).
//!
//! It bundles: snapshotted window stats, achievement coverage, named anomaly
//! counters, and the active non-default flags. The dashboard ingests this to
//! draw the achievement badges and the "what to test / what was tested" grid,
//! and to build a high-signal digest.
//!
//! JSON is hand-rolled (no `serde` dependency), the same approach as
//! `sim::net::wire`. The emitted object is always one line and always valid
//! JSON: non-finite floats serialize as `null`, strings are escaped.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::telemetry::achievements::Achievements;
use crate::telemetry::window::WindowStats;

#[derive(Clone, Debug, Default)]
pub struct SessionReport {
    pub kind: String, // always "session_report"
    pub ts: String,   // caller-supplied timestamp string (e.g. RFC3339)
    pub run_id: String,
    pub duration_s: f64,
    pub ticks: u64,
    pub windows: BTreeMap<String, WindowStats>,
    pub counters: BTreeMap<String, u64>,
    pub flags: Vec<String>,
    pub ach_unlocked: Vec<String>,
    pub ach_missing: Vec<String>,
    pub ach_total: usize,
}

impl SessionReport {
    pub fn new(ts: impl Into<String>, run_id: impl Into<String>, duration_s: f64, ticks: u64) -> Self {
        SessionReport {
            kind: "session_report".to_string(),
            ts: ts.into(),
            run_id: run_id.into(),
            duration_s,
            ticks,
            windows: BTreeMap::new(),
            counters: BTreeMap::new(),
            flags: Vec::new(),
            ach_unlocked: Vec::new(),
            ach_missing: Vec::new(),
            ach_total: 0,
        }
    }

    pub fn add_window(&mut self, name: &str, stats: WindowStats) {
        self.windows.insert(name.to_string(), stats);
    }

    pub fn counter(&mut self, name: &str, value: u64) {
        self.counters.insert(name.to_string(), value);
    }

    pub fn add_flags(&mut self, flags: &[String]) {
        self.flags.extend_from_slice(flags);
    }

    pub fn set_achievements(&mut self, a: &Achievements) {
        self.ach_unlocked = a.unlocked_ids().iter().map(|s| s.to_string()).collect();
        self.ach_missing = a.missing_ids().iter().map(|s| s.to_string()).collect();
        self.ach_total = a.coverage().1;
    }

    /// Serialize to a single line of JSON (no trailing newline).
    pub fn to_jsonl(&self) -> String {
        let mut s = String::with_capacity(512);
        s.push('{');
        kv_str(&mut s, "kind", &self.kind, true);
        kv_str(&mut s, "ts", &self.ts, false);
        kv_str(&mut s, "run_id", &self.run_id, false);
        s.push_str(",\"duration_s\":");
        push_num(&mut s, self.duration_s);
        s.push_str(",\"ticks\":");
        s.push_str(&self.ticks.to_string());

        // windows
        s.push_str(",\"windows\":{");
        for (i, (name, w)) in self.windows.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            push_jstr(&mut s, name);
            s.push(':');
            push_window(&mut s, w);
        }
        s.push('}');

        // counters
        s.push_str(",\"counters\":{");
        for (i, (name, v)) in self.counters.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            push_jstr(&mut s, name);
            s.push(':');
            s.push_str(&v.to_string());
        }
        s.push('}');

        // achievements
        s.push_str(",\"achievements\":{\"unlocked\":");
        push_str_array(&mut s, &self.ach_unlocked);
        s.push_str(",\"missing\":");
        push_str_array(&mut s, &self.ach_missing);
        s.push_str(",\"total\":");
        s.push_str(&self.ach_total.to_string());
        s.push('}');

        // flags
        s.push_str(",\"flags\":");
        push_str_array(&mut s, &self.flags);

        s.push('}');
        s
    }

    /// Append this report as one line to `path`, creating the file if needed.
    pub fn append_to(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let mut line = self.to_jsonl();
        line.push('\n');
        f.write_all(line.as_bytes())
    }
}

// ── JSON helpers (no deps) ────────────────────────────────────────────────

fn push_jstr(out: &mut String, s: &str) {
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

fn kv_str(out: &mut String, key: &str, val: &str, first: bool) {
    if !first {
        out.push(',');
    }
    push_jstr(out, key);
    out.push(':');
    push_jstr(out, val);
}

fn push_num(out: &mut String, v: f64) {
    if v.is_finite() {
        out.push_str(&format!("{}", v));
    } else {
        out.push_str("null");
    }
}

fn push_str_array(out: &mut String, items: &[String]) {
    out.push('[');
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_jstr(out, it);
    }
    out.push(']');
}

fn push_window(out: &mut String, w: &WindowStats) {
    out.push_str("{\"n\":");
    out.push_str(&w.n.to_string());
    out.push_str(",\"mean\":");
    push_num(out, w.mean);
    out.push_str(",\"p50\":");
    push_num(out, w.p50);
    out.push_str(",\"p75\":");
    push_num(out, w.p75);
    out.push_str(",\"p90\":");
    push_num(out, w.p90);
    out.push_str(",\"p99\":");
    push_num(out, w.p99);
    out.push_str(",\"min\":");
    push_num(out, w.min);
    out.push_str(",\"max\":");
    push_num(out, w.max);
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::window::Window;

    #[test]
    fn report_is_single_line_well_formed_json() {
        let mut w = Window::new(15.0);
        for i in 0..100 {
            w.push(i as f64 / 60.0, (i % 5) as f64 * 0.5);
        }
        let mut a = Achievements::new();
        a.register_all(&[("placed_block", "Place", "build"), ("crafted_item", "Craft", "items")]);
        a.unlock("placed_block", 42);

        let mut r = SessionReport::new("2026-05-24T12:52:35Z", "r-abc", 123.4, 7400);
        r.add_window("gpu_total", w.stats().unwrap());
        r.counter("skipped_frames", 0);
        r.counter("mesher_over_budget", 3);
        r.add_flags(&["GPU_MESHER=1".to_string()]);
        r.set_achievements(&a);

        let line = r.to_jsonl();
        // single line
        assert!(!line.contains('\n'));
        // balanced braces/brackets
        let braces = line.chars().filter(|&c| c == '{').count() as i64
            - line.chars().filter(|&c| c == '}').count() as i64;
        let brackets = line.chars().filter(|&c| c == '[').count() as i64
            - line.chars().filter(|&c| c == ']').count() as i64;
        assert_eq!(braces, 0, "unbalanced braces in {line}");
        assert_eq!(brackets, 0, "unbalanced brackets in {line}");
        // key presence
        for needle in [
            "\"kind\":\"session_report\"",
            "\"gpu_total\"",
            "\"placed_block\"",
            "\"missing\":[\"crafted_item\"]",
            "\"total\":2",
            "\"GPU_MESHER=1\"",
        ] {
            assert!(line.contains(needle), "missing {needle} in {line}");
        }
    }

    #[test]
    fn escapes_and_nonfinite() {
        let mut r = SessionReport::new("t\"s", "r\\1", f64::NAN, 0);
        let mut w = WindowStats { n: 1, mean: f64::INFINITY, p50: 0.0, p75: 0.0, p90: 0.0, p99: 0.0, min: 0.0, max: 0.0 };
        w.mean = f64::INFINITY;
        r.add_window("w", w);
        let line = r.to_jsonl();
        assert!(line.contains("\"duration_s\":null"));
        assert!(line.contains("\"mean\":null"));
        assert!(line.contains("\\\"")); // escaped quote from ts
    }
}
