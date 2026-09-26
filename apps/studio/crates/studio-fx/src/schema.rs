//! schema — what each effect's parameters ARE, declared once.
//!
//! [`instantiate`](crate::instantiate) has always read its parameters with
//! `p(q, "name", default)`, which made the vocabulary implicit: to build a
//! control for the compressor you had to read the compressor's arm and
//! guess the range. This module is that vocabulary written down — key,
//! label, unit, range, scale, default, and how the count parameters
//! (`bands`, `notes`) expand into repeated groups — so an editor renders
//! ANY effect from the description and a new effect gets controls by
//! adding one entry here, not one panel.
//!
//! The defaults here are the defaults `instantiate` uses; the test
//! `every_kind_instantiates_from_its_own_defaults` holds the two together.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Scale {
    Linear,
    /// Frequencies: equal screen distance per octave.
    Log,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A continuous value with a unit suffix ("dB", "Hz", "ms", ":1", "").
    Float { min: f32, max: f32, unit: &'static str, scale: Scale, step: f32 },
    /// A whole number shown as such.
    Int { min: i32, max: i32, unit: &'static str },
    Bool,
    /// Stored as the index, shown as the label.
    Enum(&'static [&'static str]),
    /// A structural count: changing it adds or removes a repeated group
    /// (`bands` → one band section per band). Shown with +/- rather
    /// than a slider.
    Count { min: usize, max: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub default: f32,
    /// Section header this belongs under, for repeated groups ("band 1").
    pub group: Option<String>,
}

impl ParamSpec {
    pub fn is_count(&self) -> bool { matches!(self.kind, Kind::Count { .. }) }

    /// The value as a person reads it.
    pub fn format(&self, v: f32) -> String {
        match &self.kind {
            Kind::Float { unit, .. } => match *unit {
                "Hz" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
                "Hz" => format!("{v:.0} Hz"),
                "dB" => format!("{v:+.1} dB"),
                "ms" if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
                "ms" => format!("{v:.0} ms"),
                ":1" => format!("{v:.1}:1"),
                "%" => format!("{:.0}%", v * 100.0),
                "st" => format!("{v:+.0} st"),
                "x" => format!("{v:.2}×"),
                "" => format!("{v:.2}"),
                u => format!("{v:.2} {u}"),
            },
            Kind::Int { unit, .. } => if unit.is_empty() { format!("{}", v as i64) } else { format!("{} {unit}", v as i64) },
            Kind::Bool => if v > 0.5 { "on".into() } else { "off".into() },
            Kind::Enum(names) => names.get(v as usize).copied().unwrap_or("?").to_string(),
            Kind::Count { .. } => format!("{}", v as usize),
        }
    }

    /// Clamp to the declared range and snap ints/bools/enums.
    pub fn clamp(&self, v: f32) -> f32 {
        match &self.kind {
            Kind::Float { min, max, .. } => v.clamp(*min, *max),
            Kind::Int { min, max, .. } => (v.round()).clamp(*min as f32, *max as f32),
            Kind::Bool => if v > 0.5 { 1.0 } else { 0.0 },
            Kind::Enum(names) => v.round().clamp(0.0, (names.len().saturating_sub(1)) as f32),
            Kind::Count { min, max } => v.round().clamp(*min as f32, *max as f32),
        }
    }
}

fn f(key: &str, label: &str, min: f32, max: f32, unit: &'static str, default: f32) -> ParamSpec {
    let step = if unit == "Hz" { 0.0 } else { ((max - min) / 200.0).max(0.001) };
    ParamSpec { key: key.into(), label: label.into(), default,
        kind: Kind::Float { min, max, unit, scale: if unit == "Hz" { Scale::Log } else { Scale::Linear }, step }, group: None }
}
fn b(key: &str, label: &str, default: bool) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind: Kind::Bool, default: if default { 1.0 } else { 0.0 }, group: None }
}
fn e(key: &str, label: &str, names: &'static [&'static str], default: usize) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind: Kind::Enum(names), default: default as f32, group: None }
}
fn count(key: &str, label: &str, min: usize, max: usize, default: usize) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind: Kind::Count { min, max }, default: default as f32, group: None }
}
fn int(key: &str, label: &str, min: i32, max: i32, unit: &'static str, default: i32) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind: Kind::Int { min, max, unit }, default: default as f32, group: None }
}

const SAT_PROFILES: &[&str] = &["tube", "tape", "console", "diode"];
const BLEND_MODES: &[&str] = &["normal", "add", "multiply", "screen", "overlay"];
const BAND_KINDS: &[&str] = &["peaking", "low shelf", "high shelf", "low pass", "high pass"];
const SCALE_MODES: &[&str] = &["chromatic", "major", "minor", "pentatonic", "custom"];
const NOTE_NAMES: &[&str] = &["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Every kind `instantiate` knows, with a one-line description for a
/// picker. Order is the order a chain menu shows.
/// Kinds that are STEREO and run on a bus's stereo path in the mixdown
/// (studio-audio), not through `instantiate`'s per-sample chain -- listed so
/// the schema can offer their controls while the chain contract stays exact.
/// On a clip they do nothing (clips are processed per sample).
pub const BUS_STEREO_KINDS: &[&str] = &["imager", "doubler"];

pub const KINDS: &[(&str, &str)] = &[
    ("gain", "level, in dB"),
    ("eq", "parametric equaliser, up to 8 bands"),
    ("compressor", "dynamics: threshold, ratio, attack, release"),
    ("limiter", "brickwall ceiling with lookahead"),
    ("gate", "silence below a threshold"),
    ("multiband", "per-band gain across crossover points"),
    ("saturation", "tube / tape / console / diode colour"),
    ("adaptivesat", "saturation that follows notes and sibilance"),
    ("autotune", "pitch correction to a scale"),
    ("imager", "stereo width in three bands (busses)"),
    ("doubler", "doubled voice: two detuned copies left and right (busses)"),
    ("comb", "comb delay: metallic resonance"),
    ("sub", "sub-octave"),
    ("shift", "pitch shift, granular"),
    ("crush", "bit crusher"),
    ("tapestop", "tape stop"),
    ("halftime", "half-time"),
    ("reverb", "room: size, damping, predelay, width"),
    ("delay", "echo: time, feedback, damping, ping-pong"),
    ("transform", "position, scale, rotation, opacity (video)"),
    ("color", "lift / gamma / gain / saturation (video)"),
    ("retime", "speed ramp (video)"),
    ("blend", "blend mode and opacity onto the tracks below (video)"),
];

/// The static part of a kind's schema: everything except the repeated
/// groups a count expands into. `None` for a kind this crate does not
/// know — the editor then shows the raw map, which is not nothing.
pub fn schema(kind: &str) -> Option<Vec<ParamSpec>> {
    Some(match kind {
        // `level`: a LINEAR multiplier on top of `db` (1 = as set). An automation
        // lane MULTIPLIES a parameter's value, so a ride on `db` cannot reach
        // silence (0 dB x anything is 0 dB); a ride on `level` is the linear
        // gain itself -- FL's Fruity Balance volume rides, mutes included.
        "gain" => vec![f("db", "gain", -60.0, 24.0, "dB", 0.0), f("level", "level", 0.0, 4.0, "x", 1.0)],
        "eq" => vec![count("bands", "bands", 1, 8, 1)],
        "compressor" => vec![
            f("threshold", "threshold", -60.0, 0.0, "dB", -18.0),
            f("ratio", "ratio", 1.0, 20.0, ":1", 4.0),
            f("attack", "attack", 0.1, 200.0, "ms", 10.0),
            f("release", "release", 5.0, 2000.0, "ms", 120.0),
            f("knee", "knee", 0.0, 24.0, "dB", 6.0),
            f("makeup", "makeup", -12.0, 24.0, "dB", 0.0),
        ],
        "limiter" => vec![
            f("ceiling", "ceiling", -12.0, 0.0, "dB", -0.3),
            f("lookahead", "lookahead", 0.0, 20.0, "ms", 5.0),
            f("release", "release", 5.0, 500.0, "ms", 50.0),
        ],
        "gate" => vec![
            f("threshold", "threshold", -80.0, 0.0, "dB", -40.0),
            f("hold", "hold", 0.0, 500.0, "ms", 80.0),
            f("release", "release", 1.0, 500.0, "ms", 30.0),
        ],
        "multiband" => vec![count("bands", "crossovers", 1, 6, 2)],
        "saturation" => vec![
            e("profile", "profile", SAT_PROFILES, 0),
            f("drive", "drive", 0.0, 1.0, "%", 0.0),
            f("bias", "bias", -1.0, 1.0, "", 0.0),
            f("mix", "mix", 0.0, 1.0, "%", 1.0),
            b("auto_gain", "auto gain", true),
        ],
        "adaptivesat" => vec![
            e("profile", "profile", SAT_PROFILES, 0),
            f("drive", "drive", 0.0, 1.0, "%", 0.35),
            f("note_amount", "note amount", 0.0, 1.0, "%", 0.5),
            f("sib_amount", "sibilance amount", 0.0, 1.0, "%", 0.5),
            b("tied", "tie channels", false),
            f("bias", "bias", -1.0, 1.0, "", 0.0),
            f("mix", "mix", 0.0, 1.0, "%", 1.0),
        ],
        "doubler" => vec![
            f("amount", "amount", 0.0, 1.0, "%", 0.5),
            f("spread", "spread", 0.0, 1.0, "%", 0.8),
            f("detune", "detune", 0.0, 50.0, "cents", 8.0),
            f("delay", "delay", 5.0, 45.0, "ms", 18.0),
        ],
        "imager" => vec![
            f("low", "width below 200 Hz", 0.0, 2.0, "", 1.0),
            f("mid", "width 200 Hz - 4 kHz", 0.0, 2.0, "", 1.0),
            f("high", "width above 4 kHz", 0.0, 2.0, "", 1.0),
        ],
        "autotune" => vec![
            e("root", "root", NOTE_NAMES, 0),
            e("mode", "scale", SCALE_MODES, 0),
            f("speed", "retune speed", 0.0, 400.0, "ms", 30.0),
            // hard hold: speed 0, slide off. Slide into key: slower, slide on.
            f("slide", "slide into each note", 0.0, 1.0, "", 0.0),
            f("amount", "amount", 0.0, 1.0, "%", 1.0),
            f("tolerance", "leave notes within", 0.0, 100.0, "cents", 0.0),
            f("voiced", "voiced threshold", 0.0, 1.0, "", 0.15),
            count("notes", "custom notes", 0, 24, 0),
        ],
        "comb" => vec![
            f("delay", "delay", 1.0, 240.0, "ms", 70.0),
            f("feedback", "feedback", 0.0, 0.98, "%", 0.6),
            f("damp", "damping", 200.0, 20000.0, "Hz", 1500.0),
            f("mix", "mix", 0.0, 1.0, "%", 0.35),
        ],
        "sub" => vec![f("mix", "mix", 0.0, 1.0, "%", 0.25)],
        "shift" => vec![
            f("semitones", "shift", -24.0, 24.0, "st", -3.0),
            f("grain", "grain", 10.0, 200.0, "ms", 60.0),
        ],
        "crush" => vec![int("bits", "bits", 1, 16, "bit", 16)],
        "tapestop" => vec![
            f("start", "start", 0.0, 60.0, "s", 0.0),
            f("length", "length", 0.05, 10.0, "s", 1.0),
        ],
        "halftime" => vec![f("window", "window", 0.05, 2.0, "s", 0.5)],
        "reverb" => vec![
            f("size", "size", 0.0, 1.0, "", 0.5),
            f("damp", "damping", 0.0, 1.0, "", 0.4),
            f("predelay", "predelay", 0.0, 200.0, "ms", 10.0),
            f("width", "width", 0.0, 1.0, "", 1.0),
            f("mix", "mix", 0.0, 1.0, "%", 0.3),
        ],
        "delay" => vec![
            // 321.4 ms is the dotted eighth at 140 BPM — the trap default;
            // the shell's sync verb rewrites it from the project tempo.
            f("time", "time", 1.0, 2000.0, "ms", 321.4),
            f("feedback", "feedback", 0.0, 0.98, "%", 0.4),
            f("damp", "damping", 200.0, 20000.0, "Hz", 5000.0),
            b("pingpong", "ping-pong", false),
            f("mix", "mix", 0.0, 1.0, "%", 0.3),
        ],
        "transform" => vec![
            f("x", "x", -1.0, 1.0, "", 0.0), f("y", "y", -1.0, 1.0, "", 0.0),
            f("scale_x", "scale x", 0.01, 4.0, "x", 1.0), f("scale_y", "scale y", 0.01, 4.0, "x", 1.0),
            f("rotation", "rotation", -180.0, 180.0, "°", 0.0),
            f("anchor_x", "anchor x", 0.0, 1.0, "", 0.5), f("anchor_y", "anchor y", 0.0, 1.0, "", 0.5),
            f("opacity", "opacity", 0.0, 1.0, "%", 1.0),
            f("crop_l", "crop left", 0.0, 1.0, "%", 0.0), f("crop_t", "crop top", 0.0, 1.0, "%", 0.0),
            f("crop_r", "crop right", 0.0, 1.0, "%", 0.0), f("crop_b", "crop bottom", 0.0, 1.0, "%", 0.0),
        ],
        "color" => vec![
            f("lift_r", "lift R", -1.0, 1.0, "", 0.0), f("lift_g", "lift G", -1.0, 1.0, "", 0.0), f("lift_b", "lift B", -1.0, 1.0, "", 0.0),
            f("gamma_r", "gamma R", 0.1, 4.0, "", 1.0), f("gamma_g", "gamma G", 0.1, 4.0, "", 1.0), f("gamma_b", "gamma B", 0.1, 4.0, "", 1.0),
            f("gain_r", "gain R", 0.0, 4.0, "x", 1.0), f("gain_g", "gain G", 0.0, 4.0, "x", 1.0), f("gain_b", "gain B", 0.0, 4.0, "x", 1.0),
            f("saturation", "saturation", 0.0, 3.0, "x", 1.0),
        ],
        "retime" => vec![
            f("from_speed", "from speed", 0.1, 8.0, "x", 1.0),
            f("to_speed", "to speed", 0.1, 8.0, "x", 1.0),
            b("preserve_pitch", "preserve pitch", true),
            b("reverse", "reverse", false),
        ],
        "blend" => vec![
            e("mode", "mode", BLEND_MODES, 0),
            f("opacity", "opacity", 0.0, 1.0, "%", 1.0),
        ],
        _ => return None,
    })
}

/// The repeated group for index `i` of a count parameter — what one band
/// of the EQ is, what one crossover of the multiband is.
fn group_specs(kind: &str, count_key: &str, i: usize) -> Vec<ParamSpec> {
    let g = |mut s: ParamSpec, name: &str| { s.group = Some(name.into()); s };
    match (kind, count_key) {
        ("eq", "bands") => {
            let name = format!("band {}", i + 1);
            vec![
                g(e(&format!("kind{i}"), "type", BAND_KINDS, 0), &name),
                g(f(&format!("freq{i}"), "frequency", 20.0, 20000.0, "Hz", 1000.0), &name),
                g(f(&format!("gain{i}"), "gain", -24.0, 24.0, "dB", 0.0), &name),
                g(f(&format!("q{i}"), "Q", 0.1, 10.0, "", 0.707), &name),
            ]
        }
        ("multiband", "bands") => {
            // Crossover i, then band i's gain/bypass; the last band (N) has
            // no crossover above it.
            let name = format!("band {}", i + 1);
            vec![
                g(f(&format!("freq{i}"), "crossover", 40.0, 16000.0, "Hz", 200.0 * 2f32.powi(i as i32 + 1)), &name),
                g(f(&format!("band{i}_gain"), "gain", -24.0, 24.0, "dB", 0.0), &name),
                g(b(&format!("band{i}_bypass"), "bypass", true), &name),
            ]
        }
        ("autotune", "notes") => vec![g(e(&format!("note{i}"), "note", NOTE_NAMES, 0), "custom notes")],
        _ => Vec::new(),
    }
}

/// The full, expanded schema for THIS effect: static specs, then one group
/// per index of each count, using the effect's current count (or the
/// default when unset).
pub fn expand(effect: &studio_core::Effect) -> Option<Vec<ParamSpec>> {
    let base = schema(&effect.kind)?;
    let mut out = Vec::with_capacity(base.len() * 3);
    for spec in &base {
        out.push(spec.clone());
        if let Kind::Count { .. } = spec.kind {
            let n = effect.params.get(&spec.key).copied().unwrap_or(spec.default) as usize;
            for i in 0..n {
                out.extend(group_specs(&effect.kind, &spec.key, i));
            }
        }
    }
    Some(out)
}

/// A fresh params map holding every default of a kind's expanded schema.
pub fn defaults(kind: &str) -> BTreeMap<String, f32> {
    let probe = studio_core::Effect { kind: kind.into(), params: BTreeMap::new(), enabled: true };
    expand(&probe).unwrap_or_default().into_iter().map(|s| (s.key, s.default)).collect()
}

/// The value an editor shows: the effect's own, or the schema default.
pub fn value(effect: &studio_core::Effect, spec: &ParamSpec) -> f32 {
    effect.params.get(&spec.key).copied().unwrap_or(spec.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_instantiates_from_its_own_defaults() {
        for (kind, _) in KINDS {
            assert!(schema(kind).is_some(), "{kind} has no schema");
            let fx = studio_core::Effect { kind: (*kind).into(), params: defaults(kind), enabled: true };
            if BUS_STEREO_KINDS.contains(kind) {
                assert!(crate::instantiate(&fx, 48_000.0).is_none(), "{kind} runs on the bus's stereo path, not here");
                continue;
            }
            assert!(crate::instantiate(&fx, 48_000.0).is_some(), "{kind} did not instantiate from schema defaults");
        }
    }

    #[test]
    fn a_count_expands_into_that_many_groups_and_tracks_the_effects_value() {
        let mut fx = studio_core::Effect { kind: "eq".into(), params: BTreeMap::new(), enabled: true };
        let one = expand(&fx).unwrap();
        assert_eq!(one.len(), 1 + 4, "default one band: count + four per band");
        assert_eq!(one[1].group.as_deref(), Some("band 1"));
        fx.params.insert("bands".into(), 3.0);
        let three = expand(&fx).unwrap();
        assert_eq!(three.len(), 1 + 12);
        assert!(three.iter().any(|s| s.key == "freq2"));
        assert_eq!(three.iter().filter(|s| s.group.as_deref() == Some("band 3")).count(), 4);
    }

    #[test]
    fn formatting_reads_like_a_plugin_not_a_float() {
        let s = schema("compressor").unwrap();
        assert_eq!(s[0].format(-18.0), "-18.0 dB");
        assert_eq!(s[1].format(4.0), "4.0:1");
        assert_eq!(s[2].format(10.0), "10 ms");
        assert_eq!(s[3].format(1500.0), "1.50 s");
        let eq = expand(&studio_core::Effect { kind: "eq".into(), params: BTreeMap::new(), enabled: true }).unwrap();
        let freq = eq.iter().find(|s| s.key == "freq0").unwrap();
        assert_eq!(freq.format(1000.0), "1.00 kHz");
        assert_eq!(freq.format(250.0), "250 Hz");
        assert!(matches!(freq.kind, Kind::Float { scale: Scale::Log, .. }), "frequency is log-scaled");
        let kind = eq.iter().find(|s| s.key == "kind0").unwrap();
        assert_eq!(kind.format(1.0), "low shelf");
        let sat = schema("saturation").unwrap();
        assert_eq!(sat[1].format(0.35), "35%");
        assert_eq!(sat[4].format(1.0), "on");
    }

    #[test]
    fn clamp_snaps_each_kind_to_its_domain() {
        let s = schema("crush").unwrap();
        assert_eq!(s[0].clamp(3.7), 4.0);
        assert_eq!(s[0].clamp(99.0), 16.0);
        let sat = schema("saturation").unwrap();
        assert_eq!(sat[0].clamp(9.0), 3.0, "enum clamps to the last name");
        assert_eq!(sat[4].clamp(0.2), 0.0);
        let eq = schema("eq").unwrap();
        assert_eq!(eq[0].clamp(0.0), 1.0);
        assert_eq!(eq[0].clamp(12.0), 8.0);
    }

    #[test]
    fn an_unknown_kind_has_no_schema_rather_than_a_wrong_one() {
        // This test's example used to be "reverb" — which then got built.
        // A name no build will ever register, so the claim stays true.
        assert!(schema("no-such-effect").is_none());
        assert!(expand(&studio_core::Effect { kind: "no-such-effect".into(), params: BTreeMap::new(), enabled: true }).is_none());
        assert!(schema("reverb").is_some(), "and the one that used to stand in for unknown is known now");
    }
}
