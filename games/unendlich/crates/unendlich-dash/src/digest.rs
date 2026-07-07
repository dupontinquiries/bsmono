//! Builds the "copy for Claude" digest — a compact text summary of the selected
//! run(s) that pastes cleanly back into a chat.

use crate::model::{Run, Telemetry};

fn run_block(tel: &Telemetry, run: &Run) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "RUN {}  v{}  dur={}\n",
        run.uuid.as_deref().unwrap_or("?"),
        run.version.as_deref().unwrap_or("?"),
        run.duration.as_deref().unwrap_or("?"),
    ));
    if let Some(env) = &run.env {
        s.push_str(&format!("  env: {env}\n"));
    }
    if !run.gpu.is_empty() {
        s.push_str("  GPU pass (p50/p99/max ms):\n");
        for m in &run.gpu {
            s.push_str(&format!(
                "    {:8} p50={:.2} p99={:.2} max={:.2}{}\n",
                m.stage,
                m.p50,
                m.p99,
                m.max,
                m.over_n.map(|n| format!("  over={n}")).unwrap_or_default(),
            ));
        }
    }
    if !run.mesher.is_empty() {
        s.push_str("  Mesher (p50/p99/max ms):\n");
        for m in &run.mesher {
            s.push_str(&format!("    {:8} p50={:.2} p99={:.2} max={:.2}\n", m.stage, m.p50, m.p99, m.max));
        }
    }
    if !run.workload.is_empty() {
        s.push_str("  Workload (chunks p50/max):\n");
        for w in &run.workload {
            s.push_str(&format!("    {:10} p50={} max={}\n", w.kind, w.p50, w.max));
        }
    }
    if !run.flags.is_empty() {
        s.push_str("  Flags:\n");
        for (k, v) in &run.flags {
            s.push_str(&format!("    {k} = {v}\n"));
        }
    }
    let a = tel.achievements(run);
    if !a.is_empty() {
        s.push_str(&format!("  Signals: {}\n", a.join(", ")));
    }
    s
}

pub fn digest(tel: &Telemetry, selected: &[usize]) -> String {
    let mut s = String::from("# unendlich telemetry digest\n\n");
    if selected.is_empty() {
        s.push_str("(no run selected)\n");
        return s;
    }
    for &i in selected {
        if let Some(run) = tel.runs.get(i) {
            s.push_str(&run_block(tel, run));
            s.push('\n');
        }
    }
    if selected.len() == 2 {
        if let (Some(a), Some(b)) = (tel.runs.get(selected[0]), tel.runs.get(selected[1])) {
            if let (Some(ta), Some(tb)) = (a.gpu_total(), b.gpu_total()) {
                s.push_str(&format!(
                    "DELTA total p99: {:.2} → {:.2} ms ({:+.2})\n",
                    ta.p99,
                    tb.p99,
                    tb.p99 - ta.p99
                ));
            }
        }
    }
    s
}
