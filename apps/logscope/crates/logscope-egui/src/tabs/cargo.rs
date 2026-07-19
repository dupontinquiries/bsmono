//! Cargo tab — point it at ANY Cargo root, run a real `cargo check --message-format=json`, and work
//! the diagnostics: expand rendered source, copy, or send one to Chat for the model to explain. The
//! tab is also the on-ramp for working on that repo: one button makes the root a `.gitmind` project
//! (marker config + `git init` if needed), one button prefills `:load-gitmind <root>` in Chat so an
//! agent session opens over it, and a peek list lets you read any file — and hand it to Chat as
//! context — without leaving the app. (This replaces the old hard-coded demo diagnostics.)
//!
//! Discipline: the check runs on a worker thread streaming parsed `Diag`s over a channel (the UI never
//! blocks on cargo); the JSON→Diag mapping is a pure fn with a unit test.
use crate::model::{model_combo, ModelState};
use crate::theme;
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub struct Diag {
    file: String,
    line: u32,
    level: String,
    message: String,
    code: Option<String>,
    source: Vec<String>,
}

enum CheckMsg {
    Diag(Diag),
    Done { ok: bool, errors: usize, warnings: usize },
}

pub struct Cargo {
    root: String,            // editable path field
    root_ok: Option<PathBuf>, // validated root (contains Cargo.toml)
    gitmind: bool,           // root is already a .gitmind project
    status: String,
    diags: Vec<Diag>,
    expanded: Option<usize>,
    rx: Option<Receiver<CheckMsg>>, // Some while a check streams in
    peek_files: Vec<String>,        // rel paths under the root (bounded walk)
    peek_show: bool,
    peek_open: Option<(String, String)>, // (rel path, contents ≤ 64 KB)
}

impl Default for Cargo {
    fn default() -> Self {
        Self {
            root: String::new(), root_ok: None, gitmind: false,
            status: "cargo check — pick a project root".into(),
            diags: Vec::new(), expanded: None, rx: None,
            peek_files: Vec::new(), peek_show: false, peek_open: None,
        }
    }
}

impl Cargo {
    /// Validate the typed root: it must contain a `Cargo.toml`. On success, refresh the gitmind flag
    /// and the peek list; the status names the workspace.
    fn apply_root(&mut self) {
        let p = PathBuf::from(self.root.trim());
        if p.join("Cargo.toml").is_file() {
            self.gitmind = crate::gitmind_session::is_gitmind_project(&p);
            self.peek_files = scan_files(&p, 400);
            self.status = format!("root: {} — {} files peekable · Check to compile", p.display(), self.peek_files.len());
            self.root_ok = Some(p);
            self.diags.clear(); self.expanded = None;
        } else {
            self.root_ok = None;
            self.status = format!("no Cargo.toml under {} — point at a crate or workspace root", p.display());
        }
    }

    /// Run `cargo check --message-format=json` on a worker thread, streaming parsed diagnostics back.
    fn run_check(&mut self, root: &Path) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.diags.clear(); self.expanded = None;
        self.status = "cargo check — running…".into();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let out = std::process::Command::new("cargo")
                .args(["check", "--message-format=json", "--color", "never"])
                .current_dir(&root)
                .output();
            match out {
                Err(e) => { let _ = tx.send(CheckMsg::Done { ok: false, errors: 0, warnings: 0 });
                            let _ = e; }
                Ok(out) => {
                    let (mut errors, mut warnings) = (0usize, 0usize);
                    for line in String::from_utf8_lossy(&out.stdout).lines() {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                        if let Some(d) = parse_diag(&v) {
                            if d.level == "error" { errors += 1; } else { warnings += 1; }
                            if tx.send(CheckMsg::Diag(d)).is_err() { return; } // UI dismissed us
                        }
                    }
                    let _ = tx.send(CheckMsg::Done { ok: out.status.success(), errors, warnings });
                }
            }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, scanning: bool) -> Option<String> {
        let mut send: Option<String> = None;

        // Drain the streaming check (if one is running).
        if let Some(rx) = &self.rx {
            let mut done = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    CheckMsg::Diag(d) => self.diags.push(d),
                    CheckMsg::Done { ok, errors, warnings } => {
                        self.status = if ok && errors == 0 {
                            format!("cargo check — clean ({warnings} warning(s))")
                        } else {
                            format!("cargo check — {errors} error(s), {warnings} warning(s)")
                        };
                        done = true;
                    }
                }
            }
            if done { self.rx = None; } else { ui.ctx().request_repaint(); }
        }

        // ── header: root picker + actions ────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label(RichText::new("Cargo root").color(theme::INK()).strong());
            let te = ui.add(egui::TextEdit::singleline(&mut self.root)
                .desired_width(340.0).hint_text("/path/to/your/crate-or-workspace"));
            let entered = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Set root").clicked() || entered { self.apply_root(); }
            let can_check = self.root_ok.is_some() && self.rx.is_none();
            if ui.add_enabled(can_check, egui::Button::new("▶ Check")).clicked() {
                if let Some(r) = self.root_ok.clone() { self.run_check(&r); }
            }
            if self.rx.is_some() && ui.button("Dismiss").on_hover_text("Stop listening to this check (cargo finishes in the background)").clicked() {
                self.rx = None;
                self.status = "cargo check — dismissed".into();
            }
        });
        ui.label(RichText::new(&self.status).color(theme::DIM()).small());

        // ── gitmind + session row: make the root workable, open it in Chat ──────────────────────
        if let Some(root) = self.root_ok.clone() {
            ui.horizontal(|ui| {
                if self.gitmind {
                    theme::chip(ui, ".gitmind ✓", theme::GREEN());
                } else {
                    theme::chip(ui, "no .gitmind", theme::DIM());
                    if ui.button("Init .gitmind here").on_hover_text("Write the [gitmind] marker (+ git init if needed) so sessions can load this root").clicked() {
                        let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
                        match crate::gitmind_session::init_gitmind(&root, &name) {
                            Ok(()) => { self.gitmind = true; self.status = format!("initialized .gitmind '{name}' — ready for sessions"); }
                            Err(e) => self.status = format!("init .gitmind — {e}"),
                        }
                    }
                }
                if ui.add_enabled(self.gitmind, egui::Button::new("⇥ Work on this in Chat"))
                    .on_hover_text("Prefill :load-gitmind for this root — Enter in Chat opens an isolated session over it")
                    .clicked() {
                    send = Some(format!(":load-gitmind {}", root.display()));
                }
                if ui.button(if self.peek_show { "▾ files" } else { "▸ files" }).clicked() { self.peek_show = !self.peek_show; }
            });
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("model for explanations").color(theme::DIM()).small());
            model_combo(ui, model, "cargo_model", scanning);
        });
        ui.separator();

        // ── peek: any file in the repo, readable here, sendable to Chat as context ──────────────
        if self.peek_show {
            if let Some(root) = self.root_ok.clone() {
                egui::ScrollArea::vertical().id_salt("cargo_peek").max_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
                    for rel in &self.peek_files {
                        if ui.small_button(rel).clicked() {
                            let text = std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| format!("(read failed: {e})"));
                            let capped: String = text.chars().take(65_536).collect();
                            self.peek_open = Some((rel.clone(), capped));
                        }
                    }
                });
            }
        }
        if let Some((rel, text)) = &self.peek_open {
            let (rel_c, text_c) = (rel.clone(), text.clone());
            egui::Frame { fill: Color32::from_rgb(0x14, 0x14, 0x14), corner_radius: egui::CornerRadius::same(6),
                inner_margin: egui::Margin::same(8), outer_margin: egui::Margin { bottom: 8, ..Default::default() },
                ..Default::default() }
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&rel_c).color(theme::AMBER_SOFT()).monospace().small());
                    if ui.small_button("→ Chat").on_hover_text("Hand this file to Chat as context").clicked() {
                        send = Some(format!("Here is `{rel_c}` from the project — hold it as context; I'll ask about it next:\n\n```rust\n{text_c}\n```"));
                    }
                    if ui.small_button("✕").clicked() { self.peek_open = None; }
                });
                egui::ScrollArea::vertical().id_salt("cargo_peek_body").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                    for l in text_c.lines().take(600) { ui.label(RichText::new(l).monospace().small().color(theme::DIM())); }
                });
            });
        }

        // ── diagnostics ──────────────────────────────────────────────────────────────────────────
        egui::ScrollArea::vertical().id_salt("cargo_diags").auto_shrink([false, false]).show(ui, |ui| {
            for (i, d) in self.diags.iter().enumerate() {
                let lvl_color = level_color(&d.level);
                egui::Frame { fill: theme::PANEL(), corner_radius: egui::CornerRadius::same(8),
                    inner_margin: egui::Margin::same(10), outer_margin: egui::Margin { bottom: 8, ..Default::default() },
                    ..Default::default() }
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(d.level.to_uppercase()).color(lvl_color).strong().small());
                        if let Some(c) = &d.code { ui.label(RichText::new(format!("[{c}]")).color(theme::DIM()).small()); }
                        ui.label(RichText::new(format!("{}:{}", d.file, d.line)).color(theme::AMBER_SOFT()).monospace().small());
                    });
                    ui.label(RichText::new(&d.message).color(theme::INK()));
                    let open = self.expanded == Some(i);
                    ui.horizontal(|ui| {
                        if ui.button(if open { "▾ source" } else { "▸ source" }).clicked() {
                            self.expanded = if open { None } else { Some(i) };
                        }
                        if ui.button("✦ Explain").on_hover_text("Send this diagnostic to Chat").clicked() {
                            send = Some(format!(
                                "Explain this Rust compiler diagnostic and how to fix it:\n\n{} [{}] {}:{}\n{}\n\n```rust\n{}\n```",
                                d.level.to_uppercase(), d.code.as_deref().unwrap_or("-"), d.file, d.line, d.message, d.source.join("\n")));
                        }
                        if ui.button("⧉ Copy").clicked() {
                            ui.ctx().copy_text(format!("{} [{}] {}:{}\n{}", d.level, d.code.as_deref().unwrap_or("-"), d.file, d.line, d.message));
                        }
                    });
                });
            }
            if self.diags.is_empty() && self.rx.is_none() {
                ui.label(RichText::new("no diagnostics — set a root and run Check").color(theme::FAINT()).small());
            }
        });
        // expanded source rendered after the loop to avoid borrow overlap
        if let Some(i) = self.expanded {
            if let Some(d) = self.diags.get(i) {
                egui::Frame { fill: Color32::from_rgb(0x16,0x16,0x16), corner_radius: egui::CornerRadius::same(6),
                    inner_margin: egui::Margin::same(10), ..Default::default() }
                .show(ui, |ui| {
                    for l in &d.source { ui.label(RichText::new(l).monospace().color(theme::DIM())); }
                });
            }
        }
        send
    }
}

fn level_color(level: &str) -> Color32 {
    match level { "error" => theme::RED(), "warning" => theme::YELLOW(), _ => theme::DIM() }
}

/// Map ONE line of `cargo check --message-format=json` to a `Diag`. Pure; unit-tested. Roll-up lines
/// ("aborting due to…", the "N warnings emitted" summary) are dropped — they're totals, not diagnostics.
fn parse_diag(v: &serde_json::Value) -> Option<Diag> {
    if v.get("reason")?.as_str()? != "compiler-message" { return None; }
    let m = v.get("message")?;
    let level = m.get("level")?.as_str()?.to_string();
    if level != "error" && level != "warning" { return None; }
    let message = m.get("message")?.as_str()?.to_string();
    if message.starts_with("aborting due to") || message.ends_with("warnings emitted")
        || message.starts_with("unused ") && message.contains("warnings") {
        return None;
    }
    let code = m.get("code").and_then(|c| c.get("code")).and_then(|c| c.as_str()).map(String::from);
    let spans = m.get("spans").and_then(|s| s.as_array());
    let prim = spans.and_then(|s| {
        s.iter().find(|sp| sp.get("is_primary").and_then(|b| b.as_bool()).unwrap_or(false)).or_else(|| s.first())
    });
    let file = prim.and_then(|s| s.get("file_name")).and_then(|f| f.as_str()).unwrap_or("").to_string();
    let line = prim.and_then(|s| s.get("line_start")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
    let source = m.get("rendered").and_then(|r| r.as_str())
        .map(|r| r.lines().take(14).map(String::from).collect()).unwrap_or_default();
    Some(Diag { file, line, level, message, code, source })
}

/// Bounded repo walk for the peek list: source-ish files, depth ≤ 6, `cap` entries, skipping the heavy
/// and generated trees. Deliberately tiny — the agent's Search/ReadRange handle the real navigation;
/// this is for a human eyeballing a repo from the Cargo tab.
fn scan_files(root: &Path, cap: usize) -> Vec<String> {
    const EXTS: [&str; 8] = ["rs", "toml", "md", "wgsl", "txt", "json", "sh", "py"];
    const SKIP: [&str; 6] = ["target", ".git", ".gitmind", "node_modules", "dist", ".cargo"];
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= cap || depth > 6 { continue; }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if out.len() >= cap { break; }
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') { stack.push((p, depth + 1)); }
            } else if p.extension().and_then(|x| x.to_str()).map(|x| EXTS.contains(&x)).unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diag_maps_a_real_cargo_json_line() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"borrow of moved value: `to_request`","code":{"code":"E0382"},"spans":[{"file_name":"src/world/chunk_manager.rs","line_start":412,"is_primary":true}],"rendered":"error[E0382]: borrow of moved value\n --> src/world/chunk_manager.rs:412:9\n"}}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let d = parse_diag(&v).expect("parses");
        assert_eq!((d.level.as_str(), d.file.as_str(), d.line), ("error", "src/world/chunk_manager.rs", 412));
        assert_eq!(d.code.as_deref(), Some("E0382"));
        assert!(d.source.iter().any(|l| l.contains("-->")));
        // non-diagnostic reasons and roll-ups are dropped
        let art: serde_json::Value = serde_json::from_str(r#"{"reason":"compiler-artifact"}"#).unwrap();
        assert!(parse_diag(&art).is_none());
        let roll: serde_json::Value = serde_json::from_str(
            r#"{"reason":"compiler-message","message":{"level":"error","message":"aborting due to 2 previous errors","spans":[]}}"#).unwrap();
        assert!(parse_diag(&roll).is_none());
    }

    #[test]
    fn scan_files_is_bounded_and_skips_generated_trees() {
        let d = std::env::temp_dir().join(format!("cargo-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target/debug")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(d.join("target/debug/junk.rs"), "").unwrap();
        let files = scan_files(&d, 100);
        assert!(files.contains(&"Cargo.toml".to_string()) && files.contains(&"src/main.rs".to_string()));
        assert!(!files.iter().any(|f| f.starts_with("target")), "target/ stays out: {files:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
