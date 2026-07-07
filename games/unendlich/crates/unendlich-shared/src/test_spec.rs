//! `.unendtest` — declarative test-spec format for unendlich.
//!
//! A spec is a TOML file describing how to run the engine and what to
//! expect afterwards. The engine reads the spec, applies the env-var
//! overrides, runs for the specified duration, and writes a
//! `.unendresult.json` file with the measured values + per-assertion
//! pass/fail. Exit code reflects assertion outcome.
//!
//! Together with the launcher (c.10+) this gives us a reproducible
//! regression suite: every claim about engine behaviour gets a spec
//! that demonstrates it; every future change is verifiable against
//! the suite; cross-version comparisons surface regressions
//! automatically.
//!
//! ## Spec format
//!
//! ```toml
//! spec_version = 1
//! name = "rd8_default_30s"
//! description = "Baseline at RD=8 to compare against rd8_low_upload."
//!
//! [engine]
//! binary = "unendlich-world"
//! duration_ms = 30000
//!
//! [env]
//! UNENDLICH_RENDER_DISTANCE = "8"
//! UNENDLICH_VSYNC = "1"
//! UNENDLICH_INDIRECT = "1"
//! UNENDLICH_LOD_FACTORS = "4,8,16,32"
//! UNENDLICH_MAX_UPLOADS_PER_FRAME = "16"
//!
//! [expect]
//! shadow_p50_ms = { max = 80 }
//! rendering_max = { min = 80 }
//! duration_completed = true
//!
//! [output]
//! # optional; defaults below
//! result_path = "rd8_default_30s.unendresult.json"
//! events_path = "rd8_default_30s.events.jsonl"
//! ```
//!
//! ## Versioning
//!
//! `spec_version = 1` is the only currently-accepted version. Future
//! schema changes either:
//!   - Add optional fields (still spec_version 1) — old binaries ignore.
//!   - Bump to spec_version = 2 — old binaries reject with a clear error.
//!
//! ## Determinism
//!
//! Hybrid (per the c.7-era discussion):
//!   - World state is tightly deterministic: same seed + same camera
//!     path → same chunks generated → same blocks.
//!   - Perf metrics are loosely deterministic: assertions use ranges
//!     (`max = 80`, `min = 80`) rather than exact values. Inherent
//!     run-to-run variance is expected and the assertion DSL is
//!     designed around it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The currently-supported spec format version. Binaries reject specs
/// with `spec_version > CURRENT_SPEC_VERSION`.
pub const CURRENT_SPEC_VERSION: u32 = 1;

/// Top-level spec, parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct TestSpec {
    pub spec_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub expect: BTreeMap<String, AssertionSpec>,
    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EngineConfig {
    /// Which binary to run. Currently only `unendlich-world` is
    /// supported; future ships may add `unendlich-spin` etc.
    #[serde(default = "default_binary")]
    pub binary: String,
    /// Max wall-clock duration before auto-shutdown. The binary's
    /// `UNENDLICH_MAX_DURATION_MS` env var is set from this. A spec
    /// without a duration runs until window close — usable but not
    /// reproducible for assertions.
    pub duration_ms: Option<u64>,
    /// c.13-instrumentation: warmup window in milliseconds. The
    /// engine runs for this duration BEFORE starting the measurement
    /// window (per-frame events, GPU timer aggregation, FPS log).
    /// During warmup, chunks generate, the world settles, the
    /// shadow caches fill, the GPU pipelines compile.
    ///
    /// Default 0 (no warmup, measure from frame 0). Useful for perf
    /// specs where startup variance contaminates the metric of
    /// interest. Recommended: 3000ms for rd=8+ workloads.
    #[serde(default)]
    pub warmup_ms: Option<u64>,
}

fn default_binary() -> String { "unendlich-world".to_string() }

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OutputConfig {
    /// Where to write the result JSON. Defaults to `<spec_stem>.unendresult.json`.
    pub result_path: Option<String>,
    /// Where to direct events.jsonl. Defaults to `<spec_stem>.events.jsonl`.
    pub events_path: Option<String>,
}

/// An assertion. Three shapes (deserialized via untagged enum):
///   - Inline table: `{ max = 80 }`, `{ min = 80 }`, `{ min = 80, max = 200 }`
///   - Bool: `true` / `false` (for boolean predicates like `duration_completed`)
///   - Integer: `500` (sugar for `no_long_frames_over_ms = 500`)
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AssertionSpec {
    /// Range with explicit min and/or max.
    Range {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    /// Boolean predicate (e.g., `duration_completed = true`).
    Bool(bool),
    /// Scalar threshold — interpretation is metric-specific (caller
    /// translates to a `Range` based on the metric name).
    Scalar(f64),
}

/// A loaded spec — the raw spec plus the path it came from. The path
/// is used to default the result/events paths.
#[derive(Debug)]
pub struct LoadedSpec {
    pub spec: TestSpec,
    pub spec_path: std::path::PathBuf,
}

impl LoadedSpec {
    /// Load and validate a spec from a TOML file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, SpecError> {
        let path = path.as_ref().to_path_buf();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| SpecError::Io(path.display().to_string(), e))?;
        let spec: TestSpec = toml::from_str(&text)
            .map_err(|e| SpecError::Parse(path.display().to_string(), e.to_string()))?;
        if spec.spec_version > CURRENT_SPEC_VERSION {
            return Err(SpecError::UnsupportedVersion {
                got: spec.spec_version,
                max_supported: CURRENT_SPEC_VERSION,
            });
        }
        if spec.spec_version == 0 {
            return Err(SpecError::Parse(
                path.display().to_string(),
                "spec_version must be >= 1".to_string(),
            ));
        }
        Ok(LoadedSpec { spec, spec_path: path })
    }

    /// Resolve the result file path. Precedence (highest first):
    ///   1. `UNENDLICH_RESULT_PATH` env var (used by the launcher to
    ///      redirect outputs into a per-run directory).
    ///   2. The spec's `output.result_path` field.
    ///   3. Default sibling `<spec_stem>.unendresult.json`.
    pub fn result_path(&self) -> std::path::PathBuf {
        if let Ok(p) = std::env::var("UNENDLICH_RESULT_PATH") {
            return std::path::PathBuf::from(p);
        }
        if let Some(ref p) = self.spec.output.result_path {
            std::path::PathBuf::from(p)
        } else {
            self.derived_sibling("unendresult.json")
        }
    }

    /// Resolve the events file path. Precedence (highest first):
    ///   1. `UNENDLICH_EVENTS_PATH` env var (consumed by the
    ///      `util::events` module's own init; we honour it here for
    ///      consistency in the result-record so the value the user
    ///      sees in the result matches where events were actually
    ///      written).
    ///   2. The spec's `output.events_path` field.
    ///   3. Default sibling `<spec_stem>.events.jsonl`.
    pub fn events_path(&self) -> std::path::PathBuf {
        if let Ok(p) = std::env::var("UNENDLICH_EVENTS_PATH") {
            return std::path::PathBuf::from(p);
        }
        if let Some(ref p) = self.spec.output.events_path {
            std::path::PathBuf::from(p)
        } else {
            self.derived_sibling("events.jsonl")
        }
    }

    fn derived_sibling(&self, suffix: &str) -> std::path::PathBuf {
        let stem = self.spec_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.spec.name.clone());
        let parent = self.spec_path.parent().unwrap_or_else(|| Path::new("."));
        parent.join(format!("{}.{}", stem, suffix))
    }
}

#[derive(Debug)]
pub enum SpecError {
    Io(String, std::io::Error),
    Parse(String, String),
    UnsupportedVersion { got: u32, max_supported: u32 },
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::Io(path, e) => write!(f, "could not read spec {}: {}", path, e),
            SpecError::Parse(path, msg) => write!(f, "could not parse spec {}: {}", path, msg),
            SpecError::UnsupportedVersion { got, max_supported } => write!(
                f,
                "spec_version {} is newer than this binary supports (max {})",
                got, max_supported,
            ),
        }
    }
}

impl std::error::Error for SpecError {}

// ===== Result file format =====

/// Result of running a spec. Serialised as JSON and written to the
/// spec's `output.result_path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub spec_version: u32,
    pub spec_path: String,
    pub spec_name: String,
    pub binary_version: String,
    pub binary: String,
    pub run_uuid: String,
    pub ran_at_utc: String,
    pub duration_secs: f32,
    pub shutdown_reason: String,
    /// Resolved env-var values at run time (env-source values + defaults).
    /// One row per registered flag, with `value` and `source`.
    pub env: BTreeMap<String, ResolvedFlag>,
    /// v0.24.2c.12-b: GPU/driver identity at the moment of the run.
    /// Used by the launcher's `compare` command to detect driver-epoch
    /// drift between compared runs.
    ///
    /// Optional for backward compatibility: result files from c.11 and
    /// earlier don't have it, and deserializing them must still work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_info: Option<GpuInfo>,
    /// Aggregated stats from the run.
    pub stats: serde_json::Map<String, serde_json::Value>,
    /// Per-assertion result.
    pub assertions: Vec<AssertionResult>,
    /// Overall pass/fail.
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedFlag {
    pub value: String,
    pub source: String,  // "env" or "default"
}

/// GPU + driver identity captured at the moment of a run.
///
/// Populated by the engine at Vulkan context creation, then passed
/// through `BuildResultInputs` into the result file.
///
/// The fields come from `VkPhysicalDeviceProperties` plus the
/// `VK_KHR_driver_properties` extension (when available — older
/// drivers may omit it; in that case `driver_name`/`driver_info` are
/// empty).
///
/// `driver_hash` is a short, stable identifier computed from the
/// composite (vendor_id, device_id, driver_version_raw, driver_name,
/// c.13-scout: support for `VK_KHR_multiview` (Vulkan 1.1 core).
///
/// The multiview extension lets a single render pass write to multiple
/// layers of a layered framebuffer, with a per-invocation `gl_ViewIndex`
/// available in shaders. This is the foundation for efficient cascaded
/// shadow maps (one render pass touching all 4 cascade layers instead
/// of 4 separate passes).
///
/// We probe at startup to know whether c.17's multiview-CSM implementation
/// is feasible on the current driver. Asahi/Honeykrisp/Mesa support is
/// not guaranteed at the time of writing — this scout answers it.
///
/// Captured in GpuInfo (separate from the driver_hash so it doesn't
/// confound driver-epoch comparisons; absence is treated as "unknown
/// support, assume false" for older results).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MultiviewSupport {
    /// True if `VkPhysicalDeviceMultiviewFeatures::multiview` is enabled.
    /// If false, none of the other fields matter — multiview is unavailable.
    pub multiview: bool,
    /// Max number of views (layers) writable in one multiview pass.
    /// CSM needs ≥4 (one layer per cascade). Implementations are required
    /// to support at least 6 if multiview is supported at all, but we
    /// record the actual value for safety.
    pub max_view_count: u32,
    /// Max instance index when using multiview with instanced draws.
    /// Records for completeness; CSM doesn't need this.
    pub max_instance_index: u32,
    /// Whether multiview works with geometry shaders. Not relevant for
    /// shadow CSM (no GS), but recorded for completeness.
    pub geometry_shader: bool,
    /// Whether multiview works with tessellation. Same — not relevant
    /// for shadow CSM, but recorded for completeness.
    pub tessellation_shader: bool,
}

impl MultiviewSupport {
    /// Quick check: does this driver support CSM via multiview?
    /// (i.e., multiview enabled AND ≥4 views supported.)
    pub fn supports_csm(&self) -> bool {
        self.multiview && self.max_view_count >= 4
    }

    /// Human-readable summary for logs.
    pub fn summary(&self) -> String {
        if !self.multiview {
            return "unsupported".to_string();
        }
        format!(
            "supported (max_views={}, max_instance={}, geom={}, tess={})",
            self.max_view_count,
            self.max_instance_index,
            self.geometry_shader,
            self.tessellation_shader,
        )
    }
}

/// Composite identity for the GPU + driver combination used by a run.
/// Captured at engine startup. Identity is a hash over (vendor_id,
/// device_id, driver_version_raw, driver_name (e.g., "Honeykrisp"),
/// driver_info). The launcher's `compare` command warns when two
/// compared runs have different `driver_hash` values, since this
/// usually indicates a Mesa/driver update that may confound perf
/// comparisons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuInfo {
    /// Raw `VkPhysicalDeviceProperties::vendorID` (PCI vendor ID).
    pub vendor_id: u32,
    /// Raw `VkPhysicalDeviceProperties::deviceID`.
    pub device_id: u32,
    /// Raw `VkPhysicalDeviceProperties::driverVersion` (vendor-encoded).
    pub driver_version_raw: u32,
    /// From `VK_KHR_driver_properties::driverName` (e.g. "Asahi", "Intel
    /// open-source Mesa driver", "NVIDIA"). Empty if extension absent.
    #[serde(default)]
    pub driver_name: String,
    /// From `VK_KHR_driver_properties::driverInfo` (typically Mesa
    /// version string + git hash). Empty if extension absent.
    #[serde(default)]
    pub driver_info: String,
    /// Short hash of the composite identity, used as a quick equality
    /// check for "same driver epoch?" The launcher's compare command
    /// uses this to surface driver-mismatch warnings.
    ///
    /// Format: 16 hex chars of SHA-1 over the joined fields, prefixed
    /// with `drv-`. Stable across runs on the same machine + driver.
    pub driver_hash: String,
    /// c.13-scout: multiview support probed at startup. Not included
    /// in the driver_hash so result files predating this field still
    /// compare cleanly. Defaults to all-false for results without the
    /// field present.
    #[serde(default)]
    pub multiview: MultiviewSupport,
}

impl GpuInfo {
    /// Compute the driver_hash for a populated GpuInfo. Idempotent;
    /// the caller may call this when populating to fill in the field.
    pub fn compute_hash(
        vendor_id: u32,
        device_id: u32,
        driver_version_raw: u32,
        driver_name: &str,
        driver_info: &str,
    ) -> String {
        // We use a very small ad-hoc hash to avoid pulling in a real
        // cryptographic dep. The hash isn't security-sensitive — it
        // just needs to be stable and have low collision probability
        // across realistic driver combinations.
        //
        // FNV-1a 64-bit, encoded as 16 hex chars.
        let mut h: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        let mut feed = |bytes: &[u8]| {
            for b in bytes {
                h ^= *b as u64;
                h = h.wrapping_mul(prime);
            }
        };
        feed(&vendor_id.to_le_bytes());
        feed(&[0]); // field separator
        feed(&device_id.to_le_bytes());
        feed(&[0]);
        feed(&driver_version_raw.to_le_bytes());
        feed(&[0]);
        feed(driver_name.as_bytes());
        feed(&[0]);
        feed(driver_info.as_bytes());
        format!("drv-{:016x}", h)
    }

    /// Build a GpuInfo with `driver_hash` computed.
    pub fn new(
        vendor_id: u32,
        device_id: u32,
        driver_version_raw: u32,
        driver_name: String,
        driver_info: String,
    ) -> Self {
        let driver_hash = Self::compute_hash(
            vendor_id, device_id, driver_version_raw, &driver_name, &driver_info,
        );
        Self {
            vendor_id,
            device_id,
            driver_version_raw,
            driver_name,
            driver_info,
            driver_hash,
            // c.13-scout: caller can populate via with_multiview().
            multiview: MultiviewSupport::default(),
        }
    }

    /// Replace the multiview field. Builder-style for chaining off of
    /// `GpuInfo::new(...).with_multiview(probed)`.
    pub fn with_multiview(mut self, multiview: MultiviewSupport) -> Self {
        self.multiview = multiview;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    pub name: String,
    pub expected: String,
    pub actual: serde_json::Value,
    pub passed: bool,
    /// If failed: a brief human-readable reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ===== Assertion evaluation =====

/// Evaluate one assertion against the run's stats. The metric name is
/// the spec's key (`shadow_p50_ms`, `rendering_max`, etc.); how it's
/// looked up in `stats` is conventional:
///
///   - `<metric>` looked up at `stats[<metric>]`.
///   - `<dist>_max` / `<dist>_min` / `<dist>_p50` etc. looked up at
///     `stats[<dist>][max/min/p50/etc.]`.
///   - Special names handled separately (e.g. `duration_completed`).
pub fn eval_assertion(
    name: &str,
    spec: &AssertionSpec,
    stats: &serde_json::Map<String, serde_json::Value>,
    shutdown_reason: &str,
) -> AssertionResult {
    // Special-case: `duration_completed` checks shutdown reason.
    if name == "duration_completed" {
        let expected_completed = matches!(spec, AssertionSpec::Bool(true));
        let actual_completed = shutdown_reason == "max_duration";
        let passed = expected_completed == actual_completed;
        return AssertionResult {
            name: name.to_string(),
            expected: format!("shutdown_reason == 'max_duration' = {}", expected_completed),
            actual: serde_json::Value::Bool(actual_completed),
            passed,
            reason: if passed {
                None
            } else {
                Some(format!(
                    "duration_completed was {} (shutdown_reason={}), expected {}",
                    actual_completed, shutdown_reason, expected_completed,
                ))
            },
        };
    }

    // Look up the metric. Support both flat and nested lookups.
    let actual = lookup_metric(stats, name);

    let actual_num = actual.as_ref().and_then(|v| v.as_f64());

    match spec {
        AssertionSpec::Range { min, max } => {
            let actual_value = match actual_num {
                Some(v) => v,
                None => return missing_metric(name, format!("{:?}", spec), actual),
            };
            let mut ok = true;
            let mut reasons: Vec<String> = Vec::new();
            if let Some(m) = min {
                if actual_value < *m {
                    ok = false;
                    reasons.push(format!("{} < min={}", actual_value, m));
                }
            }
            if let Some(m) = max {
                if actual_value > *m {
                    ok = false;
                    reasons.push(format!("{} > max={}", actual_value, m));
                }
            }
            let expected = format_range(min, max);
            AssertionResult {
                name: name.to_string(),
                expected,
                actual: serde_json::json!(actual_value),
                passed: ok,
                reason: if ok { None } else { Some(reasons.join(", ")) },
            }
        }
        AssertionSpec::Scalar(threshold) => {
            // Sugar: scalar means `max = threshold` for *_ms metrics,
            // ambiguous otherwise. For c.9 we treat scalar as max.
            let actual_value = match actual_num {
                Some(v) => v,
                None => return missing_metric(name, format!("max={}", threshold), actual),
            };
            let ok = actual_value <= *threshold;
            AssertionResult {
                name: name.to_string(),
                expected: format!("<= {}", threshold),
                actual: serde_json::json!(actual_value),
                passed: ok,
                reason: if ok {
                    None
                } else {
                    Some(format!("{} > {}", actual_value, threshold))
                },
            }
        }
        AssertionSpec::Bool(expected) => {
            // For unknown bool assertions, look up `name` and compare
            // truthiness. Not many use cases; we still want it to be
            // well-defined.
            let actual_truthy = match actual.as_ref() {
                Some(serde_json::Value::Bool(b)) => *b,
                Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
                _ => false,
            };
            let ok = actual_truthy == *expected;
            AssertionResult {
                name: name.to_string(),
                expected: format!("{}", expected),
                actual: actual.unwrap_or(serde_json::Value::Null),
                passed: ok,
                reason: if ok {
                    None
                } else {
                    Some(format!("expected bool {}, got {}", expected, actual_truthy))
                },
            }
        }
    }
}

fn missing_metric(name: &str, expected: String, actual: Option<serde_json::Value>) -> AssertionResult {
    AssertionResult {
        name: name.to_string(),
        expected,
        actual: actual.unwrap_or(serde_json::Value::Null),
        passed: false,
        reason: Some(format!("metric `{}` not found in run stats", name)),
    }
}

fn format_range(min: &Option<f64>, max: &Option<f64>) -> String {
    match (min, max) {
        (Some(mi), Some(ma)) => format!("min={}, max={}", mi, ma),
        (Some(mi), None) => format!("min={}", mi),
        (None, Some(ma)) => format!("max={}", ma),
        (None, None) => "(no constraint)".to_string(),
    }
}

/// Look up a metric name in the stats map. Supports:
///   - `flat_key` → `stats[flat_key]`
///   - `distribution.field` → `stats[distribution][field]` (dotted)
///   - `distribution_field` → `stats[distribution][field]` (suffix sugar,
///     when `field` is one of `min`, `p50`, `p90`, `p99`, `max`,
///     `mean`, `value`)
fn lookup_metric(stats: &serde_json::Map<String, serde_json::Value>, name: &str)
    -> Option<serde_json::Value>
{
    if let Some(v) = stats.get(name) {
        return Some(v.clone());
    }
    // Try dotted lookup: `a.b` → stats[a][b].
    if let Some((head, tail)) = name.split_once('.') {
        if let Some(serde_json::Value::Object(inner)) = stats.get(head) {
            if let Some(v) = inner.get(tail) {
                return Some(v.clone());
            }
        }
    }
    // Try suffix sugar: `frame_p50_ms` → stats[frame_p50_ms] OR
    // stats[frame][p50_ms] OR stats[frame_p50][ms]. We restrict the
    // suffix set so we don't get surprising matches.
    const SUFFIXES: &[&str] = &["min", "p50", "p90", "p99", "max", "mean", "value"];
    for suffix in SUFFIXES {
        let with_underscore = format!("_{}", suffix);
        if let Some(head) = name.strip_suffix(&with_underscore) {
            if let Some(serde_json::Value::Object(inner)) = stats.get(head) {
                if let Some(v) = inner.get(*suffix) {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}

/// Inputs for `build_result` — separated from the binary's main()
/// for unit testing. Each field corresponds to data the binary has
/// at shutdown time.
pub struct BuildResultInputs<'a> {
    pub spec: &'a LoadedSpec,
    pub binary_version: &'a str,
    pub binary: &'a str,
    pub run_uuid: &'a str,
    pub ran_at_utc: &'a str,
    pub duration_secs: f32,
    pub shutdown_reason: &'a str,
    pub env: BTreeMap<String, ResolvedFlag>,
    pub stats: serde_json::Map<String, serde_json::Value>,
    /// GPU/driver identity at run time. `None` for binaries built
    /// without render features (no Vulkan context). Future c.12+
    /// builds with render always populate this.
    pub gpu_info: Option<GpuInfo>,
}

/// Build a `TestResult` from per-run data and the spec's assertions.
/// Pure function — same inputs always produce the same result.
pub fn build_result(inputs: BuildResultInputs<'_>) -> TestResult {
    let mut assertions: Vec<AssertionResult> = Vec::new();
    for (name, spec) in &inputs.spec.spec.expect {
        assertions.push(eval_assertion(
            name, spec, &inputs.stats, inputs.shutdown_reason,
        ));
    }
    let passed = assertions.iter().all(|r| r.passed);
    TestResult {
        spec_version: inputs.spec.spec.spec_version,
        spec_path: inputs.spec.spec_path.display().to_string(),
        spec_name: inputs.spec.spec.name.clone(),
        binary_version: inputs.binary_version.to_string(),
        binary: inputs.binary.to_string(),
        run_uuid: inputs.run_uuid.to_string(),
        ran_at_utc: inputs.ran_at_utc.to_string(),
        duration_secs: inputs.duration_secs,
        shutdown_reason: inputs.shutdown_reason.to_string(),
        env: inputs.env,
        gpu_info: inputs.gpu_info,
        stats: inputs.stats,
        assertions,
        passed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_temp(contents: &str, label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "unendtest-{}-{}-{}.unendtest",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::write(&path, contents).expect("write temp spec");
        path
    }

    #[test]
    fn minimal_valid_spec_loads() {
        let path = write_temp(r#"
spec_version = 1
name = "minimal"
"#, "minimal");
        let s = LoadedSpec::load(&path).expect("load");
        assert_eq!(s.spec.spec_version, 1);
        assert_eq!(s.spec.name, "minimal");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_future_spec_version() {
        let path = write_temp(r#"
spec_version = 99
name = "from_the_future"
"#, "future");
        let err = LoadedSpec::load(&path).unwrap_err();
        match err {
            SpecError::UnsupportedVersion { got, .. } => assert_eq!(got, 99),
            other => panic!("expected UnsupportedVersion; got {:?}", other),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn full_spec_round_trips() {
        let path = write_temp(r#"
spec_version = 1
name = "rd8_default_30s"
description = "Baseline"

[engine]
binary = "unendlich-world"
duration_ms = 30000

[env]
UNENDLICH_RENDER_DISTANCE = "8"
UNENDLICH_VSYNC = "1"

[expect]
shadow_p50_ms = { max = 80 }
rendering_max = { min = 80 }
duration_completed = true
"#, "full");
        let s = LoadedSpec::load(&path).expect("load");
        assert_eq!(s.spec.engine.duration_ms, Some(30000));
        assert_eq!(s.spec.env.get("UNENDLICH_RENDER_DISTANCE"), Some(&"8".to_string()));
        assert_eq!(s.spec.expect.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn result_path_defaults_to_sibling() {
        let path = write_temp(r#"
spec_version = 1
name = "foo"
"#, "result_default");
        let s = LoadedSpec::load(&path).expect("load");
        let r = s.result_path();
        let s_str = r.to_string_lossy();
        assert!(s_str.ends_with(".unendresult.json"),
            "result path didn't end with .unendresult.json: {}", s_str);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn result_path_honours_override() {
        let path = write_temp(r#"
spec_version = 1
name = "foo"

[output]
result_path = "/tmp/custom_path.json"
"#, "result_custom");
        let s = LoadedSpec::load(&path).expect("load");
        assert_eq!(s.result_path(), std::path::PathBuf::from("/tmp/custom_path.json"));
        let _ = std::fs::remove_file(&path);
    }

    /// CAUTION: this test sets a process-wide env var. We use a path
    /// value unique enough that no other test could collide on read,
    /// and we always unset at the end via the guard.
    ///
    /// The launcher's primary need is this override: it sets the env
    /// var, spawns the binary, and the spec's result_path is
    /// transparently redirected. Spec on disk doesn't have to be
    /// rewritten.
    #[test]
    fn result_path_env_var_wins_over_spec() {
        let path = write_temp(r#"
spec_version = 1
name = "foo"

[output]
result_path = "/tmp/from_spec.json"
"#, "result_env");
        let s = LoadedSpec::load(&path).expect("load");

        // Guard to ensure the env var is removed even on test failure.
        struct EnvGuard;
        impl Drop for EnvGuard {
            fn drop(&mut self) { std::env::remove_var("UNENDLICH_RESULT_PATH"); }
        }
        let _g = EnvGuard;

        std::env::set_var("UNENDLICH_RESULT_PATH", "/tmp/from_env_55518.json");
        assert_eq!(s.result_path(),
            std::path::PathBuf::from("/tmp/from_env_55518.json"),
            "env var should override spec's output.result_path");

        let _ = std::fs::remove_file(&path);
    }

    /// Same for events_path. The events module also reads this env
    /// var internally; we honour it in `events_path()` so the result
    /// file's `events_path` field matches where events actually
    /// landed.
    #[test]
    fn events_path_env_var_wins_over_spec() {
        let path = write_temp(r#"
spec_version = 1
name = "foo"

[output]
events_path = "/tmp/from_spec_evts.jsonl"
"#, "events_env");
        let s = LoadedSpec::load(&path).expect("load");

        struct EnvGuard;
        impl Drop for EnvGuard {
            fn drop(&mut self) { std::env::remove_var("UNENDLICH_EVENTS_PATH"); }
        }
        let _g = EnvGuard;

        std::env::set_var("UNENDLICH_EVENTS_PATH", "/tmp/from_env_99381.jsonl");
        assert_eq!(s.events_path(),
            std::path::PathBuf::from("/tmp/from_env_99381.jsonl"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lookup_metric_flat() {
        let stats: serde_json::Map<String, serde_json::Value> =
            json!({"shadow_p50_ms": 47.0}).as_object().unwrap().clone();
        assert_eq!(lookup_metric(&stats, "shadow_p50_ms"),
                   Some(json!(47.0)));
    }

    #[test]
    fn lookup_metric_dotted() {
        let stats: serde_json::Map<String, serde_json::Value> = json!({
            "rendering": {"min":10, "p50":127, "max":144}
        }).as_object().unwrap().clone();
        assert_eq!(lookup_metric(&stats, "rendering.max"),
                   Some(json!(144)));
    }

    #[test]
    fn lookup_metric_suffix_sugar() {
        let stats: serde_json::Map<String, serde_json::Value> = json!({
            "rendering": {"min":10, "max":144}
        }).as_object().unwrap().clone();
        // `rendering_max` should resolve to stats[rendering][max].
        assert_eq!(lookup_metric(&stats, "rendering_max"),
                   Some(json!(144)));
    }

    #[test]
    fn lookup_metric_missing() {
        let stats = serde_json::Map::new();
        assert_eq!(lookup_metric(&stats, "nonexistent"), None);
    }

    #[test]
    fn eval_range_passes_when_within() {
        let stats: serde_json::Map<String, serde_json::Value> =
            json!({"shadow_p50_ms": 47.0}).as_object().unwrap().clone();
        let spec = AssertionSpec::Range { min: None, max: Some(80.0) };
        let r = eval_assertion("shadow_p50_ms", &spec, &stats, "max_duration");
        assert!(r.passed, "expected pass; got reason: {:?}", r.reason);
    }

    #[test]
    fn eval_range_fails_when_above_max() {
        let stats: serde_json::Map<String, serde_json::Value> =
            json!({"shadow_p50_ms": 95.0}).as_object().unwrap().clone();
        let spec = AssertionSpec::Range { min: None, max: Some(80.0) };
        let r = eval_assertion("shadow_p50_ms", &spec, &stats, "max_duration");
        assert!(!r.passed);
        assert!(r.reason.as_ref().unwrap().contains("95"));
    }

    #[test]
    fn eval_range_fails_when_below_min() {
        let stats: serde_json::Map<String, serde_json::Value> =
            json!({"rendering": {"max": 50}}).as_object().unwrap().clone();
        let spec = AssertionSpec::Range { min: Some(80.0), max: None };
        let r = eval_assertion("rendering_max", &spec, &stats, "max_duration");
        assert!(!r.passed);
    }

    #[test]
    fn eval_duration_completed_special_case() {
        let stats = serde_json::Map::new();
        let spec = AssertionSpec::Bool(true);
        let r = eval_assertion("duration_completed", &spec, &stats, "max_duration");
        assert!(r.passed);
        let r2 = eval_assertion("duration_completed", &spec, &stats, "window_close_or_esc");
        assert!(!r2.passed);
    }

    #[test]
    fn eval_missing_metric_fails_cleanly() {
        let stats = serde_json::Map::new();
        let spec = AssertionSpec::Range { min: None, max: Some(80.0) };
        let r = eval_assertion("nonexistent", &spec, &stats, "max_duration");
        assert!(!r.passed);
        assert!(r.reason.as_ref().unwrap().contains("not found"));
    }

    /// End-to-end: load a spec, build synthetic stats matching it,
    /// call build_result, check assertions and overall pass.
    #[test]
    fn build_result_evaluates_all_assertions() {
        let path = write_temp(r#"
spec_version = 1
name = "synthetic"

[engine]
duration_ms = 1000

[expect]
shadow_p50_ms = { max = 80 }
rendering_max = { min = 80 }
duration_completed = true
"#, "build_result");
        let spec = LoadedSpec::load(&path).expect("load");

        let stats = json!({
            "shadow_p50_ms": 47.0,
            "rendering_max": 100,
        }).as_object().unwrap().clone();

        let result = build_result(BuildResultInputs {
            spec: &spec,
            binary_version: "0.24.2-c.9",
            binary: "unendlich-world",
            run_uuid: "r-0000000000000099",
            ran_at_utc: "2026-05-11T00:00:00Z",
            duration_secs: 1.0,
            shutdown_reason: "max_duration",
            env: BTreeMap::new(),
            stats,
            gpu_info: None,
        });

        assert!(result.passed, "all 3 assertions should pass; got: {:#?}",
            result.assertions);
        assert_eq!(result.assertions.len(), 3);
        assert_eq!(result.binary_version, "0.24.2-c.9");
        assert_eq!(result.run_uuid, "r-0000000000000099");

        let _ = std::fs::remove_file(&path);
    }

    /// One failing assertion makes the overall result fail.
    #[test]
    fn build_result_one_failure_fails_overall() {
        let path = write_temp(r#"
spec_version = 1
name = "synth_fail"

[expect]
shadow_p50_ms = { max = 50 }
"#, "build_result_fail");
        let spec = LoadedSpec::load(&path).expect("load");
        let stats = json!({"shadow_p50_ms": 95.0}).as_object().unwrap().clone();

        let result = build_result(BuildResultInputs {
            spec: &spec,
            binary_version: "v",
            binary: "b",
            run_uuid: "r-0000000000000098",
            ran_at_utc: "t",
            duration_secs: 0.0,
            shutdown_reason: "max_duration",
            env: BTreeMap::new(),
            stats,
            gpu_info: None,
        });

        assert!(!result.passed);
        let failed: Vec<_> = result.assertions.iter().filter(|a| !a.passed).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].name, "shadow_p50_ms");

        let _ = std::fs::remove_file(&path);
    }

    /// Round-trip serde: a built TestResult serialises and deserialises.
    /// Important because that's how the analyze CLI / launcher will
    /// load result files.
    #[test]
    fn test_result_serde_round_trip() {
        let path = write_temp(r#"
spec_version = 1
name = "rt"
[expect]
shadow_p50_ms = { max = 80 }
"#, "round_trip");
        let spec = LoadedSpec::load(&path).expect("load");
        let stats = json!({"shadow_p50_ms": 47.0}).as_object().unwrap().clone();
        let result = build_result(BuildResultInputs {
            spec: &spec,
            binary_version: "0.24.2-c.9",
            binary: "unendlich-world",
            run_uuid: "r-0000000000000097",
            ran_at_utc: "2026-05-11T00:00:00Z",
            duration_secs: 1.5,
            shutdown_reason: "max_duration",
            env: BTreeMap::new(),
            stats,
            gpu_info: None,
        });

        let json = serde_json::to_string(&result).expect("serialize");
        let back: TestResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.spec_version, 1);
        assert_eq!(back.spec_name, "rt");
        assert_eq!(back.run_uuid, "r-0000000000000097");
        assert_eq!(back.passed, true);
        assert_eq!(back.assertions.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // ============================================================
    // GpuInfo tests
    // ============================================================

    /// Hash is deterministic across calls with the same inputs.
    /// This is foundational — if it weren't, the launcher's
    /// compare command couldn't detect driver-mismatch reliably.
    #[test]
    fn gpu_info_hash_is_deterministic() {
        let h1 = GpuInfo::compute_hash(0x106B, 0x1024, 0x00400000, "Asahi", "Mesa 25.0.0-devel");
        let h2 = GpuInfo::compute_hash(0x106B, 0x1024, 0x00400000, "Asahi", "Mesa 25.0.0-devel");
        assert_eq!(h1, h2);
        assert!(h1.starts_with("drv-"), "hash should start with drv-: {}", h1);
        assert_eq!(h1.len(), 20, "drv- + 16 hex chars = 20"); // "drv-" + 16 hex
    }

    /// Different inputs produce different hashes.
    /// FNV-1a isn't collision-free in principle, but for realistic
    /// distinct driver tuples it should never collide. The point is:
    /// changing ANY field changes the hash.
    #[test]
    fn gpu_info_hash_distinguishes_drivers() {
        let base = GpuInfo::compute_hash(0x106B, 0x1024, 0x00400000, "Asahi", "Mesa 25.0.0");

        // Different driver version (Mesa update).
        let with_new_version = GpuInfo::compute_hash(0x106B, 0x1024, 0x00400000, "Asahi", "Mesa 26.0.0");
        assert_ne!(base, with_new_version, "driver_info change should change hash");

        // Different vendor (Intel vs Asahi).
        let intel = GpuInfo::compute_hash(0x8086, 0x1024, 0x00400000, "Asahi", "Mesa 25.0.0");
        assert_ne!(base, intel, "vendor_id change should change hash");

        // Different device id.
        let other_device = GpuInfo::compute_hash(0x106B, 0xFFFF, 0x00400000, "Asahi", "Mesa 25.0.0");
        assert_ne!(base, other_device, "device_id change should change hash");
    }

    /// Constructor populates driver_hash from the other fields.
    #[test]
    fn gpu_info_new_populates_hash() {
        let info = GpuInfo::new(0x106B, 0x1024, 0x00400000,
            "Asahi".to_string(), "Mesa 25.0.0".to_string());
        let expected = GpuInfo::compute_hash(0x106B, 0x1024, 0x00400000, "Asahi", "Mesa 25.0.0");
        assert_eq!(info.driver_hash, expected);
    }

    /// Old result files (without gpu_info) must still deserialize.
    /// This is the backward-compatibility test: a c.11 result file
    /// loaded by a c.12+ binary should still work.
    #[test]
    fn test_result_deserializes_without_gpu_info() {
        let old_json = r#"{
            "spec_version": 1,
            "spec_path": "/tmp/foo.unendtest",
            "spec_name": "foo",
            "binary_version": "0.24.2-c.11",
            "binary": "unendlich-world",
            "run_uuid": "r-0000000000000011",
            "ran_at_utc": "2026-05-11T00:00:00Z",
            "duration_secs": 5.0,
            "shutdown_reason": "max_duration",
            "env": {},
            "stats": {},
            "assertions": [],
            "passed": true
        }"#;
        let result: TestResult = serde_json::from_str(old_json).expect("deserialize old");
        assert!(result.gpu_info.is_none(),
            "old result file should produce gpu_info = None");
    }

    /// New result files round-trip cleanly with gpu_info populated.
    #[test]
    fn test_result_round_trip_with_gpu_info() {
        let info = GpuInfo::new(0x106B, 0x1024, 0x00400000,
            "Asahi".to_string(), "Mesa 25.0.0".to_string());

        let path = write_temp(r#"
spec_version = 1
name = "rt2"
"#, "rt_gpu");
        let spec = LoadedSpec::load(&path).expect("load");
        let result = build_result(BuildResultInputs {
            spec: &spec,
            binary_version: "0.24.2-c.12-b",
            binary: "unendlich-world",
            run_uuid: "r-0000000000000012",
            ran_at_utc: "2026-05-12T00:00:00Z",
            duration_secs: 3.0,
            shutdown_reason: "max_duration",
            env: BTreeMap::new(),
            stats: serde_json::Map::new(),
            gpu_info: Some(info.clone()),
        });

        let json = serde_json::to_string(&result).expect("serialize");
        let back: TestResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.gpu_info, Some(info));

        let _ = std::fs::remove_file(&path);
    }
}
