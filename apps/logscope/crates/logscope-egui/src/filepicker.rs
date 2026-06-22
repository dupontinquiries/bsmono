//! filepicker.rs — a small, dependency-free in-app file/folder picker (egui modal),
//! built on std::fs so it needs no native dialog (no rfd, no gtk). It has the things
//! you reach for coming from a KDE/Dolphin file dialog: a Places sidebar, a breadcrumb
//! with an Up button, a typed path box, a live search/filter, a hidden-files toggle,
//! a type filter, and human-readable sizes. Own one per call-site and drive it from `ui`.

use crate::theme;
use egui::RichText;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

pub enum Outcome { Picked(PathBuf), Cancelled }

// "System file picker": route to the desktop's own dialog. The XDG desktop portal is the standard
// mechanism — `xdg-desktop-portal-kde` renders the KDE (Dolphin-family) dialog, `-cosmic` renders the
// COSMIC picker, `-gnome`/`-gtk` the GTK one — so rfd's portal backend already reaches all of them when
// the matching backend is installed. On top of that we prefer KDE's `kdialog` binary directly (the true
// KDE dialog, and it works even when the portal backend is missing) and keep `zenity` as a generic
// fallback, with the built-in std::fs modal as the floor for headless/SSH sessions.
static NATIVE: AtomicBool = AtomicBool::new(true);
/// Toggle the system file dialog on/off process-wide (e.g. from settings for headless use).
pub fn set_native(on: bool) { NATIVE.store(on, Ordering::Relaxed); }
/// Whether the system dialog is currently preferred.
pub fn native_enabled() -> bool { NATIVE.load(Ordering::Relaxed) }

/// Which concrete system dialog will be used. `Builtin` means the in-app modal (no system picker found).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SysPicker { Portal, KDialog, Zenity, Builtin }

fn has_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
}
fn has_portal() -> bool { std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() && has_session() }
fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}
/// `bin` exists somewhere on PATH.
fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
    }).unwrap_or(false)
}

/// Pick the best system dialog for this session, or `Builtin` to use the in-app modal.
fn resolve_system_picker() -> SysPicker {
    if !NATIVE.load(Ordering::Relaxed) || !has_session() { return SysPicker::Builtin; }
    let de = desktop();
    // COSMIC has no CLI dialog — its native picker is reachable only through the portal backend.
    if de.contains("cosmic") {
        return if has_portal() { SysPicker::Portal } else { SysPicker::Builtin };
    }
    // KDE/Plasma: kdialog gives the genuine KDE (Dolphin-style) dialog directly and is robust even when
    // xdg-desktop-portal-kde isn't installed; fall back to the portal (kde backend) otherwise.
    if de.contains("kde") || de.contains("plasma") {
        if which("kdialog") { return SysPicker::KDialog; }
        if has_portal() { return SysPicker::Portal; }
    }
    // GNOME and everything else: the portal (gnome/gtk backend), then zenity, then a KDE dialog if it
    // happens to be installed, before giving up to the built-in modal.
    if has_portal() { return SysPicker::Portal; }
    if which("zenity") { return SysPicker::Zenity; }
    if which("kdialog") { return SysPicker::KDialog; }
    SysPicker::Builtin
}

/// Human-readable name of the dialog that will be used — for settings/tooltips.
pub fn system_picker_label() -> &'static str {
    match resolve_system_picker() {
        SysPicker::KDialog => "KDE / Dolphin (kdialog)",
        SysPicker::Portal => {
            let de = desktop();
            if de.contains("cosmic") { "COSMIC (portal)" }
            else if de.contains("kde") || de.contains("plasma") { "KDE (portal)" }
            else if de.contains("gnome") { "GNOME (portal)" }
            else { "system (portal)" }
        }
        SysPicker::Zenity => "zenity",
        SysPicker::Builtin => "built-in",
    }
}

/// A system dialog running on a background thread; the chosen path (or None on cancel) arrives over the
/// channel so the egui frame never blocks waiting on the dialog.
struct NativeJob { rx: Receiver<Option<PathBuf>> }

/// Run a CLI dialog tool (kdialog/zenity) and parse the chosen path from stdout.
fn run_cli_picker(cmd: &str, args: &[String]) -> Option<PathBuf> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() { return None; } // non-zero == user cancelled
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("").trim();
    if line.is_empty() { None } else { Some(PathBuf::from(line)) }
}

fn kdialog_pick(title: &str, pick_dir: bool, exts: &[String], start: &Path) -> Option<PathBuf> {
    let start_s = start.to_string_lossy().to_string();
    let mut args = vec!["--title".into(), title.into()];
    if pick_dir {
        args.push("--getexistingdirectory".into());
        args.push(start_s);
    } else {
        args.push("--getopenfilename".into());
        args.push(start_s);
        if !exts.is_empty() {
            // KDE filter syntax: "Description (*.ext1 *.ext2)"
            let pat = exts.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(" ");
            args.push(format!("files ({pat})"));
        }
    }
    run_cli_picker("kdialog", &args)
}

fn zenity_pick(title: &str, pick_dir: bool, exts: &[String], start: &Path) -> Option<PathBuf> {
    let mut args = vec!["--file-selection".into(), format!("--title={title}")];
    if pick_dir { args.push("--directory".into()); }
    // trailing slash hints zenity to open inside the directory
    args.push(format!("--filename={}/", start.to_string_lossy()));
    if !pick_dir && !exts.is_empty() {
        let pat = exts.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(" ");
        args.push(format!("--file-filter=files | {pat}"));
    }
    run_cli_picker("zenity", &args)
}

fn portal_pick(title: &str, pick_dir: bool, exts: &[String], start: &Path) -> Option<PathBuf> {
    let (title, start, exts) = (title.to_string(), start.to_path_buf(), exts.to_vec());
    async_std::task::block_on(async move {
        let mut d = rfd::AsyncFileDialog::new().set_title(&title);
        if start.is_dir() { d = d.set_directory(&start); }
        if !exts.is_empty() {
            let refs: Vec<&str> = exts.iter().map(String::as_str).collect();
            d = d.add_filter("files", &refs);
        }
        if pick_dir { d.pick_folder().await.map(|h| h.path().to_path_buf()) }
        else { d.pick_file().await.map(|h| h.path().to_path_buf()) }
    })
}

fn spawn_native(strat: SysPicker, title: String, pick_dir: bool, exts: Vec<String>, start: PathBuf) -> NativeJob {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = match strat {
            SysPicker::Portal => portal_pick(&title, pick_dir, &exts, &start),
            SysPicker::KDialog => kdialog_pick(&title, pick_dir, &exts, &start),
            SysPicker::Zenity => zenity_pick(&title, pick_dir, &exts, &start),
            SysPicker::Builtin => None,
        };
        let _ = tx.send(res);
    });
    NativeJob { rx }
}

#[derive(Default)]
pub struct FilePicker {
    open: bool,
    cwd: PathBuf,
    query: String,
    show_hidden: bool,
    title: String,
    pick_dir: bool,        // choosing a directory rather than a file
    exts: Vec<String>,     // allowed file extensions (lowercase, no dot); empty = any
    selected: Option<PathBuf>,
    path_edit: String,
    error: Option<String>,
    native: Option<NativeJob>,   // when Some, a native portal dialog is in flight (no modal shown)
}

struct Entry { name: String, path: PathBuf, is_dir: bool, size: u64 }

fn default_dir() -> PathBuf {
    std::env::var("HOME").ok().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from("/"))
}

fn places() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Ok(h) = std::env::var("HOME") {
        let h = PathBuf::from(h);
        v.push(("🏠 Home".into(), h.clone()));
        for sub in ["Downloads", "Documents"] {
            let p = h.join(sub);
            if p.is_dir() { v.push((format!("   {sub}"), p)); }
        }
    }
    v.push(("   /".into(), PathBuf::from("/")));
    for p in ["/var/log", "/etc", "/tmp"] {
        if Path::new(p).is_dir() { v.push((format!("   {p}"), PathBuf::from(p))); }
    }
    v
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let (mut f, mut i) = (n as f64, 0usize);
    while f >= 1024.0 && i + 1 < U.len() { f /= 1024.0; i += 1; }
    if i == 0 { format!("{n} B") } else { format!("{f:.1}{}", U[i]) }
}

impl FilePicker {
    #[allow(dead_code)]
    pub fn is_open(&self) -> bool { self.open }

    /// Open to pick a FILE, restricted to `exts` (empty = any type).
    pub fn open_file(&mut self, start: &Path, title: &str, exts: &[&str]) { self.start(start, title, false, exts); }
    /// Open to pick a DIRECTORY.
    pub fn open_dir(&mut self, start: &Path, title: &str) { self.start(start, title, true, &[]); }

    fn start(&mut self, start: &Path, title: &str, pick_dir: bool, exts: &[&str]) {
        let cwd = if start.is_dir() { start.to_path_buf() }
                  else { start.parent().map(Path::to_path_buf).unwrap_or_else(default_dir) };
        self.cwd = if cwd.is_dir() { cwd } else { default_dir() };
        self.title = title.to_string();
        self.pick_dir = pick_dir;
        self.exts = exts.iter().map(|e| e.trim_start_matches('.').to_lowercase()).collect();
        self.query.clear();
        self.selected = None;
        self.path_edit = self.cwd.display().to_string();
        self.error = None;
        // System dialog (KDE/COSMIC/portal/zenity) by default; built-in modal when none is available.
        match resolve_system_picker() {
            SysPicker::Builtin => { self.native = None; self.open = true; }
            strat => {
                self.native = Some(spawn_native(strat, self.title.clone(), pick_dir, self.exts.clone(), self.cwd.clone()));
                self.open = false;
            }
        }
    }

    fn navigate(&mut self, to: PathBuf) {
        if to.is_dir() {
            self.cwd = to;
            self.path_edit = self.cwd.display().to_string();
            self.query.clear();
            self.selected = None;
            self.error = None;
        } else {
            self.error = Some(format!("not a directory: {}", to.display()));
        }
    }

    fn read_entries(&self) -> Vec<Entry> {
        let mut v = Vec::new();
        let q = self.query.to_lowercase();
        match fs::read_dir(&self.cwd) {
            Ok(rd) => for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !self.show_hidden && name.starts_with('.') { continue; }
                if !q.is_empty() && !name.to_lowercase().contains(&q) { continue; }
                let md = e.metadata().ok();
                let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let path = e.path();
                if !is_dir && !self.exts.is_empty() {
                    let ok = path.extension().and_then(|x| x.to_str())
                        .map(|x| self.exts.contains(&x.to_lowercase())).unwrap_or(false);
                    if !ok { continue; }
                }
                v.push(Entry { name, path, is_dir, size: md.as_ref().map(|m| m.len()).unwrap_or(0) });
            },
            Err(_) => {}
        }
        v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }

    /// Render the modal. Returns `Some` once the user picks or cancels.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        // A native portal dialog is running on its thread — poll without blocking the frame.
        if let Some(job) = &self.native {
            match job.rx.try_recv() {
                Ok(picked) => { self.native = None; return Some(picked.map(Outcome::Picked).unwrap_or(Outcome::Cancelled)); }
                Err(mpsc::TryRecvError::Empty) => { ctx.request_repaint(); return None; }
                Err(mpsc::TryRecvError::Disconnected) => { self.native = None; return Some(Outcome::Cancelled); }
            }
        }
        if !self.open { return None; }
        let mut outcome: Option<Outcome> = None;
        let mut window_open = true;
        egui::Window::new(RichText::new(&self.title).strong())
            .collapsible(false).resizable(true).default_size([660.0, 480.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut window_open)
            .show(ctx, |ui| {
                // ── breadcrumb + up ──
                ui.horizontal(|ui| {
                    if ui.button("⬆").on_hover_text("Up one level").clicked() {
                        if let Some(p) = self.cwd.parent() { let p = p.to_path_buf(); self.navigate(p); }
                    }
                    let cwd = self.cwd.clone();
                    let mut acc = PathBuf::new();
                    let crumbs: Vec<(String, PathBuf)> = cwd.components().map(|c| {
                        acc.push(c.as_os_str());
                        let name = c.as_os_str().to_string_lossy().to_string();
                        let label = if name == "/" || name.is_empty() { "/".to_string() } else { name };
                        (label, acc.clone())
                    }).collect();
                    for (i, (label, p)) in crumbs.iter().enumerate() {
                        if ui.add(egui::Button::new(RichText::new(label).color(theme::CYAN()).small()).frame(false)).clicked() {
                            let p = p.clone(); self.navigate(p);
                        }
                        if i + 1 < crumbs.len() && label != "/" { ui.label(RichText::new("/").color(theme::FAINT()).small()); }
                    }
                });
                // ── typed path ──
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Path").color(theme::DIM()).small());
                    let r = ui.add(egui::TextEdit::singleline(&mut self.path_edit)
                        .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let p = PathBuf::from(self.path_edit.trim()); self.navigate(p);
                    }
                });
                // ── search + filters ──
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    ui.add(egui::TextEdit::singleline(&mut self.query)
                        .hint_text("search in this folder…").desired_width(280.0));
                    if !self.query.is_empty() && ui.button("✕").on_hover_text("clear").clicked() { self.query.clear(); }
                    theme::switch_row(ui, "hidden", &mut self.show_hidden, theme::AMBER());
                    if !self.exts.is_empty() {
                        let f = self.exts.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(" ");
                        ui.label(RichText::new(format!("· {f}")).color(theme::DIM()).small());
                    }
                });
                ui.separator();

                // ── places sidebar + entry list ──
                let entries = self.read_entries();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(140.0);
                        ui.label(RichText::new("Places").color(theme::DIM()).small());
                        for (label, path) in places() {
                            if ui.add(egui::Button::new(RichText::new(label).small()).frame(false)).clicked() {
                                self.navigate(path);
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        if let Some(e) = &self.error { ui.label(RichText::new(e).color(theme::RED()).small()); }
                        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(300.0)
                            .id_salt("fp_entries").show(ui, |ui| {
                            for e in &entries {
                                let sel = self.selected.as_deref() == Some(e.path.as_path());
                                let icon = if e.is_dir { "📁" } else { "  " };
                                let col = if e.is_dir { theme::CYAN() } else { theme::INK() };
                                let size = if e.is_dir { String::new() } else { human(e.size) };
                                ui.horizontal(|ui| {
                                    let r = ui.selectable_label(sel,
                                        RichText::new(format!("{icon} {}", e.name)).monospace().color(col));
                                    if !size.is_empty() {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(size).color(theme::FAINT()).small().monospace());
                                        });
                                    }
                                    if r.double_clicked() {
                                        if e.is_dir { self.navigate(e.path.clone()); }
                                        else { outcome = Some(Outcome::Picked(e.path.clone())); }
                                    } else if r.clicked() {
                                        self.selected = Some(e.path.clone());
                                    }
                                });
                            }
                            if entries.is_empty() {
                                ui.label(RichText::new("— empty (or nothing matches the filter) —").color(theme::FAINT()).small());
                            }
                        });
                    });
                });

                ui.separator();
                ui.horizontal(|ui| {
                    let can = if self.pick_dir { true }
                              else { self.selected.as_ref().map(|p| p.is_file()).unwrap_or(false) };
                    let label = if self.pick_dir { "Choose folder" } else { "Open" };
                    if theme::action_button(ui, label, theme::GREEN(), can).clicked() {
                        let p = if self.pick_dir { self.selected.clone().unwrap_or_else(|| self.cwd.clone()) }
                                else { self.selected.clone().unwrap() };
                        outcome = Some(Outcome::Picked(p));
                    }
                    if ui.button("Cancel").clicked() { outcome = Some(Outcome::Cancelled); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(s) = &self.selected {
                            let n = s.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                            ui.label(RichText::new(n).color(theme::AMBER_SOFT()).small());
                        }
                    });
                });
            });
        if !window_open { outcome = Some(Outcome::Cancelled); }
        if outcome.is_some() { self.open = false; }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::human;
    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0K");
        assert_eq!(human(1536), "1.5K");
        assert_eq!(human(1048576), "1.0M");
    }
}
