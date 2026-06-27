// src/soundscape_config.rs
//
// Higher-level soundscape configuration:
//   * Channel layout for the target playback environment.
//   * Per-layer channel routing (Pin / Pair / Diffuse / Auto).
//   * Validation: file paths exist, weights sum sensibly, channel
//     names parse, layer references in states line up.
//   * Import from JSON files.
//   * Export to JSON.
//
// The existing `Soundscape` struct in soundscape.rs stays as the
// "compiled" runtime form. This module is the user-facing config:
// the format you'd save to disk and load from a file dialog. The
// `compile()` function turns config into runtime Soundscape +
// resolves channel routing for the target layout.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::channel_layout::{Channel, Layout, RouteStrategy, resolve, ResolvedRouting,
                             legacy_channel_to_strategy};
use super::soundscape::{Layer, Soundscape, SceneState, Millis};

/// A layer definition as it appears in the config file.
///
/// `channel` may be either a legacy free-form string (parsed via
/// `legacy_channel_to_strategy`) or a structured `RouteStrategy`. Use
/// `routing` for the structured form; if both are present, `routing`
/// wins. This lets older config files still load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
    pub name: String,
    pub pool: Vec<String>,
    /// Legacy free-form channel hint. Used when `routing` is None.
    #[serde(default)]
    pub channel: String,
    /// Structured routing strategy. Overrides `channel` when present.
    #[serde(default)]
    pub routing: Option<RouteStrategy>,
    #[serde(default = "default_crossfade")]
    pub crossfade_ms: Millis,
    #[serde(default = "default_unit_range")]
    pub gain_range: (f32, f32),
    #[serde(default)]
    pub pan_range: (f32, f32),
    #[serde(default = "default_unit_range")]
    pub rate_range: (f32, f32),
    #[serde(default = "default_one")]
    pub trigger_prob: f32,
    #[serde(default = "default_interval")]
    pub interval_ms: (Millis, Millis),
    /// Optional clip duration (ms). Used by the scheduler to avoid
    /// stacking too many simultaneous clips per layer. Optional
    /// because the scheduler can also derive it from clip metadata.
    #[serde(default)]
    pub duration_ms: Option<Millis>,
    /// Swell (slow gain breathing): full LFO cycle length in ms and peak
    /// fractional depth in [0,1]. Both default to off so existing scenes
    /// are unchanged. See soundscape::swell_gain.
    #[serde(default)]
    pub swell_period_ms: Millis,
    #[serde(default)]
    pub swell_depth: f32,
    /// Timing humanization 0..1 (fraction of inter-trigger step). See
    /// soundscape::Layer::timing_jitter. Default 0 = exact grid timing.
    #[serde(default)]
    pub timing_jitter: f32,
}

/// Per-state layer weighting. Same shape as runtime SceneState but
/// the JSON layout is friendlier (transitions as named arrays rather
/// than index tuples).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateConfig {
    pub name: String,
    /// layer_name → weight in this state.
    pub layer_weights: HashMap<String, f32>,
    /// Dwell range [min, max] in ms.
    pub dwell_ms: (Millis, Millis),
    /// Named transitions: { "to_state_name": weight }.
    pub transitions: HashMap<String, f32>,
}

/// Top-level soundscape config — what gets written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundscapeConfig {
    pub name: String,
    #[serde(default = "default_layout")]
    pub layout: Layout,
    pub layers: Vec<LayerConfig>,
    pub states: Vec<StateConfig>,
    #[serde(default)]
    pub start_state: String,
    /// Optional human description shown in the picker UI.
    #[serde(default)]
    pub description: String,
    /// Optional creator / source attribution.
    #[serde(default)]
    pub author: String,
    /// Schema version. Bumped when breaking changes happen.
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_crossfade() -> Millis { 1500 }
fn default_unit_range() -> (f32, f32) { (1.0, 1.0) }
fn default_one() -> f32 { 1.0 }
fn default_interval() -> (Millis, Millis) { (8000, 16000) }
fn default_layout() -> Layout { Layout::Stereo }
fn default_version() -> u32 { 1 }

/// Validation problems found in a config.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }
    pub fn merge(mut self, other: ValidationReport) -> Self {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
        self
    }
}

/// Validate a config: every layer named in a state's weights must
/// exist, transitions must reference real states, channel routings
/// must parse, weights must be non-negative, ranges must be ordered.
///
/// `paths_exist_check` controls whether we actually stat() file paths
/// from `layer.pool`. Disable for unit tests + previewing remote configs.
pub fn validate(cfg: &SoundscapeConfig, paths_exist_check: bool) -> ValidationReport {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    if cfg.name.is_empty() {
        errors.push("soundscape name is empty".into());
    }
    if cfg.layers.is_empty() {
        errors.push("no layers defined".into());
    }
    if cfg.states.is_empty() {
        errors.push("no states defined".into());
    }

    // Layer-name uniqueness + clip pool sanity.
    let mut layer_names: HashMap<String, ()> = HashMap::new();
    for layer in &cfg.layers {
        if layer_names.insert(layer.name.clone(), ()).is_some() {
            errors.push(format!("duplicate layer name: {}", layer.name));
        }
        if layer.pool.is_empty() {
            warnings.push(format!("layer '{}' has empty clip pool", layer.name));
        }
        if layer.routing.is_none() && layer.channel.is_empty() {
            warnings.push(format!(
                "layer '{}' has neither `routing` nor legacy `channel` set; \
                 will default to ambient diffuse", layer.name));
        }
        // Numeric finite-ness checks. A non-finite value here would
        // propagate into the soundscape director's per-sample math
        // (gain, pan, rate multipliers) and produce NaN audio that
        // the downstream filters' defenses would catch but only
        // after silent samples have been emitted. Reject at config-
        // load time instead so the failure is loud and pointable.
        if !layer.gain_range.0.is_finite() || !layer.gain_range.1.is_finite() {
            errors.push(format!(
                "layer '{}' gain_range has non-finite bound: ({}, {})",
                layer.name, layer.gain_range.0, layer.gain_range.1));
        }
        if !layer.pan_range.0.is_finite() || !layer.pan_range.1.is_finite() {
            errors.push(format!(
                "layer '{}' pan_range has non-finite bound: ({}, {})",
                layer.name, layer.pan_range.0, layer.pan_range.1));
        }
        if !layer.rate_range.0.is_finite() || !layer.rate_range.1.is_finite() {
            errors.push(format!(
                "layer '{}' rate_range has non-finite bound: ({}, {})",
                layer.name, layer.rate_range.0, layer.rate_range.1));
        }
        if !layer.trigger_prob.is_finite() {
            errors.push(format!(
                "layer '{}' trigger_prob is non-finite", layer.name));
        }
        // Ranges sanity.
        if layer.gain_range.0 > layer.gain_range.1 {
            errors.push(format!(
                "layer '{}' gain_range lo({}) > hi({})",
                layer.name, layer.gain_range.0, layer.gain_range.1));
        }
        if layer.rate_range.0 <= 0.0 {
            errors.push(format!("layer '{}' rate_range starts at <= 0", layer.name));
        }
        if layer.trigger_prob < 0.0 || layer.trigger_prob > 1.0 {
            errors.push(format!(
                "layer '{}' trigger_prob {} out of [0,1]",
                layer.name, layer.trigger_prob));
        }
        if layer.interval_ms.0 > layer.interval_ms.1 {
            errors.push(format!(
                "layer '{}' interval_ms lo > hi", layer.name));
        }
        if paths_exist_check {
            for p in &layer.pool {
                if !Path::new(p).exists() {
                    warnings.push(format!(
                        "layer '{}' clip not found: {}", layer.name, p));
                }
            }
        }
    }

    // State-name uniqueness + transitions.
    let mut state_names: HashMap<String, ()> = HashMap::new();
    for state in &cfg.states {
        if state_names.insert(state.name.clone(), ()).is_some() {
            errors.push(format!("duplicate state name: {}", state.name));
        }
        for (layer_name, weight) in &state.layer_weights {
            if !layer_names.contains_key(layer_name) {
                errors.push(format!(
                    "state '{}' references unknown layer '{}'",
                    state.name, layer_name));
            }
            if !weight.is_finite() {
                errors.push(format!(
                    "state '{}' weight for '{}' is non-finite ({})",
                    state.name, layer_name, weight));
            } else if *weight < 0.0 {
                errors.push(format!(
                    "state '{}' weight for '{}' is negative",
                    state.name, layer_name));
            }
        }
        if state.dwell_ms.0 > state.dwell_ms.1 {
            errors.push(format!("state '{}' dwell_ms lo > hi", state.name));
        }
        for (to_state, weight) in &state.transitions {
            if !cfg.states.iter().any(|s| &s.name == to_state) {
                errors.push(format!(
                    "state '{}' transitions to unknown '{}'",
                    state.name, to_state));
            }
            if !weight.is_finite() {
                errors.push(format!(
                    "state '{}' transition to '{}' has non-finite weight ({})",
                    state.name, to_state, weight));
            } else if *weight < 0.0 {
                errors.push(format!(
                    "state '{}' transition to '{}' has negative weight",
                    state.name, to_state));
            }
        }
    }

    // start_state must be a real state if specified.
    if !cfg.start_state.is_empty()
        && !cfg.states.iter().any(|s| s.name == cfg.start_state)
    {
        errors.push(format!(
            "start_state '{}' not in defined states", cfg.start_state));
    }

    ValidationReport { errors, warnings }
}

/// Compile a config into the runtime `Soundscape` + a parallel map of
/// resolved per-layer channel routings keyed by layer name. Both are
/// needed by the scheduler/transport: Soundscape drives the FSM
/// director; the routings drive the per-channel gain matrix.
///
/// Returns the resolved structure or an error if validation fails.
pub fn compile(cfg: &SoundscapeConfig) -> Result<(Soundscape, HashMap<String, ResolvedRouting>), String> {
    let report = validate(cfg, false);
    if !report.is_ok() {
        return Err(format!(
            "config validation failed:\n  - {}",
            report.errors.join("\n  - ")));
    }
    // Layer name → index for state weight maps + start_state lookup.
    let state_name_to_idx: HashMap<&str, usize> = cfg.states.iter()
        .enumerate().map(|(i, s)| (s.name.as_str(), i)).collect();

    let mut routings: HashMap<String, ResolvedRouting> = HashMap::new();
    let layers: Vec<Layer> = cfg.layers.iter().map(|lc| {
        let strategy = lc.routing.clone()
            .unwrap_or_else(|| legacy_channel_to_strategy(&lc.channel));
        let resolved = resolve(&strategy, cfg.layout);
        // Pick the channel the runtime layer will actually announce to
        // the mesh. The raw `lc.channel` string may name a channel that
        // doesn't exist in this layout (e.g. "C" on Stereo), which would
        // leave the layer with an unclaimable channel and no audio. We
        // resolve to the highest-gain present channel so the mesh has a
        // real speaker target. Falls back to the raw string only when
        // resolve produced nothing (shouldn't happen now that Pin
        // downmixes, but keeps behaviour defined).
        let effective_channel = resolved.channel_gains.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(ch, _)| ch.as_str().to_string())
            .unwrap_or_else(|| lc.channel.clone());
        // Build the spatial spread: the full set of (channel, gain)
        // targets from the resolved routing, sorted for deterministic
        // ordering across nodes. When this has 2+ entries the scheduler
        // decorrelates the layer across those speakers for envelopment.
        let mut spread: Vec<(String, f32)> = resolved.channel_gains.iter()
            .map(|(ch, g)| (ch.as_str().to_string(), *g))
            .collect();
        spread.sort_by(|a, b| a.0.cmp(&b.0));
        routings.insert(lc.name.clone(), resolved);
        // The runtime layer uses one `nominal_clip_ms` field. We pick
        // `duration_ms` if the config provided it, else the midpoint of
        // the interval as a reasonable default.
        let nominal_clip_ms = lc.duration_ms.unwrap_or_else(||
            (lc.interval_ms.0 + lc.interval_ms.1) / 2);
        Layer {
            name: lc.name.clone(),
            pool: lc.pool.clone(),
            channel: effective_channel,
            crossfade_ms: lc.crossfade_ms,
            gain_range: lc.gain_range,
            pan_range: lc.pan_range,
            rate_range: lc.rate_range,
            trigger_prob: lc.trigger_prob,
            nominal_clip_ms,
            spread,
            swell_period_ms: lc.swell_period_ms,
            swell_depth: lc.swell_depth,
            timing_jitter: lc.timing_jitter,
        }
    }).collect();

    let states: Vec<SceneState> = cfg.states.iter().map(|sc| {
        let transitions: Vec<(usize, f32)> = sc.transitions.iter()
            .filter_map(|(name, w)| state_name_to_idx.get(name.as_str()).map(|&i| (i, *w)))
            .collect();
        SceneState {
            name: sc.name.clone(),
            layer_weights: sc.layer_weights.clone(),
            dwell_ms: sc.dwell_ms,
            transitions,
        }
    }).collect();

    let start_state_idx = if cfg.start_state.is_empty() {
        0
    } else {
        *state_name_to_idx.get(cfg.start_state.as_str()).unwrap_or(&0)
    };

    Ok((Soundscape {
        name: cfg.name.clone(),
        layers,
        states,
        start_state: start_state_idx,
        // External configs don't yet carry a target_sink (the
        // soundscape.json schema predates it). Default to None;
        // a future schema bump can surface it as an optional field.
        target_sink: None,
    }, routings))
}

/// Load a config from a JSON file.
pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<SoundscapeConfig, String> {
    let text = std::fs::read_to_string(path.as_ref())
        .map_err(|e| format!("read {}: {}", path.as_ref().display(), e))?;
    parse(&text)
}

/// Parse a config from a JSON string.
pub fn parse(json: &str) -> Result<SoundscapeConfig, String> {
    serde_json::from_str(json).map_err(|e| format!("parse JSON: {}", e))
}

/// Save a config to a JSON file (pretty-printed).
pub fn save_to_file<P: AsRef<Path>>(path: P, cfg: &SoundscapeConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(path.as_ref(), json)
        .map_err(|e| format!("write {}: {}", path.as_ref().display(), e))?;
    Ok(())
}

/// Build an example 5.1 forest soundscape demonstrating the routing API.
/// Used by the example file + tests.
pub fn example_forest_5_1() -> SoundscapeConfig {
    let mut wind_weight = HashMap::new();
    wind_weight.insert("wind".to_string(), 1.0);
    wind_weight.insert("birds".to_string(), 0.6);
    wind_weight.insert("creek".to_string(), 0.8);
    wind_weight.insert("subharmonic".to_string(), 0.3);
    let mut dawn_trans = HashMap::new();
    dawn_trans.insert("midday".to_string(), 1.0);

    let mut midday_weight = HashMap::new();
    midday_weight.insert("wind".to_string(), 0.6);
    midday_weight.insert("birds".to_string(), 1.0);
    midday_weight.insert("creek".to_string(), 0.8);
    midday_weight.insert("subharmonic".to_string(), 0.2);
    let mut midday_trans = HashMap::new();
    midday_trans.insert("dusk".to_string(), 1.0);
    midday_trans.insert("dawn".to_string(), 0.2);

    let mut dusk_weight = HashMap::new();
    dusk_weight.insert("wind".to_string(), 0.8);
    dusk_weight.insert("birds".to_string(), 0.4);
    dusk_weight.insert("creek".to_string(), 0.8);
    dusk_weight.insert("subharmonic".to_string(), 0.5);
    let mut dusk_trans = HashMap::new();
    dusk_trans.insert("dawn".to_string(), 1.0);

    SoundscapeConfig {
        name: "Forest 5.1".to_string(),
        layout: Layout::FivePointOne,
        description: "A 5.1 forest scene: birds in the front field, wind in \
                     the surrounds, creek positioned slightly off-center, a \
                     subharmonic pad through the LFE for body.".to_string(),
        author: "Example".to_string(),
        version: 1,
        start_state: "dawn".to_string(),
        layers: vec![
            LayerConfig {
                name: "wind".to_string(),
                pool: vec!["wind_loop_a.wav".to_string(), "wind_loop_b.wav".to_string()],
                channel: String::new(),
                routing: Some(RouteStrategy::Diffuse {
                    channels: vec![(Channel::RL, 0.6), (Channel::RR, 0.6),
                                   (Channel::FL, 0.3), (Channel::FR, 0.3)],
                }),
                crossfade_ms: 4000,
                gain_range: (0.7, 1.0),
                pan_range: (-0.15, 0.15),
                rate_range: (0.97, 1.03),
                trigger_prob: 1.0,
                interval_ms: (12000, 25000),
                duration_ms: Some(30000),
                // Wind breathes: a ~45 s rise-and-fall swell, +/-25% gain.
                swell_period_ms: 45000,
                swell_depth: 0.25,
                timing_jitter: 0.0,
            },
            LayerConfig {
                name: "birds".to_string(),
                pool: vec!["bird_chirp_1.wav".to_string(), "bird_chirp_2.wav".to_string()],
                channel: String::new(),
                routing: Some(RouteStrategy::Pair {
                    a: Channel::FL, b: Channel::FR, gain_per_channel: 0.85,
                }),
                crossfade_ms: 200,
                gain_range: (0.6, 1.0),
                pan_range: (-0.6, 0.6),
                rate_range: (0.92, 1.08),
                trigger_prob: 0.8,
                interval_ms: (3000, 9000),
                duration_ms: Some(2500),
                swell_period_ms: 0,
                swell_depth: 0.0,
                timing_jitter: 0.0,
            },
            LayerConfig {
                name: "creek".to_string(),
                pool: vec!["creek_loop.wav".to_string()],
                channel: String::new(),
                routing: Some(RouteStrategy::Pin {
                    channel: Channel::C, gain: 0.7,
                }),
                crossfade_ms: 2500,
                gain_range: (0.5, 0.8),
                pan_range: (0.0, 0.0),
                rate_range: (1.0, 1.0),
                trigger_prob: 1.0,
                interval_ms: (15000, 15000),
                duration_ms: Some(20000),
                swell_period_ms: 0,
                swell_depth: 0.0,
                timing_jitter: 0.0,
            },
            LayerConfig {
                name: "subharmonic".to_string(),
                pool: vec!["sub_pad_30hz.wav".to_string()],
                channel: String::new(),
                routing: Some(RouteStrategy::Pin {
                    channel: Channel::LFE, gain: 0.5,
                }),
                crossfade_ms: 6000,
                gain_range: (0.3, 0.7),
                pan_range: (0.0, 0.0),
                rate_range: (0.98, 1.02),
                trigger_prob: 0.7,
                interval_ms: (45000, 90000),
                duration_ms: Some(40000),
                swell_period_ms: 0,
                swell_depth: 0.0,
                timing_jitter: 0.0,
            },
        ],
        states: vec![
            StateConfig {
                name: "dawn".to_string(),
                layer_weights: wind_weight,
                dwell_ms: (60000, 120000),
                transitions: dawn_trans,
            },
            StateConfig {
                name: "midday".to_string(),
                layer_weights: midday_weight,
                dwell_ms: (180000, 360000),
                transitions: midday_trans,
            },
            StateConfig {
                name: "dusk".to_string(),
                layer_weights: dusk_weight,
                dwell_ms: (90000, 180000),
                transitions: dusk_trans,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_validates_cleanly() {
        let cfg = example_forest_5_1();
        let report = validate(&cfg, false);
        assert!(report.is_ok(),
                "example should validate, errors: {:?}", report.errors);
    }

    #[test]
    fn example_compiles() {
        let cfg = example_forest_5_1();
        let (scene, routings) = compile(&cfg).expect("compile should succeed");
        assert_eq!(scene.name, "Forest 5.1");
        assert_eq!(scene.layers.len(), 4);
        assert_eq!(scene.states.len(), 3);
        // The birds layer should resolve to FL+FR pair.
        let birds = routings.get("birds").unwrap();
        assert!(birds.gain(Channel::FL) > 0.0);
        assert!(birds.gain(Channel::FR) > 0.0);
        assert_eq!(birds.gain(Channel::C), 0.0);
        // Creek should be pinned to center.
        let creek = routings.get("creek").unwrap();
        assert!(creek.gain(Channel::C) > 0.0);
        assert_eq!(creek.gain(Channel::FL), 0.0);
        // Subharmonic goes to LFE.
        let sub = routings.get("subharmonic").unwrap();
        assert!(sub.gain(Channel::LFE) > 0.0);
    }

    #[test]
    fn validate_catches_duplicate_layer() {
        let mut cfg = example_forest_5_1();
        cfg.layers.push(cfg.layers[0].clone());
        let report = validate(&cfg, false);
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("duplicate layer")));
    }

    #[test]
    fn validate_catches_unknown_layer_in_state() {
        let mut cfg = example_forest_5_1();
        let mut bad_weights = HashMap::new();
        bad_weights.insert("phantom".to_string(), 1.0);
        cfg.states[0].layer_weights = bad_weights;
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("unknown layer 'phantom'")));
    }

    #[test]
    fn validate_catches_unknown_start_state() {
        let mut cfg = example_forest_5_1();
        cfg.start_state = "noon".to_string();
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("start_state 'noon' not in")));
    }

    #[test]
    fn validate_catches_bad_transition() {
        let mut cfg = example_forest_5_1();
        cfg.states[0].transitions.insert("ghost".to_string(), 1.0);
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("transitions to unknown 'ghost'")));
    }

    #[test]
    fn validate_catches_negative_weight() {
        let mut cfg = example_forest_5_1();
        cfg.states[0].layer_weights.insert("wind".to_string(), -0.5);
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("weight for 'wind' is negative")));
    }

    #[test]
    fn validate_warns_on_empty_pool() {
        let mut cfg = example_forest_5_1();
        cfg.layers[0].pool.clear();
        let report = validate(&cfg, false);
        // Empty pool is a warning, not an error.
        assert!(report.is_ok());
        assert!(report.warnings.iter()
            .any(|w| w.contains("empty clip pool")));
    }

    #[test]
    fn validate_rejects_bad_trigger_prob() {
        let mut cfg = example_forest_5_1();
        cfg.layers[0].trigger_prob = 1.5;
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("trigger_prob 1.5")));
    }

    #[test]
    fn validate_rejects_inverted_ranges() {
        let mut cfg = example_forest_5_1();
        cfg.layers[0].gain_range = (1.0, 0.5);
        let report = validate(&cfg, false);
        assert!(report.errors.iter()
            .any(|e| e.contains("gain_range")));
    }

    #[test]
    fn parse_json_roundtrip() {
        let cfg = example_forest_5_1();
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SoundscapeConfig = parse(&json).unwrap();
        assert_eq!(parsed.name, cfg.name);
        assert_eq!(parsed.layers.len(), cfg.layers.len());
        assert_eq!(parsed.states.len(), cfg.states.len());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let cfg = example_forest_5_1();
        let path = std::env::temp_dir()
            .join(format!("bo_test_ss_{}.json", std::process::id()));
        save_to_file(&path, &cfg).unwrap();
        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.name, cfg.name);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_channel_field_still_works() {
        // A config with the old free-form `channel` field but no
        // `routing` should still compile.
        let mut cfg = example_forest_5_1();
        for layer in &mut cfg.layers {
            layer.routing = None;
            layer.channel = match layer.name.as_str() {
                "wind"        => "rear".to_string(),
                "birds"       => "front".to_string(),
                "creek"       => "vocals".to_string(),
                "subharmonic" => "LFE".to_string(),
                _ => String::new(),
            };
        }
        let (_, routings) = compile(&cfg).expect("legacy compile");
        // "rear" → Pair(RL,RR)
        assert!(routings.get("wind").unwrap().gain(Channel::RL) > 0.0);
        // "vocals" → Pin(C)
        assert!(routings.get("creek").unwrap().gain(Channel::C) > 0.0);
        // "LFE" → Pin(LFE)
        assert!(routings.get("subharmonic").unwrap().gain(Channel::LFE) > 0.0);
    }

    #[test]
    fn compile_5_1_scene_to_stereo_downmixes() {
        // Same scene but compiled for stereo: every channel routing
        // must end up assigning ONLY FL and/or FR (no orphaned RLs).
        let mut cfg = example_forest_5_1();
        cfg.layout = Layout::Stereo;
        let (_, routings) = compile(&cfg).expect("compile stereo");
        for (name, routing) in &routings {
            for (ch, _) in &routing.channel_gains {
                assert!(matches!(ch, Channel::FL | Channel::FR),
                    "layer '{}' assigned to {:?} under stereo", name, ch);
            }
        }
    }

    #[test]
    fn compile_resolves_start_state_by_name() {
        let mut cfg = example_forest_5_1();
        cfg.start_state = "midday".to_string();
        let (scene, _) = compile(&cfg).unwrap();
        // dawn(0), midday(1), dusk(2)
        assert_eq!(scene.start_state, 1);
    }

    #[test]
    fn validation_report_merge() {
        let mut a = ValidationReport { errors: vec!["a".into()], warnings: vec!["w1".into()] };
        let b = ValidationReport { errors: vec!["b".into()], warnings: vec!["w2".into()] };
        a = a.merge(b);
        assert_eq!(a.errors.len(), 2);
        assert_eq!(a.warnings.len(), 2);
    }
}

// Integration smoke tests against the bundled example files. They sit
// behind a #[cfg(test)] gate but use real filesystem paths so they
// only run from a workspace checkout.
#[cfg(test)]
mod example_smoke_tests {
    use super::*;

    fn example_path(name: &str) -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("examples");
        p.push(name);
        p
    }

    #[test]
    fn forest_5_1_example_loads_and_compiles() {
        let p = example_path("forest_5_1.soundscape.json");
        let cfg = load_from_file(&p).expect("load forest_5_1");
        assert_eq!(cfg.name, "Forest — 5.1 Surround");
        assert_eq!(cfg.layout, Layout::FivePointOne);
        let (scene, routings) = compile(&cfg).expect("compile");
        assert_eq!(scene.layers.len(), 4);
        assert!(routings.get("subharmonic").unwrap().gain(Channel::LFE) > 0.0);
    }

    #[test]
    fn ocean_quad_example_loads_and_compiles() {
        let p = example_path("ocean_quad.soundscape.json");
        let cfg = load_from_file(&p).expect("load ocean_quad");
        assert_eq!(cfg.name, "Ocean — Quad");
        assert_eq!(cfg.layout, Layout::Quad);
        let (scene, routings) = compile(&cfg).expect("compile");
        assert_eq!(scene.layers.len(), 3);
        // waves should hit FL + FR
        let waves = routings.get("waves").unwrap();
        assert!(waves.gain(Channel::FL) > 0.0);
        assert!(waves.gain(Channel::FR) > 0.0);
        // whale_distant should hit RL + RR
        let whale = routings.get("whale_distant").unwrap();
        assert!(whale.gain(Channel::RL) > 0.0);
        assert!(whale.gain(Channel::RR) > 0.0);
    }

    #[test]
    fn center_pinned_layer_resolves_to_present_channel_on_stereo() {
        // synth_pad pins its single "pad" layer to C (center). On a
        // Stereo layout there is no center speaker, so before the fix
        // the layer kept channel "C" — which the mesh can never claim,
        // leaving the pad silent. After the fix, compile() rewrites the
        // layer's effective channel to a channel that actually exists
        // in the layout (FL via the phantom-center downmix), so the
        // mesh has a real target and the routing carries front gains.
        let p = example_path("synth_pad.soundscape.json");
        let cfg = load_from_file(&p).expect("load synth_pad");
        assert_eq!(cfg.layout, Layout::Stereo);
        let (scene, routings) = compile(&cfg).expect("compile");
        let pad_layer = scene.layers.iter().find(|l| l.name == "pad").unwrap();
        // The runtime layer's channel must be one the layout contains.
        let ch = Channel::parse(&pad_layer.channel).expect("valid channel");
        assert!(cfg.layout.contains(ch),
            "pad layer resolved to '{}' which isn't in the Stereo layout",
            pad_layer.channel);
        // And the resolved routing must put audio on the front pair.
        let pad_routing = routings.get("pad").unwrap();
        assert!(pad_routing.gain(Channel::FL) > 0.0
                && pad_routing.gain(Channel::FR) > 0.0,
            "center pad should fold onto the front pair on stereo");
    }
}
