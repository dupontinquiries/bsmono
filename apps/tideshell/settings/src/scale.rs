//! The Display screen: how big everything is.
//!
//! Three numbers, kept separate because they answer different questions and
//! a single "UI scale" forces a compromise on someone:
//!
//! * **Text size.** Long sight, small type, reading at arm's length.
//! * **Control size.** Unsteady hands, gloves, a phone used while walking.
//! * **Minimum target.** A floor in points for anything interactive, for
//!   someone who knows what their hands need better than a device-class
//!   heuristic does. Zero leaves the heuristic in charge.
//!
//! A person can need bigger text and normal-sized controls, or the reverse.
//! Collapsing them loses that.
//!
//! ## The screen scales itself
//!
//! The rows here are sized by the values being edited, so the effect is
//! visible while choosing rather than after restarting something. That also
//! makes an unusable setting self-correcting: the buttons for turning it
//! back down grow or shrink with everything else and never leave the screen.
//! The clamp in `tideshell_core::prefs::parse_scale` is what keeps that
//! true at the extremes.

use tideshell_core::prefs::ShellPrefs;

#[derive(Debug, Clone, PartialEq)]
pub enum ScaleAction {
    Text(f32),
    Touch(f32),
    MinTarget(f32),
    Reset,
}

/// What the rows offer. Steps rather than a continuous slider: a thin slider
/// is the wrong control on a phone, and these want a handful of recognisable
/// sizes rather than a number nobody can reproduce.
pub const STEPS: [f32; 6] = [0.8, 0.9, 1.0, 1.15, 1.35, 1.6];
/// Target floors in points. 0 is "let the device decide".
pub const TARGETS: [f32; 4] = [0.0, 44.0, 52.0, 64.0];

/// Row height for a given touch scale, which is how the screen scales
/// itself. Floored so the controls never become unhittable, which would
/// strand someone at a setting they cannot undo.
pub fn row_height(p: &ShellPrefs) -> f32 {
    (48.0 * p.touch_scale).max(p.min_target).clamp(36.0, 96.0)
}

/// Text size for this screen's own labels.
pub fn label_size(p: &ShellPrefs) -> f32 {
    (16.0 * p.text_scale).clamp(11.0, 30.0)
}

pub fn describe_target(v: f32) -> String {
    if v <= 0.0 {
        "automatic".into()
    } else {
        format!("{v:.0} pt")
    }
}

pub fn apply(p: &mut ShellPrefs, a: &ScaleAction) {
    match *a {
        ScaleAction::Text(v) => p.text_scale = tideshell_core::prefs::parse_scale(&v.to_string()).unwrap_or(1.0),
        ScaleAction::Touch(v) => p.touch_scale = tideshell_core::prefs::parse_scale(&v.to_string()).unwrap_or(1.0),
        ScaleAction::MinTarget(v) => p.min_target = v.clamp(0.0, 96.0),
        ScaleAction::Reset => {
            let d = ShellPrefs::default();
            p.text_scale = d.text_scale;
            p.touch_scale = d.touch_scale;
            p.min_target = d.min_target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_follow_the_setting_being_edited() {
        // The effect has to be visible while choosing, not after a restart.
        let mut p = ShellPrefs::default();
        let base = row_height(&p);
        apply(&mut p, &ScaleAction::Touch(1.6));
        assert!(row_height(&p) > base);
        apply(&mut p, &ScaleAction::Touch(0.8));
        assert!(row_height(&p) < base);
        let small = label_size(&p);
        apply(&mut p, &ScaleAction::Text(1.6));
        assert!(label_size(&p) > small);
    }

    #[test]
    fn no_setting_can_strand_the_person_who_chose_it() {
        // Whatever is picked, the controls for undoing it stay hittable and
        // readable — including through the prefs file, where someone can
        // write anything.
        let mut p = ShellPrefs::default();
        for v in [-100.0, 0.0, 0.01, 9.0, 1e9] {
            apply(&mut p, &ScaleAction::Text(v));
            apply(&mut p, &ScaleAction::Touch(v));
            apply(&mut p, &ScaleAction::MinTarget(v));
            let h = row_height(&p);
            let t = label_size(&p);
            assert!((36.0..=96.0).contains(&h), "row {h} at {v}");
            assert!((11.0..=30.0).contains(&t), "text {t} at {v}");
        }
    }

    #[test]
    fn a_floor_wins_over_a_small_scale() {
        // Someone who set a 64 pt floor meant it, even if they also shrank
        // everything else.
        let mut p = ShellPrefs::default();
        apply(&mut p, &ScaleAction::Touch(0.8));
        apply(&mut p, &ScaleAction::MinTarget(64.0));
        assert_eq!(row_height(&p), 64.0);
        assert_eq!(describe_target(0.0), "automatic");
        assert_eq!(describe_target(52.0), "52 pt");
    }

    #[test]
    fn reset_returns_all_three() {
        let mut p = ShellPrefs::default();
        apply(&mut p, &ScaleAction::Text(1.6));
        apply(&mut p, &ScaleAction::Touch(1.35));
        apply(&mut p, &ScaleAction::MinTarget(64.0));
        apply(&mut p, &ScaleAction::Reset);
        let d = ShellPrefs::default();
        assert_eq!((p.text_scale, p.touch_scale, p.min_target), (d.text_scale, d.touch_scale, d.min_target));
    }
}

// ── the screen ──────────────────────────────────────────────────────────────

/// Where the shell keeps its prefs, so this app edits the same file the
/// shell reads rather than a copy of its own.
pub fn prefs_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("tideshell").join("prefs"))
}

/// Read the shell's prefs. Unknown keys and bad values are the shell's
/// problem to report; here they are simply left at their defaults.
pub fn load() -> ShellPrefs {
    let mut p = ShellPrefs::default();
    let Some(text) = prefs_path().and_then(|f| std::fs::read_to_string(f).ok()) else { return p };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = line.split_once('=') {
            let _ = p.apply(k.trim(), v.trim());
        }
    }
    p
}

/// Write them back in the shell's own format.
///
/// The whole file is rewritten from the parsed prefs, which means a key this
/// app does not know about survives only because `ShellPrefs` knows about
/// it — the same struct the shell uses, so there is nothing it could lose
/// that the shell would have kept.
pub fn save(p: &ShellPrefs) -> Result<std::path::PathBuf, String> {
    let path = prefs_path().ok_or("no HOME or XDG_CONFIG_HOME")?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut text = String::from("# tideshell prefs: shell.* keys\n");
    for (k, v) in p.to_kv() {
        text.push_str(&format!("{k} = {v}\n"));
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

pub fn ui(p: &mut ShellPrefs, ui: &mut egui::Ui, status: &mut String) -> Vec<ScaleAction> {
    let mut out = Vec::new();
    let h = row_height(p);
    let size = label_size(p);
    ui.label(egui::RichText::new("Display").strong().size(size));
    ui.label(egui::RichText::new("Text and controls scale separately: bigger type does not have to mean bigger buttons.").size(size * 0.8).weak());
    ui.separator();

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Text size").size(size));
    ui.horizontal_wrapped(|ui| {
        for s in STEPS {
            if pick(ui, &format!("{:.0}%", s * 100.0), h, (p.text_scale - s).abs() < 0.01, size) {
                out.push(ScaleAction::Text(s));
            }
        }
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Control size").size(size));
    ui.horizontal_wrapped(|ui| {
        for s in STEPS {
            if pick(ui, &format!("{:.0}%", s * 100.0), h, (p.touch_scale - s).abs() < 0.01, size) {
                out.push(ScaleAction::Touch(s));
            }
        }
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Smallest tappable thing").size(size));
    ui.horizontal_wrapped(|ui| {
        for t in TARGETS {
            if pick(ui, &describe_target(t), h, (p.min_target - t).abs() < 0.5, size) {
                out.push(ScaleAction::MinTarget(t));
            }
        }
    });

    ui.add_space(14.0);
    if pick(ui, "Reset", h, false, size) {
        out.push(ScaleAction::Reset);
    }

    for a in &out {
        apply(p, a);
    }
    if !out.is_empty() {
        *status = match save(p) {
            Ok(path) => format!("saved {} (the shell reads it on its next change)", path.display()),
            Err(e) => format!("not saved: {e}"),
        };
    }
    out
}

fn pick(ui: &mut egui::Ui, text: &str, h: f32, on: bool, size: f32) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(text).size(size)).selected(on).min_size(egui::vec2(h * 1.5, h))).clicked()
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn it_reads_and_writes_the_shell_own_prefs_file() {
        // Not a copy of its own: the shell reads this file, so an edit here
        // has to land in the same place and in the same format.
        let dir = std::env::temp_dir().join(format!("ts-scale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        assert_eq!(prefs_path().unwrap(), dir.join("tideshell").join("prefs"));

        let mut p = load();
        assert_eq!(p.text_scale, 1.0, "no file yet: defaults");
        apply(&mut p, &ScaleAction::Text(1.35));
        apply(&mut p, &ScaleAction::MinTarget(52.0));
        let path = save(&p).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("shell.scale.text = 1.35"), "{text}");
        assert!(text.contains("shell.scale.min_target = 52"), "{text}");
        assert_eq!(load().text_scale, 1.35, "and it reads back");

        // A setting this screen does not touch survives the round trip,
        // because the struct it goes through is the shell's own.
        std::fs::write(&path, format!("{text}shell.clock.fuzzy = 15\n")).unwrap();
        let reloaded = load();
        assert_eq!(reloaded.text_scale, 1.35, "and the scale keys came back too");
        let saved = save(&reloaded).unwrap();
        assert!(std::fs::read_to_string(saved).unwrap().contains("shell.clock.fuzzy = 15"));

        // A value that is not a valid token is dropped rather than kept as
        // text: the file is a set of typed settings, not a bag of strings,
        // and writing back something the shell would reject helps nobody.
        std::fs::write(&path, "shell.clock.fuzzy = quarters\n").unwrap();
        assert!(!std::fs::read_to_string(save(&load()).unwrap()).unwrap().contains("quarters"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
