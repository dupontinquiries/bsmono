//! Data model for parsed telemetry. Mirrors the HTML dashboard's structures so
//! the two tools stay in lockstep.

/// One timing metric line (a GPU pass or a mesher LOD), e.g.
/// `total n=15095 mean=1.19ms± 0.20 p50=1.21 p90=1.41 p99=1.59 min=0.00 max=5.31 over_8.3ms=0 (0.0%)`.
#[derive(Clone, Debug, Default)]
pub struct Metric {
    pub stage: String,
    pub n: u64,
    pub mean: f64,
    pub std: Option<f64>,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub over_ms: Option<f64>,
    pub over_n: Option<u64>,
    pub over_pct: Option<f64>,
}

/// A workload line (`loaded`/`rendering` chunk counts).
#[derive(Clone, Debug, Default)]
pub struct Workload {
    pub kind: String,
    pub n: u64,
    pub min: u64,
    pub p50: u64,
    pub p90: u64,
    pub max: u64,
}

/// The `memory:` line — GPU octree-pool utilization + the indirect-slot pool cap
/// + how many chunks fell back to the CPU mesher under OOM this run. A non-zero
/// `gpu_oom_fallback` is the fingerprint of the device-memory stripe.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub octree_bytes: u64,
    pub octree_capacity: u64,
    pub octree_chunks: u64,
    pub indirect_slots_cap: u64,
    pub gpu_oom_fallback: u64,
    pub lod_near: u64,
    pub lod_mid: u64,
    pub lod_far: u64,
    pub lod_vfar: u64,
}

impl Memory {
    /// Octree-pool utilization in [0, 1] (0 if capacity is unknown).
    pub fn octree_frac(&self) -> f64 {
        if self.octree_capacity == 0 {
            0.0
        } else {
            self.octree_bytes as f64 / self.octree_capacity as f64
        }
    }
}

/// The `compression:` line — what fraction of the live-storage memory would be
/// recovered if loaded chunks were held in SurfaceRun encoding instead of the
/// flat `Vec<BlockId>`. A ratio of e.g. 4.0 means the same chunks would fit in
/// a quarter of the bytes. This is measurement; no live storage swap yet.
#[derive(Clone, Debug, Default)]
pub struct Compression {
    pub chunks: u64,
    pub raw_bytes: u64,
    pub surface_run_bytes: u64,
    pub ratio: f64,
    pub worst_col_runs: u64,
}

/// The `tick_apply:` line — per-tick chunk-apply timing, sampled once per
/// FPS-log window. The max captures the loading-wave hitch (a single tick
/// applying hundreds of chunks at 100+ms) that drives frame-time spikes once
/// memory is off the critical path.
#[derive(Clone, Debug, Default)]
pub struct TickApply {
    pub n: u64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub max_chunks: u64,
}

/// One run appended to `run_summaries.txt`.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub uuid: Option<String>,
    pub version: Option<String>,
    pub duration: Option<String>,
    pub env: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub gpu: Vec<Metric>,
    pub mesher: Vec<Metric>,
    pub workload: Vec<Workload>,
    pub flags: Vec<(String, String)>,
    pub memory: Option<Memory>,
    pub compression: Option<Compression>,
    pub tick_apply: Option<TickApply>,
}

impl Run {
    /// Stable identity for correlation/selection.
    pub fn id(&self) -> String {
        self.uuid
            .clone()
            .or_else(|| self.start.clone())
            .unwrap_or_else(|| "run".to_string())
    }

    /// One-line label for the run list.
    pub fn label(&self) -> String {
        let uuid = self.uuid.as_deref().unwrap_or("(no uuid)");
        let short = uuid.split('-').last().map(|s| &s[..s.len().min(10)]).unwrap_or(uuid);
        let ver = self.version.as_deref().unwrap_or("?");
        let dur = self.duration.as_deref().unwrap_or("?");
        format!("{short}  ·  v{ver}  ·  {dur}")
    }

    pub fn gpu_total(&self) -> Option<&Metric> {
        self.gpu.iter().find(|m| m.stage == "total")
    }

    pub fn flag(&self, key: &str) -> Option<&str> {
        self.flags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// One parsed event line from `events.jsonl`. Fields are best-effort so the
/// parser tolerates schema drift.
#[derive(Clone, Debug)]
pub struct Event {
    pub uuid: Option<String>,
    pub kind: Option<String>,
    pub ts: Option<f64>,
    pub ms: Option<f64>,
}

/// Everything loaded so far.
#[derive(Default)]
pub struct Telemetry {
    pub runs: Vec<Run>,
    pub events: Vec<Event>,
}

impl Telemetry {
    pub fn events_for<'a>(&'a self, run: &Run) -> Vec<&'a Event> {
        let id = run.uuid.clone();
        self.events
            .iter()
            .filter(|e| id.is_some() && e.uuid == id)
            .collect()
    }

    /// `[seconds, frame_ms]` points for the frame-time curve.
    pub fn frame_curve(&self, run: &Run) -> Vec<[f64; 2]> {
        let mut pts: Vec<[f64; 2]> = self
            .events_for(run)
            .into_iter()
            .filter(|e| e.kind.as_deref() == Some("frame_window"))
            .filter_map(|e| match (e.ts, e.ms) {
                (Some(t), Some(m)) => Some([t / 1000.0, m]),
                _ => None,
            })
            .collect();
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        pts
    }

    /// `[seconds, ms]` points for spike markers.
    pub fn spikes(&self, run: &Run) -> Vec<[f64; 2]> {
        self.events_for(run)
            .into_iter()
            .filter(|e| e.kind.as_deref().map(|k| k.contains("spike")).unwrap_or(false))
            .filter_map(|e| match (e.ts, e.ms) {
                (Some(t), Some(m)) => Some([t / 1000.0, m]),
                _ => None,
            })
            .collect()
    }

    pub fn achievements(&self, run: &Run) -> Vec<String> {
        let evs = self.events_for(run);
        let mut a = Vec::new();
        let has_events = !evs.is_empty();
        if has_events {
            if evs.iter().any(|e| e.kind.as_deref() == Some("shutdown")) {
                a.push("clean-exit".to_string());
            } else {
                a.push("crash".to_string());
            }
            if evs.iter().any(|e| e.kind.as_deref().map(|k| k.contains("spike")).unwrap_or(false)) {
                a.push("spikes".to_string());
            }
        }
        let gpu_mesher = run.flag("UNENDLICH_GPU_MESHER") == Some("1")
            || run.env.as_deref().map(|e| e.contains("GPU_MESHER=1")).unwrap_or(false);
        if gpu_mesher {
            a.push("gpu-mesher".to_string());
        }
        if let Some(d) = &run.duration {
            if d.trim_end_matches('s').parse::<f64>().map(|v| v >= 120.0).unwrap_or(false) {
                a.push("sustained".to_string());
            }
        }
        a
    }
}
