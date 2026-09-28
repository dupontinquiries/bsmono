//! The window, and the disk.
//!
//! Everything here is either eframe plumbing or one of the four things
//! the shell will not do itself: start a scan, move a file to the trash,
//! write the index, write the grants. The scan runs on the scanner's
//! thread and arrives as events; this file feeds them to the shell one
//! frame at a time.
//!
//! Usage: `photosync-egui [FOLDER]`. Without a folder: `$XDG_PICTURES_DIR`
//! if set, else `~/Pictures`. The choice is remembered.

use photosync_egui::library::Index;
use photosync_egui::scan::{self, ScanEvent};
use photosync_egui::{HostAction, Shell};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

const APP: &str = "photosync";
const SETTINGS: &str = "settings.conf";
const GRANTS: &str = "grants.tsv";
/// Trashed files go here, under the library root, where a file manager
/// can find them and the scanner's hidden-directory rule skips them.
const TRASH_DIR: &str = ".photosync-trash";

struct App {
    shell: Shell,
    colors: uikit_egui::PaletteColors,
    scan: Option<Receiver<ScanEvent>>,
    save_due: Option<std::time::Instant>,
}

impl App {
    fn start_scan(&mut self, root: &str) {
        let root = root.to_string();
        let index = Index::load(&root).unwrap_or_default();
        // A different root: forget the old library entirely.
        if self.shell.library.root != root {
            self.shell.library = index.to_library();
            self.shell.library.root = root.clone();
            self.shell.similar = None;
            self.shell.viewer = None;
            self.restore_grants();
            let _ = config_kit::save(APP, SETTINGS, &format!("root = {root}\n"));
        }
        self.shell.lib_page.root_text = root.clone();
        self.shell.scan_status().start();
        self.shell.note(format!("scanning {root} ({} indexed)", index.entries.len()));
        self.scan = Some(scan::spawn(PathBuf::from(&root), index, scan::default_cache_dir()));
    }

    fn restore_grants(&mut self) {
        if let Some(text) = config_kit::load(APP, GRANTS) {
            let lib = self.shell.library.clone();
            let n = self.shell.share.restore_grants(&text, |key| {
                let i = lib.find(key)?;
                let p = &lib.photos[i];
                Some(photosync_egui::share::PhotoRecord {
                    name: p.name().to_string(), size: p.size, dhash: p.entry.dhash,
                    tags: p.written_tags().iter().map(|t| t.to_string()).collect(), at: p.at,
                })
            });
            if n > 0 { self.shell.note(format!("{n} grant(s) restored")); }
        }
    }

    fn pump_scan(&mut self) {
        let Some(rx) = &self.scan else { return };
        let mut got = 0;
        // Bounded per frame so a fast cache restore does not freeze the
        // first frame uploading two thousand thumbnails at once.
        while got < 32 {
            match rx.try_recv() {
                Ok(ScanEvent::Found(n)) => { self.shell.scan_status().found = n; }
                Ok(ScanEvent::Photo(p)) => { self.shell.add_photo(*p); self.shell.scan_status().done += 1; got += 1; }
                Ok(ScanEvent::Failed { path, why }) => { self.shell.scan_status().failed.push((path, why)); }
                Ok(ScanEvent::Done { scanned, from_cache }) => {
                    let st = self.shell.scan_status();
                    st.running = false;
                    st.done = scanned;
                    st.from_cache = from_cache;
                    let line = st.line();
                    self.shell.note(format!("scan finished: {line}"));
                    self.scan = None;
                    self.save_due = Some(std::time::Instant::now());
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.shell.scan_status().running = false; self.scan = None; return; }
            }
        }
    }

    fn save(&mut self) {
        let idx = Index::from_library(&self.shell.library);
        match idx.save() {
            Ok(()) => {}
            Err(e) => self.shell.note(format!("could not save the index: {e}")),
        }
        if let Err(e) = config_kit::save(APP, GRANTS, &self.shell.share.render_grants()) {
            self.shell.note(format!("could not save the grants: {e}"));
        }
    }

    fn apply(&mut self, a: HostAction) {
        match a {
            HostAction::Trash(paths) => {
                let trash = Path::new(&self.shell.library.root).join(TRASH_DIR);
                if let Err(e) = std::fs::create_dir_all(&trash) {
                    self.shell.note(format!("could not create {}: {e}", trash.display()));
                    return;
                }
                for p in paths {
                    let name = Path::new(&p).file_name().map(|n| n.to_os_string()).unwrap_or_default();
                    let mut dest = trash.join(&name);
                    let mut n = 1;
                    while dest.exists() {
                        dest = trash.join(format!("{}-{n}", name.to_string_lossy()));
                        n += 1;
                    }
                    match std::fs::rename(&p, &dest) {
                        Ok(()) => self.shell.note(format!("moved {} to {}", p, dest.display())),
                        Err(e) => self.shell.note(format!("could not move {p}: {e}")),
                    }
                }
            }
            HostAction::Rescan => { let root = self.shell.library.root.clone(); self.start_scan(&root); }
            HostAction::ChangeRoot(r) => {
                if Path::new(&r).is_dir() { self.start_scan(&r); }
                else { self.shell.note(format!("{r} is not a folder")); }
            }
            HostAction::Save => { self.save_due = Some(std::time::Instant::now()); }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        self.pump_scan();
        egui::CentralPanel::default().show(ctx, |ui| { self.shell.ui(ui, &self.colors); });
        for a in std::mem::take(&mut self.shell.pending) { self.apply(a); }
        // Saves are coalesced: a burst of tag edits writes once, a second
        // after the last one.
        if let Some(t) = self.save_due {
            if t.elapsed() > std::time::Duration::from_secs(1) { self.save_due = None; self.save(); }
            else { ctx.request_repaint_after(std::time::Duration::from_millis(300)); }
        }
        if self.scan.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
    }
}

fn default_root() -> String {
    if let Some(text) = config_kit::load(APP, SETTINGS) {
        let ini = config_kit::Ini::parse(&text);
        if let Some(r) = ini.get("", "root").or_else(|| ini.get("general", "root")) {
            if Path::new(r.trim()).is_dir() { return r.trim().to_string(); }
        }
    }
    if let Some(d) = std::env::var_os("XDG_PICTURES_DIR") { return d.to_string_lossy().to_string(); }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/Pictures")
}

fn main() -> eframe::Result<()> {
    const APP_ID: &str = "org.vidkit.PhotoSync";
    let root = std::env::args().nth(1).unwrap_or_else(default_root);
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "PhotoSync", [420.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native("photosync-egui", opts, Box::new(move |cc| {
        let colors = uikit_egui::palette_colors_ctx(&cc.egui_ctx, "cloudscape");
        let mut app = App {
            shell: Shell::new(scan::default_cache_dir(), "photosync"),
            colors, scan: None, save_due: None,
        };
        app.shell.contacts = photosync_egui::share::load_contacts();
        let n = app.shell.contacts.len();
        app.shell.note(format!("{n} snapclone contact(s)"));
        app.start_scan(&root);
        Ok(Box::new(app))
    }))
}
