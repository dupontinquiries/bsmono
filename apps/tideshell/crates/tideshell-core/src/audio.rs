//! Voicing: the second axis of the volume control. `tone` runs from -1
//! ("quiet room": soft) through 0 (flat) to +1 ("loud place": cut through),
//! and pans across a fixed chain of effects instead of asking the user to
//! set each one:
//!
//! ```text
//!  tone  -1 ............... 0 ............... +1
//!  low-pass   3.5 kHz ->  off
//!  high-pass             off  ->  250 Hz
//!  presence              0 dB ->  +4 dB  (peaking, 2.5 kHz, Q 1.0)
//!  compression           0    ->  1      (only when a compressor plugin exists)
//!  trim         -3 dB -> 0 dB (a soft voicing is also a touch quieter)
//! ```
//!
//! The chain lives in a PipeWire filter-chain sink; `pw_params` names the
//! control ports as filter-chain exposes them (`<node>:<port>`), and
//! `filter_chain_config` is the sink's config file. Volume itself stays on
//! the sink (wpctl), it is not part of the chain.

pub const HP_MAX_HZ: f32 = 250.0;
pub const LP_MIN_HZ: f32 = 3500.0;
pub const LP_OFF_HZ: f32 = 20000.0;
pub const HP_OFF_HZ: f32 = 20.0;
pub const PRESENCE_HZ: f32 = 2500.0;
pub const PRESENCE_MAX_DB: f32 = 4.0;
pub const SOFT_TRIM_DB: f32 = -3.0;
pub const SINK_NAME: &str = "tideshell-voicing";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicingParams {
    pub hp_hz: f32,
    pub lp_hz: f32,
    pub presence_db: f32,
    /// 0..1, for a compressor if one is in the chain.
    pub compression: f32,
    pub trim_db: f32,
}

/// The effects for a tone position. Continuous, flat at 0.
pub fn params(tone: f32) -> VoicingParams {
    let t = tone.clamp(-1.0, 1.0);
    let up = t.max(0.0);
    let down = (-t).max(0.0);
    // Frequencies move on a log scale, which is how they are heard.
    let lerp_log = |a: f32, b: f32, x: f32| (a.ln() + (b.ln() - a.ln()) * x).exp();
    VoicingParams {
        hp_hz: lerp_log(HP_OFF_HZ, HP_MAX_HZ, up),
        lp_hz: lerp_log(LP_OFF_HZ, LP_MIN_HZ, down),
        presence_db: PRESENCE_MAX_DB * up,
        compression: up,
        trim_db: SOFT_TRIM_DB * down,
    }
}

/// filter-chain control values, as `pw-cli set-param <node> Props` wants
/// them: `params = [ "hp:Freq" 250.0 ... ]`.
pub fn pw_params(p: &VoicingParams) -> Vec<(String, f32)> {
    vec![
        ("hp:Freq".into(), p.hp_hz),
        ("lp:Freq".into(), p.lp_hz),
        ("presence:Freq".into(), PRESENCE_HZ),
        ("presence:Q".into(), 1.0),
        ("presence:Gain".into(), p.presence_db),
        ("trim:Mult".into(), 10f32.powf(p.trim_db / 20.0)),
    ]
}

/// The Props param text for pw-cli (SPA JSON).
pub fn props_text(p: &VoicingParams) -> String {
    let items: Vec<String> = pw_params(p).iter().map(|(k, v)| format!("\"{k}\" {v:.3}")).collect();
    format!("{{ params = [ {} ] }}", items.join(" "))
}

/// The filter-chain sink as PipeWire user config
/// (`~/.config/pipewire/pipewire.conf.d/tideshell-voicing.conf`). Built-in
/// filters only, so it works on any PipeWire; a compressor is a later,
/// optional LADSPA stage.
pub fn filter_chain_config() -> String {
    format!(
        r#"# tideshell voicing sink: high-pass -> presence -> low-pass -> trim.
# Apps play to "{sink}"; tideshell sets the controls live (pw-cli set-param).
context.modules = [
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = "tideshell voicing"
            media.name       = "tideshell voicing"
            filter.graph = {{
                nodes = [
                    {{ type = builtin label = bq_highpass name = hp       control = {{ "Freq" = {hp:.1} "Q" = 0.7 }} }}
                    {{ type = builtin label = bq_peaking  name = presence control = {{ "Freq" = {pf:.1} "Q" = 1.0 "Gain" = 0.0 }} }}
                    {{ type = builtin label = bq_lowpass  name = lp       control = {{ "Freq" = {lp:.1} "Q" = 0.7 }} }}
                    {{ type = builtin label = linear      name = trim     control = {{ "Mult" = 1.0 "Add" = 0.0 }} }}
                ]
                links = [
                    {{ output = "hp:Out" input = "presence:In" }}
                    {{ output = "presence:Out" input = "lp:In" }}
                    {{ output = "lp:Out" input = "trim:In" }}
                ]
            }}
            capture.props = {{
                node.name   = "{sink}"
                media.class = Audio/Sink
                audio.position = [ FL FR ]
            }}
            playback.props = {{
                node.name   = "{sink}.out"
                node.passive = true
                audio.position = [ FL FR ]
            }}
        }}
    }}
]
"#,
        sink = SINK_NAME,
        hp = HP_OFF_HZ,
        pf = PRESENCE_HZ,
        lp = LP_OFF_HZ
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_in_the_middle_soft_below_cutting_above() {
        let f = params(0.0);
        assert!((f.hp_hz - HP_OFF_HZ).abs() < 0.01 && (f.lp_hz - LP_OFF_HZ).abs() < 0.5 && f.presence_db == 0.0 && f.trim_db == 0.0);
        let up = params(1.0);
        assert!((up.hp_hz - HP_MAX_HZ).abs() < 0.01 && up.presence_db == PRESENCE_MAX_DB && up.compression == 1.0);
        assert!((up.lp_hz - LP_OFF_HZ).abs() < 0.5, "cutting through leaves the top open");
        let down = params(-1.0);
        assert!((down.lp_hz - LP_MIN_HZ).abs() < 0.01 && down.trim_db == SOFT_TRIM_DB && down.presence_db == 0.0);
        assert!((down.hp_hz - HP_OFF_HZ).abs() < 0.01, "soft keeps the bass");
        assert_eq!(params(7.0), params(1.0), "clamped");
    }

    #[test]
    fn the_axis_is_monotonic_in_every_effect() {
        let mut prev = params(-1.0);
        for i in 1..=40 {
            let p = params(-1.0 + i as f32 / 20.0);
            assert!(p.hp_hz >= prev.hp_hz - 1e-3 && p.lp_hz >= prev.lp_hz - 1e-3 && p.presence_db >= prev.presence_db - 1e-6 && p.trim_db >= prev.trim_db - 1e-6);
            prev = p;
        }
        // Half-way up: the high-pass is at the log midpoint, not the linear one.
        let half = params(0.5).hp_hz;
        assert!((half - (HP_OFF_HZ * HP_MAX_HZ).sqrt()).abs() < 0.5, "{half}");
    }

    #[test]
    fn pipewire_params_and_config_name_the_same_nodes() {
        let cfg = filter_chain_config();
        for (k, _) in pw_params(&params(0.3)) {
            let node = k.split(':').next().unwrap();
            assert!(cfg.contains(&format!("name = {node} ")), "config has no node {node}");
        }
        assert!(cfg.contains(SINK_NAME) && cfg.contains("bq_highpass") && cfg.contains("Audio/Sink"));
        let t = props_text(&params(1.0));
        assert!(t.starts_with("{ params = [ ") && t.contains("\"hp:Freq\" 250.000") && t.contains("\"presence:Gain\" 4.000"), "{t}");
        let soft = pw_params(&params(-1.0));
        let trim = soft.iter().find(|(k, _)| k == "trim:Mult").unwrap().1;
        assert!((trim - 0.708).abs() < 0.002, "-3 dB as a linear multiplier: {trim}");
    }
}

// ── warmth: the speaker slider ───────────────────────────────────────────────
//
// A phone speaker cannot move enough air for real bass, so two different
// tricks are used to fake it, and they fail in opposite directions:
//
// * **Harmonics** (bankstown). Synthesises the overtones of a bass note the
//   driver cannot produce; the ear infers the missing fundamental. Cheap in
//   excursion — the driver only ever reproduces frequencies it can actually
//   make — but overdone it sounds buzzy and hollow.
// * **Compression**. Raises the quiet parts, so everything is closer to the
//   loudest the speaker can manage. Makes a track feel bigger, but flattens
//   it, and pushed hard it pumps.
//
// One slider, blending between them, because they are complements rather
// than alternatives: harmonics first (free in excursion), compression
// second (costly in dynamics).
//
// A LIMITER is not on the slider. It is always on and it is not optional:
// bankstown ADDS energy, so peaks that used to sit at full scale now clip,
// and the whole point of a bass exciter is that people then turn the volume
// up because it sounds good. The limiter is what stops that from reaching
// the voice coil as a square wave.

/// Threshold for the always-on limiter, in dBFS.
pub const LIMIT_DB: f32 = -1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warmth {
    /// bankstown `amt`, 0..1.
    pub bass_amt: f32,
    /// bankstown `blend`, how much of the synthesised signal is mixed in.
    pub bass_blend: f32,
    /// Compressor ratio, 1.0 = off.
    pub comp_ratio: f32,
    /// Compressor threshold in dBFS; less negative means it acts less.
    pub comp_threshold_db: f32,
    /// Make-up gain, linear.
    pub makeup: f32,
    /// Always-on, never zero.
    pub limit_db: f32,
}

/// The slider, 0..1. Zero is bypass — genuinely bypass, so the setting can
/// be compared against untouched audio rather than against "a bit less".
///
/// The first half adds harmonics only. The second half holds harmonics near
/// their useful maximum and brings the compressor in, because past a point
/// more harmonics only buzz.
pub fn warmth(x: f32) -> Warmth {
    let x = x.clamp(0.0, 1.0);
    let first = (x * 2.0).min(1.0);
    let second = ((x - 0.5) * 2.0).max(0.0);
    Warmth {
        bass_amt: 0.75 * first,
        bass_blend: 0.35 + 0.35 * first,
        comp_ratio: 1.0 + 2.5 * second,
        comp_threshold_db: -12.0 * second,
        // Make-up follows the compression, not the harmonics: bankstown is
        // already louder by construction.
        makeup: 1.0 + 0.25 * second,
        limit_db: LIMIT_DB,
    }
}

/// bankstown's own control names, for a filter-chain node.
pub fn bankstown_controls(w: &Warmth) -> Vec<(String, f32)> {
    vec![
        ("bankstown:bypass".into(), if w.bass_amt <= 0.0 { 1.0 } else { 0.0 }),
        ("bankstown:amt".into(), w.bass_amt),
        ("bankstown:blend".into(), w.bass_blend),
        // The band it synthesises into, and the final high-pass that keeps
        // sub-bass the driver cannot make out of the cone entirely.
        ("bankstown:floor".into(), 45.0),
        ("bankstown:ceil".into(), 180.0),
        ("bankstown:final_hp".into(), 40.0),
        ("bankstown:sat_second".into(), 0.3),
        ("bankstown:sat_third".into(), 0.6),
    ]
}

#[cfg(test)]
mod warmth_tests {
    use super::*;

    #[test]
    fn zero_is_a_real_bypass() {
        let w = warmth(0.0);
        assert_eq!(w.bass_amt, 0.0);
        assert_eq!(w.comp_ratio, 1.0, "no compression either");
        assert_eq!(bankstown_controls(&w)[0], ("bankstown:bypass".to_string(), 1.0));
    }

    #[test]
    fn harmonics_come_first_and_compression_second() {
        let lo = warmth(0.25);
        let mid = warmth(0.5);
        let hi = warmth(1.0);
        assert!(lo.bass_amt > 0.0 && lo.comp_ratio == 1.0, "the cheap trick first");
        assert!((mid.bass_amt - hi.bass_amt).abs() < 1e-6, "harmonics stop climbing past halfway");
        assert!(hi.comp_ratio > mid.comp_ratio, "and the costly one takes over");
        assert!(hi.comp_threshold_db < mid.comp_threshold_db);
        // Monotonic overall, so the slider never reverses under a finger.
        let mut prev = -1.0;
        for i in 0..=20 {
            let w = warmth(i as f32 / 20.0);
            let loudness = w.bass_amt + (w.comp_ratio - 1.0);
            assert!(loudness >= prev, "went backwards at {i}");
            prev = loudness;
        }
    }

    #[test]
    fn the_limiter_is_never_off() {
        // bankstown adds energy and people then raise the volume; without a
        // limiter that arrives at the coil as clipping.
        for i in 0..=10 {
            let w = warmth(i as f32 / 10.0);
            assert_eq!(w.limit_db, LIMIT_DB);
            assert!(w.limit_db < 0.0);
        }
        assert!(warmth(2.0).bass_amt <= 0.75, "clamped");
        assert_eq!(warmth(-1.0).bass_amt, 0.0);
    }
}
