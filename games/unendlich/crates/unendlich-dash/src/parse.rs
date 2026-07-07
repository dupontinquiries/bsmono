//! Parsers for `run_summaries.txt` and `events.jsonl`.
//!
//! The regexes and run-boundary logic here are a verbatim port of the version
//! validated in Python against a realistic two-run file (framed `==== RUN
//! START/END ====` blocks + console-prefixed metric bodies). Sections are
//! inferred from metric *names* (mesher stages vs GPU passes) rather than
//! headers, which makes it robust to both the framed file and raw console
//! pastes.

use crate::model::{Compression, Event, Memory, Metric, Run, TickApply, Workload};
use regex::Regex;
use std::sync::OnceLock;

struct Res {
    log_prefix: Regex,
    metric: Regex,
    workload: Regex,
    flag: Regex,
    run_start: Regex,
    run_end: Regex,
    version: Regex,
    header: Regex,
    memory: Regex,
    compression: Regex,
    tick_apply: Regex,
}

fn res() -> &'static Res {
    static R: OnceLock<Res> = OnceLock::new();
    R.get_or_init(|| Res {
        log_prefix: Regex::new(r"^\[([0-9T:\-Z]+)\s+\w+\s+(r-[0-9a-f]+)\s+unendlich_world\]\s*(.*)$").unwrap(),
        metric: Regex::new(r"^\s*([a-z_]+)\s+n=\s*(\d+)\s+mean=\s*([\d.]+)ms±?\s*([\d.]+)?\s+p50=\s*([\d.]+)\s+p90=\s*([\d.]+)\s+p99=\s*([\d.]+)\s+min=\s*([\d.]+)\s+max=\s*([\d.]+)\s*(?:over_([\d.]+)ms=\s*(\d+)\s*\(([\d.]+)%\))?\s*$").unwrap(),
        workload: Regex::new(r"^\s*(loaded|rendering)\s+n=\s*(\d+)\s+min=\s*(\d+)\s+p50=\s*(\d+)\s+p90=\s*(\d+)\s+max=\s*(\d+)\s*$").unwrap(),
        flag: Regex::new(r"^\s*((?:UNENDLICH_[A-Z_]+)|RUST_LOG)\s*=\s*(.+?)\s*$").unwrap(),
        run_start: Regex::new(r"^==== RUN START\s+(\S+)\s+====").unwrap(),
        run_end: Regex::new(r"^==== RUN END\s+(\S+)\s+====").unwrap(),
        version: Regex::new(r"^==== VERSION\s+(\S+)\s+START\s+(\S+)\s+====").unwrap(),
        header: Regex::new(r"^\s*(version|uuid|duration|env):\s*(.+?)\s*$").unwrap(),
        memory: Regex::new(r"^\s*memory:\s+(.+)$").unwrap(),
        compression: Regex::new(r"^\s*compression:\s+(.+)$").unwrap(),
        tick_apply: Regex::new(r"^\s*tick_apply:\s+(.+)$").unwrap(),
    })
}

fn is_mesher(stage: &str) -> bool {
    matches!(stage, "near" | "mid" | "far")
}

fn f(s: &str) -> f64 {
    s.parse().unwrap_or(0.0)
}
fn u(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

pub fn parse_runs(text: &str) -> Vec<Run> {
    let r = res();
    let mut runs: Vec<Run> = Vec::new();
    let mut cur: Option<Run> = None;
    let mut cur_ver: Option<String> = None;
    let mut cur_id: Option<String> = None;

    macro_rules! push {
        () => {
            if let Some(run) = cur.take() {
                runs.push(run);
            }
        };
    }

    for raw in text.lines() {
        if let Some(c) = r.version.captures(raw) {
            cur_ver = Some(c[1].to_string());
            continue;
        }
        if let Some(c) = r.run_start.captures(raw) {
            push!();
            let mut run = Run::default();
            run.start = Some(c[1].to_string());
            cur = Some(run);
            cur_id = None;
            continue;
        }
        if let Some(c) = r.run_end.captures(raw) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            if let Some(run) = cur.as_mut() {
                run.end = Some(c[1].to_string());
            }
            push!();
            cur_id = None;
            continue;
        }

        // Strip an optional console log prefix to get the body + run id.
        let mut body = raw;
        if let Some(c) = r.log_prefix.captures(raw) {
            let rid = c[2].to_string();
            body_set(&mut body, c.get(3).map(|m| m.as_str()).unwrap_or(""));
            // A run-id change while the current run already has data is a
            // boundary (handles console pastes with no framing).
            let has_data = cur.as_ref().map(|x| !x.gpu.is_empty() || !x.mesher.is_empty()).unwrap_or(false);
            if cur_id.is_some() && cur_id.as_deref() != Some(rid.as_str()) && has_data {
                push!();
            }
            if cur.is_none() {
                cur = Some(Run::default());
            }
            if let Some(run) = cur.as_mut() {
                if run.uuid.is_none() {
                    run.uuid = Some(rid.clone());
                }
            }
            cur_id = Some(rid);
        }

        if let Some(c) = r.header.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            let run = cur.as_mut().unwrap();
            let v = c[2].to_string();
            match &c[1] {
                "version" => run.version = Some(v),
                "uuid" => run.uuid = Some(v),
                "duration" => run.duration = Some(v),
                "env" => run.env = Some(v),
                _ => {}
            }
            continue;
        }

        if let Some(c) = r.metric.captures(body) {
            let stage = c[1].to_string();
            // A second leading `cull` with no id signal = a new run.
            if stage == "cull" {
                if let Some(run) = cur.as_ref() {
                    if run.gpu.iter().any(|m| m.stage == "cull") {
                        push!();
                    }
                }
            }
            if cur.is_none() {
                cur = Some(Run::default());
            }
            let m = Metric {
                stage: stage.clone(),
                n: u(&c[2]),
                mean: f(&c[3]),
                std: c.get(4).map(|x| f(x.as_str())),
                p50: f(&c[5]),
                p90: f(&c[6]),
                p99: f(&c[7]),
                min: f(&c[8]),
                max: f(&c[9]),
                over_ms: c.get(10).map(|x| f(x.as_str())),
                over_n: c.get(11).map(|x| u(x.as_str())),
                over_pct: c.get(12).map(|x| f(x.as_str())),
            };
            let run = cur.as_mut().unwrap();
            if is_mesher(&stage) {
                run.mesher.push(m);
            } else {
                run.gpu.push(m);
            }
            continue;
        }

        if let Some(c) = r.workload.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            cur.as_mut().unwrap().workload.push(Workload {
                kind: c[1].to_string(),
                n: u(&c[2]),
                min: u(&c[3]),
                p50: u(&c[4]),
                p90: u(&c[5]),
                max: u(&c[6]),
            });
            continue;
        }

        if let Some(c) = r.memory.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            let mut mem = Memory::default();
            for tok in c[1].split_whitespace() {
                if let Some((k, v)) = tok.split_once('=') {
                    let n = u(v);
                    match k {
                        "octree_bytes" => mem.octree_bytes = n,
                        "octree_capacity" => mem.octree_capacity = n,
                        "octree_chunks" => mem.octree_chunks = n,
                        "indirect_slots_cap" => mem.indirect_slots_cap = n,
                        "gpu_oom_fallback" => mem.gpu_oom_fallback = n,
                        "lod_near" => mem.lod_near = n,
                        "lod_mid" => mem.lod_mid = n,
                        "lod_far" => mem.lod_far = n,
                        "lod_vfar" => mem.lod_vfar = n,
                        _ => {}
                    }
                }
            }
            cur.as_mut().unwrap().memory = Some(mem);
            continue;
        }

        if let Some(c) = r.compression.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            let mut cp = Compression::default();
            for tok in c[1].split_whitespace() {
                if let Some((k, v)) = tok.split_once('=') {
                    match k {
                        "chunks" => cp.chunks = u(v),
                        "raw_bytes" => cp.raw_bytes = u(v),
                        "surface_run_bytes" => cp.surface_run_bytes = u(v),
                        "ratio" => cp.ratio = f(v),
                        "worst_col_runs" => cp.worst_col_runs = u(v),
                        _ => {}
                    }
                }
            }
            cur.as_mut().unwrap().compression = Some(cp);
            continue;
        }

        if let Some(c) = r.tick_apply.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            let mut ta = TickApply::default();
            for tok in c[1].split_whitespace() {
                if let Some((k, v)) = tok.split_once('=') {
                    match k {
                        "n" => ta.n = u(v),
                        "p50_ms" => ta.p50_ms = f(v),
                        "p90_ms" => ta.p90_ms = f(v),
                        "p99_ms" => ta.p99_ms = f(v),
                        "max_ms" => ta.max_ms = f(v),
                        "max_chunks" => ta.max_chunks = u(v),
                        _ => {}
                    }
                }
            }
            cur.as_mut().unwrap().tick_apply = Some(ta);
            continue;
        }

        if let Some(c) = r.flag.captures(body) {
            if cur.is_none() {
                cur = Some(Run::default());
            }
            cur.as_mut().unwrap().flags.push((c[1].to_string(), c[2].to_string()));
            continue;
        }
    }
    push!();

    for run in runs.iter_mut() {
        if run.version.is_none() {
            run.version = cur_ver.clone();
        }
    }
    runs
}

/// Helper to reassign a `&str` body to a sub-slice of the original line.
/// (Borrow gymnastics: we keep `body` pointing into `raw`'s captured group.)
fn body_set<'a>(body: &mut &'a str, s: &'a str) {
    *body = s;
}

pub fn parse_events(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let s = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(|x| x.to_string()))
        };
        let num = |keys: &[&str]| -> Option<f64> {
            keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64()))
        };
        out.push(Event {
            uuid: s(&["uuid", "run", "run_id"]),
            kind: s(&["kind", "type", "event"]),
            ts: num(&["ts_ms", "ts", "t"]),
            ms: num(&["ms", "frame_ms", "mean_ms", "mean", "value"]),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Inlined fixtures (formerly sample/*, removed). Byte-exact so the asserted
    // values below stay valid.
    const RUNS: &str = r#"======================================================================
==== VERSION 0.24.2-c.2 START 2026-05-24T12:00:00Z ====
======================================================================

==== RUN START 2026-05-24T12:30:00Z ====
version: 0.24.2-c.2
uuid: r-3e506d452ff42d57
duration: 251.6s
env: UNENDLICH_GPU_MESHER=1
---- summaries ----
[2026-05-24T12:54:51Z INFO r-3e506d452ff42d57 unendlich_world] GPU pass timer lifetime summary:
  cull    n= 15095  mean=   0.04ms±  0.01  p50=  0.04  p90=  0.05  p99=  0.05  min=  0.00  max=   0.26  over_5.0ms=0 (0.0%)
  froxel  n= 15095  mean=   0.13ms±  0.01  p50=  0.13  p90=  0.13  p99=  0.14  min=  0.00  max=   0.38  over_5.0ms=0 (0.0%)
  main    n= 15095  mean=   0.45ms±  0.13  p50=  0.45  p90=  0.63  p99=  0.71  min=  0.00  max=   0.95  over_8.3ms=0 (0.0%)
  shadow  n= 15095  mean=   0.57ms±  0.11  p50=  0.58  p90=  0.70  p99=  0.77  min=  0.00  max=   4.53  over_8.3ms=0 (0.0%)
  total   n= 15095  mean=   1.19ms±  0.20  p50=  1.21  p90=  1.41  p99=  1.59  min=  0.00  max=   5.31  over_8.3ms=0 (0.0%)
[2026-05-24T12:54:51Z INFO r-3e506d452ff42d57 unendlich_world] Mesher timer lifetime summary (per LOD):
  near  n=  7250  mean=   0.13ms±  0.13  p50=  0.11  p90=  0.27  p99=  0.45  min=  0.00  max=   2.14
[2026-05-24T12:54:51Z INFO r-3e506d452ff42d57 unendlich_world] Flags in effect (binary=unendlich-world):
  UNENDLICH_GPU_MESHER               = 1
  UNENDLICH_VSYNC                    = (default: 1)
  loaded     n=  50  min=    4  p50=  147  p90=  231  max=  256
  rendering  n=  50  min=    5  p50=  112  p90=  192  max=  210
==== RUN END 2026-05-24T12:54:51Z ====

==== RUN START 2026-05-24T13:00:00Z ====
version: 0.24.2-c.2
uuid: r-aa11bb22cc33dd44
duration: 60.0s
env: UNENDLICH_RENDER_DISTANCE=32
---- summaries ----
[2026-05-24T13:01:00Z INFO r-aa11bb22cc33dd44 unendlich_world] GPU pass timer lifetime summary:
  cull    n=  3600  mean=   0.06ms±  0.02  p50=  0.05  p90=  0.08  p99=  0.11  min=  0.00  max=   0.40  over_5.0ms=0 (0.0%)
  main    n=  3600  mean=   0.80ms±  0.30  p50=  0.75  p90=  1.20  p99=  2.10  min=  0.00  max=   9.10  over_8.3ms=2 (0.1%)
  total   n=  3600  mean=   2.05ms±  0.55  p50=  1.98  p90=  2.80  p99=  4.20  min=  0.00  max=  11.20  over_8.3ms=5 (0.1%)
  near  n=  1800  mean=   0.40ms±  0.30  p50=  0.35  p90=  0.70  p99=  1.30  min=  0.00  max=   5.00
  UNENDLICH_RENDER_DISTANCE           = 32
==== RUN END 2026-05-24T13:01:00Z ====
"#;
    const EVENTS: &str = r#"{"uuid":"r-3e506d452ff42d57","ts_ms":1000,"kind":"startup"}
{"uuid":"r-3e506d452ff42d57","ts_ms":2000,"kind":"frame_window","ms":1.21}
{"uuid":"r-3e506d452ff42d57","ts_ms":3000,"kind":"frame_window","ms":1.30}
{"uuid":"r-3e506d452ff42d57","ts_ms":4000,"kind":"wait_fences_spike","ms":12.0}
{"uuid":"r-3e506d452ff42d57","ts_ms":5000,"kind":"frame_window","ms":1.25}
{"uuid":"r-3e506d452ff42d57","ts_ms":6000,"kind":"shutdown"}
{"uuid":"r-aa11bb22cc33dd44","ts_ms":1000,"kind":"frame_window","ms":2.0}
{"uuid":"r-aa11bb22cc33dd44","ts_ms":2000,"kind":"gpu_spike","ms":9.1}
{"uuid":"r-aa11bb22cc33dd44","ts_ms":3000,"kind":"frame_window","ms":2.8}
"#;

    #[test]
    fn parses_two_runs_with_correct_stages_and_values() {
        let runs = parse_runs(RUNS);
        assert_eq!(runs.len(), 2, "should detect exactly two runs");
        let r0 = &runs[0];
        assert_eq!(r0.uuid.as_deref(), Some("r-3e506d452ff42d57"));
        let stages: Vec<&str> = r0.gpu.iter().map(|m| m.stage.as_str()).collect();
        assert_eq!(stages, ["cull", "froxel", "main", "shadow", "total"]);
        assert_eq!(r0.mesher.iter().map(|m| m.stage.as_str()).collect::<Vec<_>>(), ["near"]);
        assert_eq!(r0.workload.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["loaded", "rendering"]);
        let total = r0.gpu_total().unwrap();
        assert!((total.p99 - 1.59).abs() < 1e-9 && (total.max - 5.31).abs() < 1e-9);
        assert_eq!(r0.flag("UNENDLICH_GPU_MESHER"), Some("1"));
        assert_eq!(r0.version.as_deref(), Some("0.24.2-c.2"));
        // version-marker inheritance reaches run 1 too
        assert_eq!(runs[1].version.as_deref(), Some("0.24.2-c.2"));
    }

    #[test]
    fn events_correlate_and_drive_achievements() {
        let tel = crate::model::Telemetry {
            runs: parse_runs(RUNS),
            events: parse_events(EVENTS),
        };
        let r0 = &tel.runs[0];
        assert_eq!(tel.frame_curve(r0).len(), 3);
        let a0 = tel.achievements(r0);
        assert!(a0.contains(&"clean-exit".to_string()));
        assert!(a0.contains(&"gpu-mesher".to_string()));
        assert!(a0.contains(&"sustained".to_string()));
        let a1 = tel.achievements(&tel.runs[1]);
        assert!(a1.contains(&"crash".to_string()) && a1.contains(&"spikes".to_string()));
    }
}
