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

/// The speaker sink: bankstown, a compressor, and a limiter that is never
/// off. Named so [`warmth_props_text`] can move the controls at runtime.
pub const SPEAKER_SINK: &str = "tideshell-speaker";

/// The PipeWire user config for the speaker chain.
///
/// An EXPLICIT two-in two-out graph, not a mono graph left to be duplicated.
/// A filter-chain whose graph has one input and one output is replicated per
/// channel, which is what the simple configs rely on — but the two speakers
/// here are different drivers that need different treatment (only one can
/// make bass), and the channels have to cross when the screen turns. Neither
/// is possible once the graph is per-channel, so both channels are wired by
/// hand and `inputs`/`outputs` name the ends.
///
/// The shape:
///
/// ```text
///  FL -> bassL -\            /-> hpTop -> compT -> limT -> top (earpiece)
///                 mixTop/mixBot
///  FR -> bassR -/            \-> ......... compB -> limB -> bottom (main)
/// ```
///
/// `mixTop`/`mixBot` are the 2x2 matrix that [`speaker-pose`] drives: mono
/// into both when upright, stereo when turned, crossfaded between. `hpTop`
/// keeps the low end out of the earpiece, which cannot make it and only
/// rattles trying — the bottom driver gets it instead.
///
/// None of the plugin stages is builtin. PipeWire's own filters are mixers,
/// copies, biquads and convolvers, so this loads only where these exist:
///
/// * bankstown, LV2 `https://chadmed.au/bankstown`, installed as a proper
///   bundle — `bankstown.so` (renamed from `libbankstown.so`),
///   `bankstown.ttl` AND `manifest.ttl`, or lilv will not find it.
/// * swh-plugins for the LADSPA compressor (`sc4`) and limiter
///   (`fastLookaheadLimiter`).
///
/// The limiter is last on both legs and is not on any slider. bankstown ADDS
/// energy, and a bass exciter's whole effect is that people then turn the
/// volume up; without a limiter that arrives at the voice coil as clipping.
///
/// Port names follow each plugin's own convention and want checking against
/// `lv2ls`/`listplugins` output on the device before anyone trusts this
/// config to come up first time.
pub fn speaker_chain_config() -> String {
    let bank = |n: &str| format!(r#"{{ type = lv2 plugin = "https://chadmed.au/bankstown" label = bankstown name = {n}
                       control = {{ "bypass" = 1.0 "amt" = 0.0 "blend" = 0.5 "floor" = 45.0 "ceil" = 180.0 "final_hp" = 40.0 "sat_second" = 0.3 "sat_third" = 0.6 }} }}"#);
    let comp = |n: &str| format!(r#"{{ type = ladspa plugin = "sc4_1882" label = sc4 name = {n}
                       control = {{ "Ratio" = 1.0 "Threshold level (dB)" = 0.0 "Attack time (ms)" = 10.0 "Release time (ms)" = 200.0 "Makeup gain (dB)" = 0.0 }} }}"#);
    let limit = |n: &str| format!(r#"{{ type = ladspa plugin = "fast_lookahead_limiter_1913" label = fastLookaheadLimiter name = {n}
                       control = {{ "Input gain (dB)" = 0.0 "Limit (dB)" = {LIMIT_DB:.1} "Release time (s)" = 0.1 }} }}"#);
    format!(
        r#"# tideshell speaker sink.
#
#   FL -> bassL -\            /-> hpTop -> compT -> limT -> top (earpiece)
#                 mixTop/mixBot
#   FR -> bassR -/            \-------------> compB -> limB -> bottom (main)
#
# Needs an LV2 bankstown bundle and swh-plugins:
#   lv2ls | grep bankstown
#   listplugins | grep -iE 'limiter|sc4'
#
# The limiter is deliberately not adjustable from the shell: it keeps what
# the exciter adds off the voice coil.
context.modules = [
    {{  name = libpipewire-module-filter-chain
        args = {{
            node.description = "tideshell speaker"
            media.name       = "tideshell speaker"
            filter.graph = {{
                nodes = [
                    {bass_l}
                    {bass_r}
                    {{ type = builtin label = mixer name = mixTop control = {{ "Gain 1" = 0.5 "Gain 2" = 0.5 }} }}
                    {{ type = builtin label = mixer name = mixBot control = {{ "Gain 1" = 0.5 "Gain 2" = 0.5 }} }}
                    {{ type = builtin label = bq_highpass name = hpTop control = {{ "Freq" = {cross:.1} "Q" = 0.7 }} }}
                    {comp_t}
                    {comp_b}
                    {limit_t}
                    {limit_b}
                ]
                links = [
                    {{ output = "bassL:Out" input = "mixTop:In 1" }}
                    {{ output = "bassR:Out" input = "mixTop:In 2" }}
                    {{ output = "bassL:Out" input = "mixBot:In 1" }}
                    {{ output = "bassR:Out" input = "mixBot:In 2" }}
                    {{ output = "mixTop:Out" input = "hpTop:In" }}
                    {{ output = "hpTop:Out" input = "compT:In" }}
                    {{ output = "compT:Out" input = "limT:In" }}
                    {{ output = "mixBot:Out" input = "compB:In" }}
                    {{ output = "compB:Out" input = "limB:In" }}
                ]
                inputs  = [ "bassL:In" "bassR:In" ]
                outputs = [ "limT:Out" "limB:Out" ]
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
        bass_l = bank("bassL"),
        bass_r = bank("bassR"),
        comp_t = comp("compT"),
        comp_b = comp("compB"),
        limit_t = limit("limT"),
        limit_b = limit("limB"),
        cross = 400.0,
        sink = SPEAKER_SINK
    )
}

/// `pw-cli set-param <node> Props` for a warmth position.
pub fn warmth_props_text(w: &Warmth) -> String {
    let bypass = if w.bass_amt <= 0.0 { 1.0 } else { 0.0 };
    let makeup_db = 20.0 * w.makeup.log10();
    let mut items: Vec<(String, f32)> = Vec::new();
    // Both legs move together: warmth is a property of the programme, not of
    // one speaker.
    for ch in ["bassL", "bassR"] {
        items.push((format!("{ch}:bypass"), bypass));
        items.push((format!("{ch}:amt"), w.bass_amt));
        items.push((format!("{ch}:blend"), w.bass_blend));
    }
    for c in ["compT", "compB"] {
        items.push((format!("{c}:Ratio"), w.comp_ratio));
        items.push((format!("{c}:Threshold level (dB)"), w.comp_threshold_db));
        items.push((format!("{c}:Makeup gain (dB)"), makeup_db));
    }
    for l in ["limT", "limB"] {
        items.push((format!("{l}:Limit (dB)"), w.limit_db));
    }
    let body: Vec<String> = items.iter().map(|(k, v)| format!("\"{k}\" {v:.3}")).collect();
    format!("{{ params = [ {} ] }}", body.join(" "))
}

#[cfg(test)]
mod speaker_chain_tests {
    use super::*;

    #[test]
    fn both_legs_end_in_a_limiter() {
        let c = speaker_chain_config();
        assert!(c.contains("chadmed.au/bankstown"));
        for leg in ["compT:Out\" input = \"limT:In", "compB:Out\" input = \"limB:In"] {
            assert!(c.contains(leg), "missing {leg}");
        }
        assert_eq!(c.matches("fastLookaheadLimiter").count(), 2, "one per speaker: the plugins are mono");
        assert!(c.contains(&format!("{LIMIT_DB:.1}")));
        assert!(c.contains("lv2ls") && c.contains("swh-plugins"), "it says what it needs; none of this is builtin");
    }

    #[test]
    fn the_graph_is_explicitly_stereo_and_crossable() {
        // A one-in one-out graph gets duplicated per channel, which would
        // make the matrix and the per-speaker high-pass impossible.
        let c = speaker_chain_config();
        assert!(c.contains(r#"inputs  = [ "bassL:In" "bassR:In" ]"#));
        assert!(c.contains(r#"outputs = [ "limT:Out" "limB:Out" ]"#));
        // Every input reaches every output: that is what lets the channels
        // swap when the screen turns.
        for link in [
            r#"output = "bassL:Out" input = "mixTop:In 1""#,
            r#"output = "bassR:Out" input = "mixTop:In 2""#,
            r#"output = "bassL:Out" input = "mixBot:In 1""#,
            r#"output = "bassR:Out" input = "mixBot:In 2""#,
        ] {
            assert!(c.contains(link), "missing {link}");
        }
        // The earpiece is high-passed and the main driver is not.
        assert!(c.contains(r#"output = "mixTop:Out" input = "hpTop:In""#));
        assert!(c.contains(r#"output = "mixBot:Out" input = "compB:In""#), "no high-pass on the bottom leg");
        assert_eq!(c.matches("bq_highpass").count(), 1);
    }

    #[test]
    fn the_props_move_every_stage_but_the_limit_stays_put() {
        let quiet = warmth_props_text(&warmth(0.0));
        assert!(quiet.contains("\"bassL:bypass\" 1.000") && quiet.contains("\"bassR:bypass\" 1.000"), "zero really bypasses: {quiet}");
        let loud = warmth_props_text(&warmth(1.0));
        assert!(loud.contains("\"bassL:bypass\" 0.000"));
        // Both legs move together: warmth is a property of the programme,
        // not of one speaker.
        assert!(loud.contains("\"compT:Ratio\" 3.500") && loud.contains("\"compB:Ratio\" 3.500"));
        for x in [0.0, 0.5, 1.0] {
            let t = warmth_props_text(&warmth(x));
            for l in ["limT", "limB"] {
                assert!(t.contains(&format!("\"{l}:Limit (dB)\" {LIMIT_DB:.3}")), "the limiter never moves, at {x} on {l}");
            }
        }
        // Make-up is dB for the LADSPA port, not the linear factor the model
        // carries.
        assert!(loud.contains("\"compT:Makeup gain (dB)\" 1.9"));
    }
}
