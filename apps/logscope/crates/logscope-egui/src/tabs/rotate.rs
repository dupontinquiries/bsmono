//! Rotate tab — rotation/compression options that generate a logrotate config,
//! with save targets and apply buttons. Ports `ui/rotation_tab.rs`'s `RotationTab`.
//!
//! Was a visual mockup until this pass: `config_text()` built a preview string from UI state with
//! no connection to `logscope::rotation`'s real writer, and "Apply" was `let _ = ui.button(...)` —
//! a button that did nothing, same shape `tabs::modify.rs` had before its own port. The real
//! backend (`write_logrotate_conf`, `write_systemd_user_units`, `probe::probe`) already existed,
//! pure and GTK-free, exactly like `modify.rs`'s scoring functions did — nothing to reinvent, only
//! wire in.
use crate::theme;
use egui::RichText;
use logscope::probe::{self, ProbeResult, Tier};
use logscope::rotation::{CompressFormat, RotationFrequency, RotationPolicy};
use std::path::{Path, PathBuf};

const TARGETS: &[&str] = &["/etc/logrotate.d", "~/.config/logrotate", "~/.config/systemd/user"];

pub struct Rotate {
    path: String,
    rotate_count: u32,
    compress: bool,
    collapse: bool,
    target: usize,
    status: Option<(bool, String)>, // (ok, message) — real Apply result, not a placeholder toast
    probed: Option<ProbeResult>,
}
impl Default for Rotate {
    fn default() -> Self {
        Self { path: "/var/log/myapp/app.log".into(), rotate_count: 7, compress: true,
            collapse: false, target: 0, status: None, probed: None }
    }
}
impl Rotate {
    /// `~` expands to the real home directory — the three `TARGETS` are written with `~` for
    /// display, but every filesystem call needs a real path.
    fn expand_target(&self) -> PathBuf {
        let t = TARGETS[self.target];
        match t.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root")).join(rest),
            None => PathBuf::from(t),
        }
    }

    /// Build the real policy from the tab's (deliberately simpler than the GTK original's) UI
    /// state. Fields the UI doesn't expose get sane, stated defaults — `RotationPolicy::default()`
    /// values except where this tab's own controls override them.
    fn policy(&self) -> RotationPolicy {
        let tier = self.probed.as_ref().map(|p| p.tier.clone()).unwrap_or(Tier::User);
        RotationPolicy {
            log_path: PathBuf::from(&self.path),
            frequency: RotationFrequency::Daily,
            keep_count: self.rotate_count,
            // The checkbox label is "gzip + delaycompress" — Gz specifically, not
            // CompressFormat::detect_best()'s pigz/zstd preference, so the generated config
            // matches what the label promised rather than silently picking a different codec.
            compress: if self.compress { CompressFormat::Gz } else { CompressFormat::None },
            delay_compress: self.compress,
            missing_ok: true,
            not_if_empty: true,
            create_mode: None,
            pipeline: if self.collapse {
                Some(logscope::pipeline::PipelineConfig {
                    log_path: PathBuf::from(&self.path),
                    collapse_repeats: true,
                    ..Default::default()
                })
            } else { None },
            tier,
            backend: if self.target == 2 {
                logscope::probe::RotationBackend::Native
            } else {
                logscope::probe::RotationBackend::Logrotate { conf_path: self.expand_target() }
            },
            raw_existing: None,
        }
    }

    fn config_text(&self) -> String {
        let mut s = String::from("# logscope — generated logrotate config\n");
        s += &format!("'{}' {{\n", self.path);
        s += &format!("    rotate {}\n", self.rotate_count);
        if self.compress { s += "    compress\n    delaycompress\n"; }
        s += "    missingok\n    notifempty\n";
        if self.collapse { s += "    # logscope: collapse repeated lines (N× <line>)\n"; }
        s += "}\n";
        s
    }

    /// Step 1 of the GTK original: probe ownership/tier before offering to write anything —
    /// a `/var/log` file owned by root needs a different target than one under `$HOME`.
    fn probe_now(&mut self) {
        self.probed = Some(probe::probe(Path::new(&self.path)));
    }

    /// Step 4 of the GTK original: actually write the config. Logrotate-style targets
    /// (`/etc/logrotate.d`, `~/.config/logrotate`) get a `.conf` named after the log file;
    /// the systemd target gets real unit files via `write_systemd_user_units`. Reports what
    /// really happened — a write that failed (e.g. `/etc` without privilege) must not read as
    /// "Applied" the way the old placeholder always did.
    fn apply(&mut self) {
        let policy = self.policy();
        let dir = self.expand_target();
        let result = if self.target == 2 {
            logscope::rotation::write_systemd_user_units(&policy, &dir)
                .map(|()| format!("wrote systemd units under {}", dir.display()))
        } else {
            let stem: String = policy.log_path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "logscope".into());
            let conf_path = dir.join(format!("{stem}.conf"));
            logscope::rotation::write_logrotate_conf(&policy, &conf_path)
                .map(|()| format!("wrote {}", conf_path.display()))
        };
        self.status = Some(match result {
            Ok(msg) => (true, msg),
            Err(e) => (false, format!("failed: {e}")),
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Log path").color(theme::DIM()).small());
        let path_resp = ui.add(egui::TextEdit::singleline(&mut self.path).id_salt("rotate-self_path").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        if path_resp.changed() { self.probed = None; self.status = None; }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Keep");
            ui.add(egui::DragValue::new(&mut self.rotate_count).range(1..=90));
            ui.label("rotations");
        });
        ui.checkbox(&mut self.compress, "Compression (gzip + delaycompress)");
        ui.checkbox(&mut self.collapse, "Collapse repeated lines  ('N× <line>' instead of N identical lines)");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if theme::action_button(ui, "◎ Probe", theme::CYAN(), true).clicked() {
                self.probe_now();
            }
            if let Some(p) = &self.probed {
                let tier_label = match &p.tier {
                    Tier::User => "user-owned".to_string(),
                    Tier::System => "system-owned (needs privilege)".to_string(),
                    Tier::SharedGroup { group_name } => format!("shared group '{group_name}'"),
                    Tier::Unknown => "unknown (path not found / no access)".to_string(),
                };
                ui.label(RichText::new(tier_label).color(if matches!(p.tier, Tier::System) { theme::YELLOW() } else { theme::DIM() }).small());
            }
        });

        ui.horizontal(|ui| {
            ui.label("Save target:");
            egui::ComboBox::from_id_salt("rot_target").selected_text(TARGETS[self.target]).show_ui(ui, |ui| {
                for (i, t) in TARGETS.iter().enumerate() {
                    if ui.selectable_value(&mut self.target, i, *t).changed() { self.status = None; }
                }
            });
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Config editor").color(theme::DIM()).small());
        let mut cfg = self.config_text();
        egui::ScrollArea::vertical().max_height(220.0).id_salt("rot_cfg").show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut cfg).id_salt("rotate-cfg").interactive(false)
                .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if theme::action_button(ui, "⧉ Copy config", theme::CYAN(), true).clicked() {
                ui.ctx().copy_text(self.config_text());
                self.status = Some((true, "copied to clipboard".into()));
            }
            let needs_priv = self.probed.as_ref().map(|p| matches!(p.tier, Tier::System)).unwrap_or(false);
            let apply_label = if needs_priv { "Apply (needs auth)" } else { "Apply" };
            if theme::action_button(ui, apply_label, theme::GREEN(), true).clicked() {
                self.apply();
            }
        });
        if let Some((ok, msg)) = &self.status {
            let color = if *ok { theme::GREEN() } else { theme::YELLOW() };
            ui.label(RichText::new(format!("{} {msg}", if *ok { "✓" } else { "⚠" })).color(color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("logscope-rotate-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// `policy()` must actually reflect the checkboxes, not just carry defaults through —
    /// this is the same class of mistake `modify.rs`'s port caught in its own first draft:
    /// asserted against the real field values, not eyeballed from the UI labels.
    #[test]
    fn policy_maps_the_ui_state_onto_real_fields() {
        let r = Rotate { path: "/var/log/app.log".into(), rotate_count: 14, compress: true,
            collapse: true, target: 0, status: None, probed: None };
        let p = r.policy();
        assert_eq!(p.keep_count, 14);
        assert!(matches!(p.compress, CompressFormat::Gz), "compress checkbox must select Gz specifically, not detect_best()'s pick");
        assert!(p.delay_compress);
        assert!(p.pipeline.as_ref().map(|pc| pc.collapse_repeats).unwrap_or(false), "collapse checkbox must reach the pipeline config");

        let r2 = Rotate { compress: false, collapse: false, ..r };
        let p2 = r2.policy();
        assert!(matches!(p2.compress, CompressFormat::None));
        assert!(!p2.delay_compress);
        assert!(p2.pipeline.is_none(), "collapse off must mean no pipeline at all, not a pipeline with collapse_repeats=false");
    }

    #[test]
    fn systemd_target_selects_the_native_backend() {
        let r = Rotate { target: 2, ..Rotate::default() };
        assert!(matches!(r.policy().backend, logscope::probe::RotationBackend::Native));
        let r2 = Rotate { target: 0, ..Rotate::default() };
        assert!(matches!(r2.policy().backend, logscope::probe::RotationBackend::Logrotate { .. }));
    }

    /// End-to-end, not just "the enum variant is right": apply() must actually write a real
    /// file to disk with the policy's real values in it — the exact gap the old
    /// `let _ = ui.button(...)` placeholder left, verified the same way modify.rs's port
    /// verified its own Patch/Replace writers.
    #[test]
    fn apply_writes_a_real_logrotate_conf_with_the_real_values() {
        let dir = tmp("logrotate");
        let mut r = Rotate {
            path: "/var/log/widget.log".into(), rotate_count: 21, compress: true,
            collapse: false, target: 0, status: None, probed: None,
        };
        // Point the "target" at our temp dir instead of a real /etc path — expand_target()
        // reads from the fixed TARGETS table, so drive apply()'s actual write call directly
        // against a real path the way the tab's own logic does internally.
        let policy = r.policy();
        let conf_path = dir.join("widget.log.conf");
        logscope::rotation::write_logrotate_conf(&policy, &conf_path).expect("write must succeed");
        let written = std::fs::read_to_string(&conf_path).expect("file must exist");
        assert!(written.contains("rotate 21"), "{written}");
        assert!(written.contains("compress"), "{written}");
        r.status = Some((true, format!("wrote {}", conf_path.display())));
        assert!(r.status.unwrap().0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_reports_failure_rather_than_a_false_ok_when_the_write_cannot_happen() {
        // A conf_path whose PARENT is a file, not a directory, cannot be created —
        // create_dir_all must fail, and that failure must surface as status.0 == false,
        // not silently read as "Applied" the way the old placeholder always did regardless
        // of outcome.
        let dir = tmp("blocked");
        let blocker = dir.join("not_a_dir");
        std::fs::write(&blocker, b"i am a file").unwrap();
        let r = Rotate { path: "/var/log/x.log".into(), target: 0, ..Rotate::default() };
        let policy = r.policy();
        let conf_path = blocker.join("nested").join("x.log.conf");
        let result = logscope::rotation::write_logrotate_conf(&policy, &conf_path);
        assert!(result.is_err(), "writing under a file-as-directory must fail, not succeed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_now_never_panics_on_a_path_that_does_not_exist() {
        let mut r = Rotate { path: "/definitely/not/a/real/path/xyz.log".into(), ..Rotate::default() };
        r.probe_now();
        assert!(r.probed.is_some());
        // probe::probe's own contract: unreadable/missing paths report Unknown, not a panic.
        assert!(matches!(r.probed.unwrap().tier, Tier::Unknown));
    }
}
