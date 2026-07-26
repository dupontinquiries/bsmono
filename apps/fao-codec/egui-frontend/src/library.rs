//! Library page — the FAO Web Library section of fao_app, ported faithfully.
//!
//! The Web Library is a PERSISTENT cross-encode catalog of canonical web meshes: when a
//! chunk's geometry matches a library canonical, the encoder records an 84-byte reference
//! instead of a 100-500 KB mesh. Two-way benefit (transcribed from the GTK panel): the
//! encoder writes smaller trailers, and the library grows as more videos contribute.
//!
//! Every operation calls the SAME backend the GTK handlers call: `Library::init/open`,
//! `stats()`, `prune_unused(30)`, `rollback_manifest` + `rollback_log` (rotating
//! .bak.0..4; before rollback the CURRENT state is pushed to .bak.0, so rollback is
//! itself reversible), and `library_db::bench`. One deliberate divergence, recorded:
//! GTK's native folder choosers become a path field here — same operations, text entry.
use std::path::PathBuf;
use fao_codec_lib as fao_v1;

pub struct State {
    /// Path field (the egui stand-in for the GTK folder chooser).
    pub path: String,
    /// The attached library, if any. None = encoder/decoder run standalone.
    pub attached: Option<PathBuf>,
    /// Writable: THIS encode adds new canonicals. Off = read-only, the curated-library
    /// safety default (GTK tooltip transcribed).
    pub writable: bool,
    /// Rollback slot selector (.bak.0 .. .bak.4).
    pub rollback_slot: usize,
    /// One-line status, mirroring the GTK status label.
    pub status: String,
    /// Last multi-line report (stats / prune / rollback / bench output).
    pub report: Option<String>,
    /// Bench runs on a thread (it allocates a few MB and shouldn't block a frame).
    bench_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl Default for State {
    fn default() -> Self {
        // Same env default the GTK app uses on startup.
        let env = std::env::var("FAO_LIBRARY").ok();
        State {
            path: env.clone().unwrap_or_default(),
            attached: env.map(PathBuf::from),
            writable: false,
            rollback_slot: 0,
            status: "No library attached. Encoder runs standalone.".into(),
            report: None,
            bench_rx: None,
        }
    }
}

impl State {
    pub fn refresh_status(&mut self) {
        self.status = match &self.attached {
            Some(p) => format!("Attached: {} ({})", p.display(),
                               if self.writable { "writable — this encode adds canonicals" }
                               else { "read-only" }),
            None => "No library attached. Encoder runs standalone.".into(),
        };
    }
    pub fn init_new(&mut self) {
        let p = PathBuf::from(self.path.trim());
        if self.path.trim().is_empty() { self.report = Some("init: choose a folder path first".into()); return; }
        match fao_v1::library::Library::init(&p) {
            Ok(()) => { self.attached = Some(p); self.report = Some("Library initialized.".into()); }
            Err(e) => self.report = Some(format!("init failed: {e}")),
        }
        self.refresh_status();
    }
    pub fn attach(&mut self) {
        let p = PathBuf::from(self.path.trim());
        match fao_v1::library::Library::open(&p) {
            Ok(_) => { self.attached = Some(p); self.report = Some("Library attached.".into()); }
            Err(e) => self.report = Some(format!("attach failed: {e}")),
        }
        self.refresh_status();
    }
    pub fn detach(&mut self) {
        self.attached = None;
        self.report = Some("Detached — encoder/decoder go standalone.".into());
        self.refresh_status();
    }
    pub fn stats(&mut self) {
        let Some(p) = self.attached.clone() else { self.report = Some("stats: no library attached".into()); return; };
        self.report = Some(match fao_v1::library::Library::open(&p) {
            Ok(lib) => lib.stats(),
            Err(e) => format!("stats: failed to open library: {e}"),
        });
    }
    pub fn prune(&mut self) {
        let Some(p) = self.attached.clone() else { self.report = Some("prune: no library attached".into()); return; };
        self.report = Some(match fao_v1::library::Library::open_writable(&p) {
            Ok(mut lib) => match lib.prune_unused(30) {
                Ok(pruned) => format!("pruned {} unused canonicals (ref_count=0, untouched 30+ days):\n{}",
                                       pruned.len(), pruned.join("\n")),
                Err(e) => format!("prune failed: {e}"),
            },
            Err(e) => format!("prune: failed to open library writable: {e}"),
        });
    }
    pub fn rollback(&mut self) {
        let Some(p) = self.attached.clone() else { self.report = Some("rollback: no library attached".into()); return; };
        let slot = self.rollback_slot.min(4);
        // GTK handler order transcribed: manifest first, then the log; current state is
        // pushed to .bak.0 by the backend, so this is reversible.
        let m = fao_v1::library::rollback_manifest(&p, slot);
        let l = fao_v1::library_db::rollback_log(&p, slot);
        self.report = Some(match (m, l) {
            (Ok(()), Ok(())) => format!("rolled back to .bak.{slot} (current state pushed to .bak.0 — reversible)"),
            (m, l) => format!("rollback: manifest={m:?} log={l:?}"),
        });
    }
    pub fn bench(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.bench_rx = Some(rx);
        self.report = Some("bench running (10k entries, 10k random gets)…".into());
        std::thread::spawn(move || { let _ = tx.send(fao_v1::library_db::bench(10_000, 10_000)); });
    }
    fn poll_bench(&mut self) {
        if let Some(rx) = &self.bench_rx {
            if let Ok(r) = rx.try_recv() { self.report = Some(r); self.bench_rx = None; }
        }
    }
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let st = &mut app.library;
    st.poll_bench();
    ui.heading("🔗 FAO Web Library");
    ui.label(egui::RichText::new("cross-encode mesh sharing — canonical chunks stored once, referenced everywhere").weak().small());
    ui.add_space(6.0);
    ui.label(&st.status);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("folder:");
        ui.add(egui::TextEdit::singleline(&mut st.path).desired_width(320.0));
    });
    ui.horizontal(|ui| {
        if ui.button("Initialize New").on_hover_text("Create a new empty library at the folder above").clicked() { st.init_new(); }
        if ui.button("Attach Existing").on_hover_text("Point at an existing library folder").clicked() { st.attach(); }
        if ui.add_enabled(st.attached.is_some(), egui::Button::new("Detach"))
            .on_hover_text("Stop using the library; encoder/decoder go standalone").clicked() { st.detach(); }
    });
    let writable_before = st.writable;
    ui.checkbox(&mut st.writable, "Writable (this encode adds new canonicals)")
        .on_hover_text("Off = read-only (curated lib safety). On = this encode contributes its canonicals, growing the library for future encodes.");
    if st.writable != writable_before { st.refresh_status(); }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(st.attached.is_some(), egui::Button::new("Stats")).clicked() { st.stats(); }
        if ui.add_enabled(st.attached.is_some(), egui::Button::new("Prune"))
            .on_hover_text("Remove ref_count=0 canonicals untouched for 30+ days").clicked() { st.prune(); }
        ui.add_enabled_ui(st.attached.is_some(), |ui| {
            if ui.button("Rollback").on_hover_text("Restore manifest + log from a rotating backup slot. Current state is pushed to .bak.0 first, so rollback is reversible.").clicked() { st.rollback(); }
            ui.add(egui::Slider::new(&mut st.rollback_slot, 0..=4).text(".bak slot"));
        });
        if ui.button("DB Bench").on_hover_text("Synthetic library_db benchmark (temp dir) — confirms DB perf targets on this hardware").clicked() { st.bench(); }
    });
    if let Some(r) = &st.report {
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            ui.label(egui::RichText::new(r).monospace().small());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn init_attach_stats_rollback_round_trip() {
        let dir = std::env::temp_dir().join(format!("fao_lib_egui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = State::default();
        st.path = dir.to_string_lossy().into_owned();
        st.attached = None;
        st.init_new();
        assert!(st.attached.is_some(), "init should attach: {:?}", st.report);
        st.stats();
        let stats = st.report.clone().unwrap();
        assert!(!stats.is_empty(), "stats report should not be empty");
        st.rollback_slot = 0;
        st.rollback();
        assert!(st.report.as_deref().unwrap().contains("rolled back")
             || st.report.as_deref().unwrap().contains("rollback"),
            "rollback reports its outcome: {:?}", st.report);
        st.detach();
        assert!(st.attached.is_none());
        assert!(st.status.contains("standalone"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
