//! Session telemetry: live rolling-window stats, achievement/coverage
//! tracking, and a structured `events.jsonl` session report.
//!
//! Pure `std`, no external crates and no engine-render dependencies — the same
//! discipline as [`crate::sim::net`]. The in-game egui overlay reads
//! [`Telemetry::live`] for the live 15s HUD; the dashboard ingests the
//! [`SessionReport`] line for badges and digests.
//!
//! Typical wiring (in the bin/launcher):
//! ```ignore
//! let mut tel = Telemetry::new();
//! tel.window("gpu_total", 15.0);
//! tel.register_specs(default_test_spec());
//! // per frame:
//! tel.push("gpu_total", now_secs, gpu_total_ms);
//! // on gameplay events:
//! tel.unlock("placed_block", sim_tick);
//! tel.bump("mesher_over_budget"); // when a frame blows the mesh budget
//! // on exit:
//! let mut report = tel.build_report(ts, run_id, duration_s, ticks, active_flags);
//! report.append_to("events.jsonl").ok();
//! ```

pub mod achievements;
pub mod report;
pub mod window;

pub use achievements::{Achievement, Achievements};
pub use report::SessionReport;
pub use window::{Window, WindowStats};

use std::collections::BTreeMap;

/// A reasonable default test-spec: `(id, label, category)`. Swap this for a
/// list loaded from your own spec files; the dashboard renders one badge per
/// entry. Kept here so the system is useful out of the box.
pub fn default_test_spec() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("placed_block", "Place a block", "build"),
        ("broke_block", "Break a block", "build"),
        ("picked_up_drop", "Pick up a dropped item", "items"),
        ("crafted_item", "Craft something", "items"),
        ("scrolled_hotbar", "Scroll the hotbar", "items"),
        ("entered_fluid", "Stand in water or lava", "physics"),
        ("took_fall", "Land after a fall", "physics"),
        ("jumped", "Jump", "physics"),
        ("opened_debug_hud", "Toggle the debug HUD (F4)", "debug"),
        ("mesher_over_budget", "Hit a mesher stall", "perf"),
        ("frame_skipped", "Drop a frame", "perf"),
        ("desync_detected", "Trigger a net desync", "net"),
    ]
}

/// Ties windows, achievements, and counters together for a session.
#[derive(Default)]
pub struct Telemetry {
    windows: BTreeMap<String, Window>,
    ach: Achievements,
    counters: BTreeMap<String, u64>,
}

impl Telemetry {
    pub fn new() -> Self {
        Telemetry::default()
    }

    /// Register a named rolling window of `secs` seconds (e.g. `15.0`).
    pub fn window(&mut self, name: &str, secs: f64) {
        self.windows.insert(name.to_string(), Window::new(secs));
    }

    /// Feed a sample to a named window. No-op if the window wasn't registered.
    pub fn push(&mut self, name: &str, now_secs: f64, value: f64) {
        if let Some(w) = self.windows.get_mut(name) {
            w.push(now_secs, value);
        }
    }

    /// Current stats for a named window — for the live overlay.
    pub fn live(&self, name: &str) -> Option<WindowStats> {
        self.windows.get(name).and_then(|w| w.stats())
    }

    pub fn register_specs(&mut self, specs: Vec<(&str, &str, &str)>) {
        for (id, label, cat) in specs {
            self.ach.register(id, label, cat);
        }
    }

    /// Mark an achievement done at `tick`. Returns true on first unlock.
    pub fn unlock(&mut self, id: &str, tick: u64) -> bool {
        self.ach.unlock(id, tick)
    }

    pub fn is_unlocked(&self, id: &str) -> bool {
        self.ach.is_unlocked(id)
    }

    pub fn coverage(&self) -> (usize, usize) {
        self.ach.coverage()
    }

    /// Increment a named anomaly counter.
    pub fn bump(&mut self, name: &str) {
        let c = self.counters.entry(name.to_string()).or_insert(0);
        *c = c.saturating_add(1);
    }

    pub fn set_counter(&mut self, name: &str, value: u64) {
        self.counters.insert(name.to_string(), value);
    }

    pub fn achievements(&self) -> &Achievements {
        &self.ach
    }

    /// Snapshot everything into a serializable report.
    pub fn build_report(
        &self,
        ts: impl Into<String>,
        run_id: impl Into<String>,
        duration_s: f64,
        ticks: u64,
        flags: &[String],
    ) -> SessionReport {
        let mut r = SessionReport::new(ts, run_id, duration_s, ticks);
        for (name, w) in &self.windows {
            if let Some(s) = w.stats() {
                r.add_window(name, s);
            }
        }
        for (name, v) in &self.counters {
            r.counter(name, *v);
        }
        r.add_flags(flags);
        r.set_achievements(&self.ach);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_session() {
        let mut tel = Telemetry::new();
        tel.window("gpu_total", 15.0);
        tel.register_specs(default_test_spec());
        for i in 0..300 {
            tel.push("gpu_total", i as f64 / 60.0, 2.0 + (i % 7) as f64 * 0.1);
        }
        tel.unlock("placed_block", 50);
        tel.unlock("jumped", 60);
        tel.bump("mesher_over_budget");
        tel.bump("mesher_over_budget");

        assert!(tel.live("gpu_total").is_some());
        let (got, total) = tel.coverage();
        assert_eq!(got, 2);
        assert!(total >= 12);

        let report = tel.build_report("2026-05-24T00:00:00Z", "r-test", 5.0, 300, &["GPU_MESHER=1".to_string()]);
        let line = report.to_jsonl();
        assert!(line.contains("\"gpu_total\""));
        assert!(line.contains("\"placed_block\""));
        assert!(line.contains("\"mesher_over_budget\":2"));
    }
}
