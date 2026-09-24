//! The speaker chain as something an operator can see and change.
//!
//! The chain is a real filter graph — exciter, matrix, crossover, compressor,
//! limiter — and until now its parameters were decided by code and written
//! as a wall of `pw-cli` props. That is fine until someone wants a different
//! trade, at which point they have to edit Rust.
//!
//! This is the model behind a touch editor for it: what the nodes are, which
//! of their parameters may be moved, and what a parameter may be tied TO.
//! No UI here, and no PipeWire either — [`resolve`] turns the whole thing
//! into the same list of control/value pairs the chain already takes.
//!
//! ## Simplified on purpose
//!
//! A general node graph on a phone is a bad idea: the links are the hard
//! part to touch, the layout needs panning and zooming, and a wrong link
//! makes silence rather than a worse sound. So the TOPOLOGY is fixed — it is
//! the chain tideshell installs — and what the editor exposes is the
//! parameters and their bindings. Everything is a big rectangle in a fixed
//! lane order, which needs no panning and cannot be wired wrong.
//!
//! ## What may not be touched
//!
//! The limiter's threshold is not bindable and not editable. It is the one
//! stage that exists to protect the hardware: the exciter adds energy, people
//! raise the volume because it sounds good, and without a ceiling that
//! arrives at the voice coil as clipping. A graph editor that let someone
//! drag it to 0 dB would be a graph editor that lets someone destroy a
//! speaker, so [`Param::locked`] says no and [`bind`] refuses.

use std::fmt;

/// A stage in the fixed chain, named as the PipeWire graph names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    BassL,
    BassR,
    MixTop,
    MixBot,
    HpTop,
    CompT,
    CompB,
    LimT,
    LimB,
}

impl Stage {
    pub const ALL: [Stage; 9] = [Stage::BassL, Stage::BassR, Stage::MixTop, Stage::MixBot, Stage::HpTop, Stage::CompT, Stage::CompB, Stage::LimT, Stage::LimB];

    pub fn node(self) -> &'static str {
        match self {
            Stage::BassL => "bassL",
            Stage::BassR => "bassR",
            Stage::MixTop => "mixTop",
            Stage::MixBot => "mixBot",
            Stage::HpTop => "hpTop",
            Stage::CompT => "compT",
            Stage::CompB => "compB",
            Stage::LimT => "limT",
            Stage::LimB => "limB",
        }
    }

    /// What to call it on a screen, in words rather than in node names.
    pub fn label(self) -> &'static str {
        match self {
            Stage::BassL | Stage::BassR => "Bass",
            Stage::MixTop | Stage::MixBot => "Routing",
            Stage::HpTop => "Earpiece filter",
            Stage::CompT | Stage::CompB => "Compressor",
            Stage::LimT | Stage::LimB => "Limiter",
        }
    }

    /// Which lane it sits in, left to right. Stages sharing a lane are the
    /// same stage on the two speakers and are drawn as one card.
    pub fn lane(self) -> usize {
        match self {
            Stage::BassL | Stage::BassR => 0,
            Stage::MixTop | Stage::MixBot => 1,
            Stage::HpTop => 2,
            Stage::CompT | Stage::CompB => 3,
            Stage::LimT | Stage::LimB => 4,
        }
    }
}

/// One adjustable value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Param {
    pub stage: Stage,
    /// The control's name inside the node.
    pub control: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Not editable and not bindable, whatever the UI wants.
    pub locked: bool,
}

/// Every parameter the editor knows about.
pub fn params() -> Vec<Param> {
    let mut v = Vec::new();
    for s in [Stage::BassL, Stage::BassR] {
        v.push(Param { stage: s, control: "amt", label: "Amount", min: 0.0, max: 1.0, default: 0.0, locked: false });
        v.push(Param { stage: s, control: "blend", label: "Blend", min: 0.0, max: 1.0, default: 0.5, locked: false });
    }
    v.push(Param { stage: Stage::HpTop, control: "Freq", label: "Crossover", min: 120.0, max: 1200.0, default: 400.0, locked: false });
    for s in [Stage::CompT, Stage::CompB] {
        v.push(Param { stage: s, control: "Ratio", label: "Ratio", min: 1.0, max: 8.0, default: 1.0, locked: false });
        v.push(Param { stage: s, control: "Threshold level (dB)", label: "Threshold", min: -40.0, max: 0.0, default: 0.0, locked: false });
    }
    for s in [Stage::LimT, Stage::LimB] {
        // The one stage that protects the hardware.
        v.push(Param { stage: s, control: "Limit (dB)", label: "Ceiling", min: -6.0, max: -0.5, default: -1.0, locked: true });
    }
    v
}

/// What a parameter can follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The warmth slider.
    Warmth,
    /// The volume slider, so a parameter can change with how loud it is —
    /// the reason this exists at all: compression wants to back off as the
    /// volume drops, because a quiet speaker is not near its limits.
    Volume,
}

/// A parameter following a source, over a range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Binding {
    pub stage: Stage,
    pub control: &'static str,
    pub source: Source,
    /// Output at source 0 and at source 1. Reversed (`at_zero` above
    /// `at_one`) is allowed and is how a parameter falls as a slider rises.
    pub at_zero: f32,
    pub at_one: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    Locked(Stage, &'static str),
    Unknown(Stage, &'static str),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::Locked(s, c) => write!(f, "{} {c} protects the speaker and cannot be bound", s.label()),
            BindError::Unknown(s, c) => write!(f, "{} has no control {c}", s.label()),
        }
    }
}

/// Make a binding, refusing the ones that must not exist.
pub fn bind(stage: Stage, control: &'static str, source: Source, at_zero: f32, at_one: f32) -> Result<Binding, BindError> {
    let p = params().into_iter().find(|p| p.stage == stage && p.control == control).ok_or(BindError::Unknown(stage, control))?;
    if p.locked {
        return Err(BindError::Locked(stage, control));
    }
    Ok(Binding { stage, control, source, at_zero: at_zero.clamp(p.min, p.max), at_one: at_one.clamp(p.min, p.max) })
}

/// The control/value pairs for the current slider positions.
///
/// Unbound parameters keep `value`, which is whatever the editor last set;
/// locked ones always take their default, so no amount of editing can move
/// them.
pub fn resolve(bindings: &[Binding], values: &[(Stage, &'static str, f32)], warmth: f32, volume: f32) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    for p in params() {
        let key = format!("{}:{}", p.stage.node(), p.control);
        if p.locked {
            out.push((key, p.default));
            continue;
        }
        let v = if let Some(b) = bindings.iter().find(|b| b.stage == p.stage && b.control == p.control) {
            let t = match b.source {
                Source::Warmth => warmth,
                Source::Volume => volume,
            }
            .clamp(0.0, 1.0);
            b.at_zero + (b.at_one - b.at_zero) * t
        } else {
            values.iter().find(|(s, c, _)| *s == p.stage && *c == p.control).map(|(_, _, v)| *v).unwrap_or(p.default)
        };
        out.push((key, v.clamp(p.min, p.max)));
    }
    out
}

/// Card geometry for a lane of stages: big rectangles in a fixed order, no
/// panning, no wires to drag.
///
/// `min_px` is the smallest comfortable touch target; the cards fill the
/// width when there is room and scroll horizontally when there is not,
/// because shrinking below the target is how a graph editor becomes
/// unusable on a phone.
pub fn lane_width(total_px: f32, lanes: usize, gap: f32, min_px: f32) -> (f32, bool) {
    if lanes == 0 {
        return (0.0, false);
    }
    let avail = total_px - gap * (lanes as f32 - 1.0);
    let w = avail / lanes as f32;
    if w >= min_px {
        (w, false)
    } else {
        (min_px, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_cannot_be_bound_or_edited() {
        // The exciter adds energy and people raise the volume because it
        // sounds good; the ceiling is what keeps that off the voice coil.
        for s in [Stage::LimT, Stage::LimB] {
            let e = bind(s, "Limit (dB)", Source::Volume, -1.0, 0.0).unwrap_err();
            assert_eq!(e, BindError::Locked(s, "Limit (dB)"));
            assert!(e.to_string().contains("protects the speaker"));
        }
        // And even a value set directly is ignored.
        let out = resolve(&[], &[(Stage::LimT, "Limit (dB)", 0.0)], 1.0, 1.0);
        let got = out.iter().find(|(k, _)| k == "limT:Limit (dB)").unwrap().1;
        assert_eq!(got, -1.0, "the ceiling holds whatever the editor says");
    }

    #[test]
    fn a_parameter_can_follow_a_slider_in_either_direction() {
        // The reason bindings exist: compression should back off as the
        // volume drops, because a quiet speaker is nowhere near its limits.
        let b = bind(Stage::CompT, "Ratio", Source::Volume, 1.0, 4.0).unwrap();
        let at = |vol: f32| {
            resolve(&[b], &[], 0.0, vol).into_iter().find(|(k, _)| k == "compT:Ratio").unwrap().1
        };
        assert_eq!(at(0.0), 1.0);
        assert_eq!(at(1.0), 4.0);
        assert!((at(0.5) - 2.5).abs() < 1e-5);
        // Reversed is legal and is how something falls as a slider rises.
        let down = bind(Stage::CompT, "Ratio", Source::Volume, 4.0, 1.0).unwrap();
        let rev = |vol: f32| resolve(&[down], &[], 0.0, vol).into_iter().find(|(k, _)| k == "compT:Ratio").unwrap().1;
        assert!(rev(0.0) > rev(1.0));
    }

    #[test]
    fn bindings_are_clamped_to_what_the_stage_accepts() {
        // A binding that asks for a ratio of 900 is a mistake, not an
        // instruction.
        let b = bind(Stage::CompT, "Ratio", Source::Warmth, -50.0, 900.0).unwrap();
        assert_eq!(b.at_zero, 1.0);
        assert_eq!(b.at_one, 8.0);
        assert!(bind(Stage::CompT, "Nonsense", Source::Warmth, 0.0, 1.0).is_err());
    }

    #[test]
    fn unbound_parameters_keep_what_the_editor_set() {
        let out = resolve(&[], &[(Stage::HpTop, "Freq", 250.0)], 0.0, 0.0);
        let f = out.iter().find(|(k, _)| k == "hpTop:Freq").unwrap().1;
        assert_eq!(f, 250.0);
        // And an untouched one takes its default rather than zero.
        let amt = out.iter().find(|(k, _)| k == "bassL:amt").unwrap().1;
        assert_eq!(amt, 0.0, "bass off by default");
        let blend = out.iter().find(|(k, _)| k == "bassL:blend").unwrap().1;
        assert_eq!(blend, 0.5, "but blend is not zero");
        // Every parameter is always emitted, so the chain is never half set.
        assert_eq!(out.len(), params().len());
    }

    #[test]
    fn the_cards_stay_touchable_and_say_when_they_must_scroll() {
        let lanes = 5;
        // A wide screen: cards share the width.
        let (w, scrolls) = lane_width(1000.0, lanes, 12.0, 120.0);
        assert!(w > 120.0 && !scrolls);
        // A phone in portrait: they hit the floor and the lane scrolls
        // rather than shrinking into something nobody can hit.
        let (w, scrolls) = lane_width(360.0, lanes, 12.0, 120.0);
        assert_eq!(w, 120.0);
        assert!(scrolls);
        assert_eq!(lane_width(500.0, 0, 8.0, 100.0), (0.0, false));
    }

    #[test]
    fn stages_are_grouped_into_lanes_in_signal_order() {
        // Left to right is the order the signal travels, which is the only
        // arrangement that reads without wires.
        assert!(Stage::BassL.lane() < Stage::MixTop.lane());
        assert!(Stage::MixTop.lane() < Stage::HpTop.lane());
        assert!(Stage::HpTop.lane() < Stage::CompT.lane());
        assert!(Stage::CompT.lane() < Stage::LimT.lane());
        // The same stage on both speakers shares a lane and a label.
        assert_eq!(Stage::CompT.lane(), Stage::CompB.lane());
        assert_eq!(Stage::BassL.label(), Stage::BassR.label());
        // Node names are the PipeWire ones, so props land where they should.
        assert_eq!(Stage::MixTop.node(), "mixTop");
    }
}

// ── the wire format ─────────────────────────────────────────────────────────
//
// The settings app decides what the chain should do; the shell owns the
// PipeWire connection. So the app sends the resolved control/value pairs over
// tideshell's control socket and the shell applies them.
//
// Defined here, in the crate both sides already depend on, so there is one
// definition rather than an encoder and a decoder that drift. Text, because
// the socket carries text and a human debugging this should be able to read
// a datagram.
//
// Control names contain spaces and parentheses ("Threshold level (dB)"), so
// the split is on the LAST '=' of a line rather than on whitespace.

/// The first line of a props message.
pub const PROPS_TAG: &str = "speaker-props";

/// Encode resolved pairs for the socket.
pub fn encode_props(pairs: &[(String, f32)]) -> String {
    let mut s = String::from(PROPS_TAG);
    for (k, v) in pairs {
        // A newline or an '=' in a control name would make the message
        // ambiguous. None exist, and a silent corruption later would be
        // hard to trace, so they are dropped here where it is obvious.
        if k.contains('\n') {
            continue;
        }
        s.push('\n');
        s.push_str(k);
        s.push('=');
        s.push_str(&format!("{v}"));
    }
    s
}

/// Decode one. `None` when it is not a props message at all.
pub fn decode_props(text: &str) -> Option<Vec<(String, f32)>> {
    let mut lines = text.lines();
    if lines.next()?.trim() != PROPS_TAG {
        return None;
    }
    let mut out = Vec::new();
    for l in lines {
        let l = l.trim_end();
        if l.is_empty() {
            continue;
        }
        // Last '=', not first: "Threshold level (dB)=-12" has one, but a
        // future name containing '=' would break a first-match split.
        let Some(at) = l.rfind('=') else { continue };
        let (k, v) = l.split_at(at);
        let Ok(v) = v[1..].parse::<f32>() else { continue };
        if !v.is_finite() {
            continue;
        }
        out.push((k.to_string(), v));
    }
    Some(out)
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn a_message_survives_the_round_trip() {
        let pairs = resolve(&[], &[], 0.5, 0.5);
        let text = encode_props(&pairs);
        let back = decode_props(&text).unwrap();
        assert_eq!(back.len(), pairs.len());
        for ((a, x), (b, y)) in back.iter().zip(&pairs) {
            assert_eq!(a, b);
            assert!((x - y).abs() < 1e-6, "{a}: {x} vs {y}");
        }
    }

    #[test]
    fn control_names_with_spaces_and_brackets_survive() {
        // "Threshold level (dB)" is a real port name; splitting on
        // whitespace or on the first '=' would mangle it.
        let pairs = vec![("compT:Threshold level (dB)".to_string(), -12.5), ("mixTop:Gain 1".to_string(), 0.5)];
        let back = decode_props(&encode_props(&pairs)).unwrap();
        assert_eq!(back, pairs);
    }

    #[test]
    fn anything_that_is_not_a_props_message_is_refused() {
        assert_eq!(decode_props("power-key"), None);
        assert_eq!(decode_props(""), None);
        assert_eq!(decode_props("speaker-propsX\na=1"), None);
        // A malformed line is dropped rather than poisoning the message: a
        // chain half-applied is better than one not applied at all, and the
        // lines are independent.
        let got = decode_props("speaker-props\ngood=1.5\nnovalue\nbad=zzz\nnan=NaN\n").unwrap();
        assert_eq!(got, vec![("good".to_string(), 1.5)]);
    }
}

// ── saved state ─────────────────────────────────────────────────────────────
//
// The props message (above) is what the chain should do RIGHT NOW. This is
// what the operator chose, which is a different thing and has to outlive
// both processes: the shell should come up sounding the way it was left.
//
// Values and bindings, not resolved props, because a binding is the setting.
// Saving the resolved numbers would freeze a parameter at whatever the
// sliders happened to be when it was saved, and quietly turn "follows the
// volume" into "1.8".

/// One line per value, one per binding, in the same flat text as everything
/// else here.
pub fn encode_state(values: &[(Stage, &'static str, f32)], bindings: &[Binding]) -> String {
    let mut s = String::from("# tideshell speaker chain\n");
    for (stage, control, v) in values {
        s.push_str(&format!("value {}:{} = {v}\n", stage.node(), control));
    }
    for b in bindings {
        let src = match b.source {
            Source::Warmth => "warmth",
            Source::Volume => "volume",
        };
        s.push_str(&format!("bind {}:{} = {src} {} {}\n", b.stage.node(), b.control, b.at_zero, b.at_one));
    }
    s
}

fn stage_by_node(node: &str) -> Option<Stage> {
    Stage::ALL.into_iter().find(|s| s.node() == node)
}

/// Read it back. Unknown stages and controls are dropped rather than
/// refused: a file written by a newer version should still give this one
/// everything it understands, and a chain missing one parameter is better
/// than a chain that would not load.
pub fn decode_state(text: &str) -> (Vec<(Stage, &'static str, f32)>, Vec<Binding>) {
    let known = params();
    let lookup = |node: &str, control: &str| -> Option<Param> { stage_by_node(node).and_then(|st| known.iter().find(|p| p.stage == st && p.control == control).copied()) };
    let (mut values, mut bindings) = (Vec::new(), Vec::new());
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((head, rest)) = line.split_once('=') else { continue };
        let Some((kind, target)) = head.trim().split_once(' ') else { continue };
        let Some((node, control)) = target.trim().split_once(':') else { continue };
        let Some(p) = lookup(node, control) else { continue };
        let rest = rest.trim();
        match kind {
            "value" => {
                if let Ok(v) = rest.parse::<f32>() {
                    if v.is_finite() {
                        values.push((p.stage, p.control, v.clamp(p.min, p.max)));
                    }
                }
            }
            "bind" => {
                let mut it = rest.split_whitespace();
                let source = match it.next() {
                    Some("warmth") => Source::Warmth,
                    Some("volume") => Source::Volume,
                    _ => continue,
                };
                let (Some(Ok(lo)), Some(Ok(hi))) = (it.next().map(str::parse::<f32>), it.next().map(str::parse::<f32>)) else { continue };
                // Through bind(), so a saved file cannot reintroduce a
                // binding the model refuses — the limiter's ceiling being
                // the one that matters.
                if let Ok(b) = bind(p.stage, p.control, source, lo, hi) {
                    bindings.push(b);
                }
            }
            _ => {}
        }
    }
    (values, bindings)
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn what_is_saved_is_the_choice_not_the_result() {
        // Saving resolved props would freeze a parameter at whatever the
        // sliders happened to be, turning "follows the volume" into "1.8".
        let values = vec![(Stage::HpTop, "Freq", 250.0f32)];
        let bindings = vec![bind(Stage::CompT, "Ratio", Source::Volume, 4.0, 1.0).unwrap()];
        let text = encode_state(&values, &bindings);
        assert!(text.contains("bind compT:Ratio = volume 4 1"), "{text}");
        let (v, b) = decode_state(&text);
        assert_eq!(v, values);
        assert_eq!(b, bindings);
        // And it still follows, rather than having become a number.
        let out = resolve(&b, &v, 0.0, 0.0);
        let hot = resolve(&b, &v, 0.0, 1.0);
        assert_ne!(out.iter().find(|(k, _)| k == "compT:Ratio"), hot.iter().find(|(k, _)| k == "compT:Ratio"));
    }

    #[test]
    fn a_saved_file_cannot_reintroduce_what_the_model_refuses() {
        // Hand-edited, or written by a version that allowed it.
        let (_, b) = decode_state("bind limT:Limit (dB) = volume -1 0\n");
        assert!(b.is_empty(), "the ceiling stays locked whatever the file says");
        // And a value for it is clamped to its own range rather than taken.
        let (v, _) = decode_state("value limT:Limit (dB) = 0\n");
        assert!(v.iter().all(|(_, _, x)| *x <= -0.5), "{v:?}");
    }

    #[test]
    fn unknown_and_malformed_lines_are_dropped_not_fatal() {
        // A file from a newer version should still give this one everything
        // it understands.
        let text = "# comment\nvalue future:thing = 1\nvalue hpTop:Freq = 300\nnonsense\nbind hpTop:Freq = sideways 1 2\nvalue hpTop:Freq = NaN\n";
        let (v, b) = decode_state(text);
        assert_eq!(v, vec![(Stage::HpTop, "Freq", 300.0)]);
        assert!(b.is_empty());
        assert_eq!(decode_state("").0.len(), 0);
    }

    #[test]
    fn values_are_clamped_on_the_way_in() {
        let (v, _) = decode_state("value compT:Ratio = 900\n");
        assert_eq!(v, vec![(Stage::CompT, "Ratio", 8.0)]);
    }
}
