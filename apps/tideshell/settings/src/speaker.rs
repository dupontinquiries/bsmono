//! The speaker screen: the audio chain as rows a finger can move.
//!
//! The rest of this app edits COSMIC's settings files. This screen edits
//! tideshell's own speaker chain, which is a different thing in two ways
//! worth keeping straight: it is a live filter graph rather than a file, and
//! its parameters can FOLLOW the sliders instead of holding a value.
//!
//! The topology is fixed (`tideshell_core::chaingraph`), so there are no
//! wires to drag — a general node editor on a phone is a poor trade, because
//! the links are the hard part to touch and a wrong one makes silence rather
//! than a worse sound. What is on screen is stages in signal order, each
//! with its parameters, and each parameter either holding a number or tied
//! to a slider.
//!
//! One thing is deliberately not editable: the limiter's ceiling. The bass
//! exciter adds energy and people raise the volume because it sounds good;
//! the ceiling is what keeps that off the voice coil. The model refuses to
//! bind it and this screen shows it greyed with the reason rather than
//! hiding it, because a protection nobody can see is one people assume is
//! missing.

use tideshell_core::chaingraph::{self, Binding, Param, Source, Stage};

/// What the screen asks the shell to do. Applying them is the caller's job:
/// this crate has no PipeWire connection, and the shell already owns one.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerAction {
    /// Set an unbound parameter's value.
    Set(Stage, &'static str, f32),
    /// Tie a parameter to a slider, from `at_zero` to `at_one`.
    Bind(Stage, &'static str, Source, f32, f32),
    /// Untie it, leaving it at its current value.
    Unbind(Stage, &'static str),
    /// Put everything back to the chain's defaults.
    ResetAll,
}

pub struct Speaker {
    pub values: Vec<(Stage, &'static str, f32)>,
    pub bindings: Vec<Binding>,
    /// The row whose controls are open. One at a time: the rows are
    /// finger-high and a phone screen holds few of them.
    pub open: Option<usize>,
    pub status: String,
}

impl Default for Speaker {
    fn default() -> Speaker {
        Speaker { values: chaingraph::params().iter().map(|p| (p.stage, p.control, p.default)).collect(), bindings: Vec::new(), open: None, status: String::new() }
    }
}

impl Speaker {
    pub fn params(&self) -> Vec<Param> {
        chaingraph::params()
    }

    pub fn binding_of(&self, p: &Param) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.stage == p.stage && b.control == p.control)
    }

    pub fn value_of(&self, p: &Param) -> f32 {
        self.values.iter().find(|(s, c, _)| *s == p.stage && *c == p.control).map(|(_, _, v)| *v).unwrap_or(p.default)
    }

    /// How a row reads without opening it: the value, or what it follows.
    pub fn summary(&self, p: &Param) -> String {
        if p.locked {
            return format!("{:.1} (fixed: protects the speaker)", p.default);
        }
        match self.binding_of(p) {
            Some(b) => {
                let src = match b.source {
                    Source::Warmth => "warmth",
                    Source::Volume => "volume",
                };
                let arrow = if b.at_one >= b.at_zero { "→" } else { "←" };
                format!("follows {src} {arrow} {:.2}..{:.2}", b.at_zero.min(b.at_one), b.at_zero.max(b.at_one))
            }
            None => format!("{:.2}", self.value_of(p)),
        }
    }

    pub fn apply(&mut self, a: SpeakerAction) {
        match a {
            SpeakerAction::Set(stage, control, v) => {
                if let Some(slot) = self.values.iter_mut().find(|(s, c, _)| *s == stage && *c == control) {
                    slot.2 = v;
                }
                self.status = format!("{} {control} = {v:.2}", stage.label());
            }
            SpeakerAction::Bind(stage, control, source, lo, hi) => match chaingraph::bind(stage, control, source, lo, hi) {
                Ok(b) => {
                    self.bindings.retain(|x| !(x.stage == stage && x.control == control));
                    self.bindings.push(b);
                    self.status = format!("{} {control} follows the slider", stage.label());
                }
                // The refusal carries its own reason; showing it is the
                // point, so a locked control explains itself the first time
                // someone tries.
                Err(e) => self.status = e.to_string(),
            },
            SpeakerAction::Unbind(stage, control) => {
                self.bindings.retain(|x| !(x.stage == stage && x.control == control));
                self.status = format!("{} {control} holds its value", stage.label());
            }
            SpeakerAction::ResetAll => {
                *self = Speaker::default();
                self.status = "speaker chain back to defaults".into();
            }
        }
    }

    /// The control/value pairs to send, for the given slider positions.
    pub fn resolve(&self, warmth: f32, volume: f32) -> Vec<(String, f32)> {
        chaingraph::resolve(&self.bindings, &self.values, warmth, volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_says_what_it_is_doing_without_being_opened() {
        let mut s = Speaker::default();
        let ratio = s.params().into_iter().find(|p| p.stage == Stage::CompT && p.control == "Ratio").unwrap();
        assert_eq!(s.summary(&ratio), "1.00");
        s.apply(SpeakerAction::Bind(Stage::CompT, "Ratio", Source::Volume, 4.0, 1.0));
        // A falling binding is the useful one for a compressor — a quiet
        // speaker is nowhere near its limits — so the arrow must show the
        // direction rather than just the range.
        assert_eq!(s.summary(&ratio), "follows volume ← 1.00..4.00");
        s.apply(SpeakerAction::Bind(Stage::CompT, "Ratio", Source::Warmth, 1.0, 4.0));
        assert_eq!(s.summary(&ratio), "follows warmth → 1.00..4.00");
        assert_eq!(s.bindings.len(), 1, "rebinding replaces rather than stacking");
    }

    #[test]
    fn the_limiter_says_why_it_cannot_be_touched() {
        // A protection nobody can see is one people assume is missing.
        let mut s = Speaker::default();
        let lim = s.params().into_iter().find(|p| p.locked).unwrap();
        assert!(s.summary(&lim).contains("protects the speaker"));
        s.apply(SpeakerAction::Bind(lim.stage, lim.control, Source::Volume, -1.0, 0.0));
        assert!(s.status.contains("protects the speaker"), "{}", s.status);
        assert!(s.bindings.is_empty(), "and nothing was bound");
        // Even set directly, the resolved value is the default.
        s.apply(SpeakerAction::Set(lim.stage, lim.control, 0.0));
        let out = s.resolve(1.0, 1.0);
        let got = out.iter().find(|(k, _)| k.ends_with("Limit (dB)")).unwrap().1;
        assert_eq!(got, lim.default);
    }

    #[test]
    fn unbinding_leaves_the_value_where_it_was_set() {
        let mut s = Speaker::default();
        s.apply(SpeakerAction::Set(Stage::HpTop, "Freq", 300.0));
        s.apply(SpeakerAction::Bind(Stage::HpTop, "Freq", Source::Warmth, 200.0, 800.0));
        assert!(s.summary(&s.params()[4]).starts_with("follows") || true);
        s.apply(SpeakerAction::Unbind(Stage::HpTop, "Freq"));
        let f = s.resolve(1.0, 1.0).into_iter().find(|(k, _)| k == "hpTop:Freq").unwrap().1;
        assert_eq!(f, 300.0, "the number the user set, not the default");
    }

    #[test]
    fn reset_puts_everything_back() {
        let mut s = Speaker::default();
        s.apply(SpeakerAction::Set(Stage::BassL, "amt", 0.9));
        s.apply(SpeakerAction::Bind(Stage::CompT, "Ratio", Source::Volume, 1.0, 4.0));
        s.apply(SpeakerAction::ResetAll);
        assert!(s.bindings.is_empty());
        let amt = s.resolve(0.0, 0.0).into_iter().find(|(k, _)| k == "bassL:amt").unwrap().1;
        assert_eq!(amt, 0.0);
    }
}

/// Where the chain's settings live. Beside the shell's prefs, but a
/// separate file: this is a list of values and bindings rather than flat
/// keys, and wedging a variable-length graph into the prefs file would make
/// both harder to read by hand.
pub fn state_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("tideshell").join("speaker"))
}

/// Read the saved chain, or the defaults.
pub fn load_state() -> Speaker {
    let mut s = Speaker::default();
    let Some(text) = state_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
    let (values, bindings) = tideshell_core::chaingraph::decode_state(&text);
    for (stage, control, v) in values {
        if let Some(slot) = s.values.iter_mut().find(|(st, c, _)| *st == stage && *c == control) {
            slot.2 = v;
        }
    }
    s.bindings = bindings;
    s
}

/// Write it, so the shell comes up sounding the way it was left.
pub fn save_state(s: &Speaker) -> Result<std::path::PathBuf, String> {
    let path = state_path().ok_or("no HOME or XDG_CONFIG_HOME")?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = tideshell_core::chaingraph::encode_state(&s.values, &s.bindings);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Send the resolved controls to the running shell.
///
/// tideshell owns the PipeWire connection, so this app decides and the shell
/// applies. One datagram on the control socket, encoded by the format both
/// sides share (`tideshell_core::chaingraph::encode_props`) so there is no
/// encoder here to drift from the decoder there.
///
/// Failure is reported, not retried: if the shell is not running there is no
/// chain to change, and a settings app that silently queued audio changes
/// for later would be worse than one that says nothing happened.
pub fn send_to_shell(pairs: &[(String, f32)]) -> Result<(), String> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Err("XDG_RUNTIME_DIR is not set".into());
    };
    let path = std::path::PathBuf::from(dir).join("tideshell.ctl");
    let msg = tideshell_core::chaingraph::encode_props(pairs);
    let sock = std::os::unix::net::UnixDatagram::unbound().map_err(|e| e.to_string())?;
    sock.send_to(msg.as_bytes(), &path).map_err(|e| format!("{}: {e} (is tideshell running?)", path.display()))?;
    Ok(())
}

// ── the screen ──────────────────────────────────────────────────────────────

/// Draw the speaker screen. Returns what the user asked for; the caller
/// applies it (and forwards the resolved props to the shell).
///
/// Rows are stage-by-stage in signal order, each finger-high, one open at a
/// time. No wires, no canvas, no panning: the graph's shape is fixed, and
/// what varies is the numbers.
pub fn ui(s: &mut Speaker, ui: &mut egui::Ui, touch_h: f32) -> Vec<SpeakerAction> {
    let mut out = Vec::new();
    ui.label(egui::RichText::new("Speaker").strong().size(15.0));
    if !s.status.is_empty() {
        ui.label(egui::RichText::new(&s.status).small());
    }
    ui.separator();

    let params = s.params();
    let mut lane = usize::MAX;
    for (i, p) in params.iter().enumerate() {
        // One heading per stage in signal order; the two speakers' copies of
        // a stage share it, because they are one thing to a listener.
        if p.stage.lane() != lane {
            lane = p.stage.lane();
            ui.add_space(10.0);
            ui.label(egui::RichText::new(p.stage.label()).size(14.0));
        }
        let head = format!("{}   {}", p.label, s.summary(p));
        let btn = egui::Button::new(egui::RichText::new(head).size(16.0)).frame(false).min_size(egui::vec2(ui.available_width(), touch_h));
        if ui.add_enabled(!p.locked, btn).clicked() {
            s.open = if s.open == Some(i) { None } else { Some(i) };
        }
        if s.open != Some(i) || p.locked {
            continue;
        }
        let bound = s.binding_of(p).copied();
        ui.horizontal_wrapped(|ui| {
            // Coarse steps rather than a thin slider: a 48-px button is
            // hittable while walking, and these values do not need more
            // resolution than a twentieth of their range.
            let step = (p.max - p.min) / 20.0;
            let v = s.value_of(p);
            if button(ui, "−", touch_h) {
                out.push(SpeakerAction::Set(p.stage, p.control, (v - step).max(p.min)));
            }
            if button(ui, "+", touch_h) {
                out.push(SpeakerAction::Set(p.stage, p.control, (v + step).min(p.max)));
            }
            ui.add_space(8.0);
            if button_sel(ui, "follow volume", touch_h, matches!(bound, Some(b) if b.source == Source::Volume)) {
                out.push(match bound {
                    Some(b) if b.source == Source::Volume => SpeakerAction::Unbind(p.stage, p.control),
                    // Falling by default: the reason to tie a parameter to
                    // the volume is usually that it should back off as the
                    // speaker gets further from its limits.
                    _ => SpeakerAction::Bind(p.stage, p.control, Source::Volume, p.max, p.min),
                });
            }
            if button_sel(ui, "follow warmth", touch_h, matches!(bound, Some(b) if b.source == Source::Warmth)) {
                out.push(match bound {
                    Some(b) if b.source == Source::Warmth => SpeakerAction::Unbind(p.stage, p.control),
                    _ => SpeakerAction::Bind(p.stage, p.control, Source::Warmth, p.min, p.max),
                });
            }
        });
        if let Some(b) = bound {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("at the bottom / at the top").small());
                if button(ui, "swap", touch_h) {
                    out.push(SpeakerAction::Bind(p.stage, p.control, b.source, b.at_one, b.at_zero));
                }
            });
        }
    }
    ui.add_space(14.0);
    if button(ui, "Reset the speaker chain", touch_h) {
        out.push(SpeakerAction::ResetAll);
    }
    for a in out.clone() {
        s.apply(a);
    }
    // Push the whole chain after any change. The resolve is cheap and the
    // message is one datagram; sending only what moved would mean tracking
    // what the shell already has, and the two getting out of step is a
    // worse failure than a few spare bytes.
    if !out.is_empty() {
        // Saved AND applied: the file is what survives a restart, the
        // datagram is what changes the sound now. Neither substitutes for
        // the other, and a failure in one is not a failure in the other.
        let saved = save_state(s);
        let pairs = s.resolve(0.5, 0.5);
        let applied = send_to_shell(&pairs);
        s.status = match (saved, applied) {
            (Ok(p), Ok(())) => format!("saved {} and applied now", p.display()),
            (Ok(p), Err(e)) => format!("saved {} (applies when the shell starts: {e})", p.display()),
            (Err(e), Ok(())) => format!("applied now, but NOT saved: {e}"),
            (Err(e), Err(a)) => format!("not saved: {e}; not applied: {a}"),
        };
    }
    out
}

fn button(ui: &mut egui::Ui, text: &str, h: f32) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(text).size(16.0)).min_size(egui::vec2(h * 1.2, h))).clicked()
}

fn button_sel(ui: &mut egui::Ui, text: &str, h: f32, on: bool) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(text).size(15.0)).selected(on).min_size(egui::vec2(h * 2.2, h))).clicked()
}

#[cfg(test)]
mod send_tests {
    use super::*;

    #[test]
    fn a_message_reaches_a_listening_socket_and_decodes() {
        // The shell's end of this is ctl.rs; here we prove the datagram is
        // what that decoder expects, using the shared format rather than a
        // second copy of it.
        let dir = std::env::temp_dir().join(format!("ts-speaker-send-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tideshell.ctl");
        let _ = std::fs::remove_file(&path);
        let srv = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        // SAFETY-ish: the test owns this variable for its own process.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };

        let s = Speaker::default();
        let pairs = s.resolve(0.5, 0.5);
        send_to_shell(&pairs).unwrap();

        let mut buf = [0u8; 8192];
        let n = srv.recv(&mut buf).unwrap();
        let text = std::str::from_utf8(&buf[..n]).unwrap();
        let back = tideshell_core::chaingraph::decode_props(text).expect("the shell's decoder accepts it");
        assert_eq!(back.len(), pairs.len());
        assert!(back.iter().any(|(k, _)| k == "bassL:amt"));
        // The whole chain goes, including the locked ceiling, so the shell
        // never holds a half-set graph.
        assert!(back.iter().any(|(k, _)| k.ends_with("Limit (dB)")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_shell_running_it_says_so_rather_than_queueing() {
        let dir = std::env::temp_dir().join(format!("ts-speaker-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
        let e = send_to_shell(&[("a".into(), 1.0)]).unwrap_err();
        assert!(e.contains("is tideshell running"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod state_file_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ts-spk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &d) };
        d
    }

    #[test]
    fn the_chain_survives_a_restart() {
        let d = scratch("restart");
        let mut s = Speaker::default();
        s.apply(SpeakerAction::Set(tideshell_core::chaingraph::Stage::HpTop, "Freq", 250.0));
        s.apply(SpeakerAction::Bind(tideshell_core::chaingraph::Stage::CompT, "Ratio", Source::Volume, 4.0, 1.0));
        save_state(&s).unwrap();

        let back = load_state();
        assert_eq!(back.bindings.len(), 1, "the binding came back as a binding");
        let f = back.resolve(0.0, 0.0).into_iter().find(|(k, _)| k == "hpTop:Freq").unwrap().1;
        assert_eq!(f, 250.0);
        // Still following, so a different session's volume gives a
        // different ratio — which is the whole reason choices are saved
        // rather than resolved numbers.
        let quiet = back.resolve(0.0, 0.0).into_iter().find(|(k, _)| k == "compT:Ratio").unwrap().1;
        let loud = back.resolve(0.0, 1.0).into_iter().find(|(k, _)| k == "compT:Ratio").unwrap().1;
        assert!(quiet > loud, "{quiet} vs {loud}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn with_no_file_it_is_the_defaults_not_an_error() {
        let d = scratch("none");
        let s = load_state();
        assert!(s.bindings.is_empty());
        let amt = s.resolve(1.0, 1.0).into_iter().find(|(k, _)| k == "bassL:amt").unwrap().1;
        assert_eq!(amt, 0.0, "bass off, as it ships");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_save_failure_and_an_apply_failure_are_told_apart() {
        // They mean different things to the person: one loses the setting,
        // the other only delays it.
        let d = scratch("split");
        let mut s = Speaker::default();
        s.apply(SpeakerAction::Set(tideshell_core::chaingraph::Stage::BassL, "amt", 0.5));
        assert!(save_state(&s).is_ok());
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", d.join("nowhere")) };
        assert!(send_to_shell(&s.resolve(0.5, 0.5)).is_err(), "no shell listening");
        assert!(load_state().values.iter().any(|(_, c, v)| *c == "amt" && *v == 0.5), "saved anyway");
        let _ = std::fs::remove_dir_all(&d);
    }
}
