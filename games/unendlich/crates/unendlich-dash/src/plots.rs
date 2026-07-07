//! Chart builders. Kept to just `Line` and `BarChart`/`Bar`, whose 0.34/0.35
//! signatures are confirmed: `Line::new(name, points)`, `BarChart::new(name,
//! bars)`, `Bar::new(x, height)`.

use crate::model::Run;
use egui_plot::{Bar, BarChart, Line, PlotPoints};

/// One bar per GPU stage, height = p99 ms. Hover shows the stage name.
pub fn gpu_p99_bars(run: &Run) -> Vec<Bar> {
    run.gpu
        .iter()
        .enumerate()
        .map(|(i, m)| Bar::new(i as f64, m.p99).width(0.7).name(format!("{} p99", m.stage)))
        .collect()
}

pub fn gpu_p99_chart(run: &Run) -> BarChart {
    BarChart::new("p99 ms", gpu_p99_bars(run))
}

/// A horizontal budget line spanning the bar range.
pub fn budget_line(run: &Run, budget_ms: f64) -> Line<'static> {
    let n = run.gpu.len().max(1) as f64;
    let pts: PlotPoints = vec![[-0.5, budget_ms], [n - 0.5, budget_ms]].into_iter().collect();
    Line::new(format!("{budget_ms} ms budget"), pts)
}

/// The frame-time curve (seconds → ms).
pub fn frame_line(curve: &[[f64; 2]]) -> Line<'static> {
    let pts: PlotPoints = curve.iter().copied().collect();
    Line::new("frame ms", pts)
}

/// Spike markers drawn as short vertical segments (Line-only, no Points dep).
pub fn spike_lines(spikes: &[[f64; 2]]) -> Vec<Line<'static>> {
    spikes
        .iter()
        .map(|p| {
            let pts: PlotPoints = vec![[p[0], 0.0], [p[0], p[1]]].into_iter().collect();
            Line::new("spike", pts)
        })
        .collect()
}

// ── Trend over run history (the "watch the webs shrink" view) ───────────────
//
// Instead of overlaying two runs' box plots, these extract one metric's
// trajectory across ALL runs in chronological (parse) order, as (run_index,
// value_ms) points. Plotting p50 + p90 + p99 as three lines makes the spread
// between them — the "web" — visible *over time*: as the engine improves both
// the median drops AND p99 collapses toward p50 (the web tightens).

/// Which percentile of a metric to trend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pctl {
    P50,
    P90,
    P99,
    Max,
}

impl Pctl {
    pub fn label(self) -> &'static str {
        match self {
            Pctl::P50 => "p50",
            Pctl::P90 => "p90",
            Pctl::P99 => "p99",
            Pctl::Max => "max",
        }
    }
}

/// Trajectory of one GPU stage's percentile across `runs`, as (run_index,
/// value_ms). Runs missing the stage are skipped, but the x index still tracks
/// the run's position so the timeline stays aligned to run order.
pub fn gpu_stage_trend(runs: &[Run], stage: &str, pct: Pctl) -> Vec<[f64; 2]> {
    runs.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            r.gpu.iter().find(|m| m.stage == stage).map(|m| {
                let v = match pct {
                    Pctl::P50 => m.p50,
                    Pctl::P90 => m.p90,
                    Pctl::P99 => m.p99,
                    Pctl::Max => m.max,
                };
                [i as f64, v]
            })
        })
        .collect()
}

/// Trajectory of `tick_apply` max_ms (the loading-wave hitch) across runs.
pub fn tick_apply_max_trend(runs: &[Run]) -> Vec<[f64; 2]> {
    runs.iter()
        .enumerate()
        .filter_map(|(i, r)| r.tick_apply.as_ref().map(|t| [i as f64, t.max_ms]))
        .collect()
}

/// Trajectory of the near-LOD chunk count across runs — the geometry load that
/// drives shadow+main cost. Watching it fall is the LOD-tuning win made visible.
pub fn lod_near_trend(runs: &[Run]) -> Vec<[f64; 2]> {
    runs.iter()
        .enumerate()
        .filter_map(|(i, r)| r.memory.as_ref().map(|m| [i as f64, m.lod_near as f64]))
        .collect()
}

/// Build a trend `Line` from extracted points.
pub fn trend_line(name: impl Into<String>, pts: Vec<[f64; 2]>) -> Line<'static> {
    let p: PlotPoints = pts.into_iter().collect();
    Line::new(name.into(), p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Memory, Metric, Run, TickApply};

    fn run_with_total(p50: f64, p99: f64) -> Run {
        Run {
            gpu: vec![Metric { stage: "total".into(), p50, p99, ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn gpu_trend_tracks_run_order_and_skips_missing() {
        let runs = vec![
            run_with_total(10.0, 40.0),
            Run::default(), // no gpu metrics → skipped, but index advances
            run_with_total(3.0, 12.0),
        ];
        let p50 = gpu_stage_trend(&runs, "total", Pctl::P50);
        assert_eq!(p50, vec![[0.0, 10.0], [2.0, 3.0]], "missing run skipped, x index preserved");
        let p99 = gpu_stage_trend(&runs, "total", Pctl::P99);
        assert_eq!(p99, vec![[0.0, 40.0], [2.0, 12.0]]);
    }

    #[test]
    fn trends_show_the_web_shrinking() {
        // p99 - p50 (the "web" width) should narrow as the engine improves.
        let runs = vec![run_with_total(10.0, 40.0), run_with_total(3.0, 12.0)];
        let p50 = gpu_stage_trend(&runs, "total", Pctl::P50);
        let p99 = gpu_stage_trend(&runs, "total", Pctl::P99);
        let web0 = p99[0][1] - p50[0][1]; // 30
        let web1 = p99[1][1] - p50[1][1]; // 9
        assert!(web1 < web0, "web should shrink: {web1} !< {web0}");
    }

    #[test]
    fn tick_apply_and_lod_trends_extract() {
        let mut r = Run::default();
        r.tick_apply = Some(TickApply { max_ms: 274.0, ..Default::default() });
        r.memory = Some(Memory { lod_near: 1834, ..Default::default() });
        let runs = vec![r];
        assert_eq!(tick_apply_max_trend(&runs), vec![[0.0, 274.0]]);
        assert_eq!(lod_near_trend(&runs), vec![[0.0, 1834.0]]);
    }
}
