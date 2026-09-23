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
