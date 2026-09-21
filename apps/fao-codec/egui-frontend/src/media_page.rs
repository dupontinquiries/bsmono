//! media_page — the Media tab: the egui shell over `media`.
//!
//! Add local folders and dufs servers, pick files by pattern or one at a time,
//! and open a `.fao` straight into the player. All the logic lives in `media`
//! and is tested there; this file is layout and wiring.

use crate::media::{self, Entry, Kind, Scan, Selection, Source};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// One file to encode: where ffmpeg reads it, and where the .fao goes.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeItem {
    pub name: String,
    /// A local path, or a dufs file URL -- ffmpeg reads HTTP directly, using
    /// the range requests dufs serves.
    pub input: String,
    pub output: PathBuf,
}

/// What the encode thread reports back.
enum JobEvent {
    Started(usize, String),
    Progress(usize, usize),
    Finished(PathBuf, Result<crate::transcode::TranscodeReport, String>),
    AllDone,
}

/// A running encode.
pub struct EncodeJob {
    rx: mpsc::Receiver<JobEvent>,
    cancel: Arc<AtomicBool>,
    pub total: usize,
    pub index: usize,
    pub current: String,
    pub frames: (usize, usize),
    pub done: usize,
    pub log: Vec<String>,
    pub running: bool,
}

/// How many folders a dufs walk visits before stopping, so a huge share
/// reports "stopped" instead of hanging.
const DUFS_MAX_DIRS: usize = 5_000;

pub struct State {
    pub sources: Vec<Source>,
    pub entries: Vec<Entry>,
    pub errors: Vec<(String, String)>,
    pub selection: Selection,
    pub new_folder: String,
    pub new_server: String,
    /// Include / exclude patterns, one per line.
    pub include_text: String,
    pub exclude_text: String,
    pub only_selected: bool,
    /// Scans in flight: each thread sends back its source index and result.
    rx: Option<mpsc::Receiver<(usize, Scan)>>,
    tx: Option<mpsc::Sender<(usize, Scan)>>,
    pending: usize,
    /// Where encodes go. Empty = beside each local source file.
    pub output_dir: String,
    pub quality: u8,
    pub job: Option<EncodeJob>,
    /// Frames sampled per file by Analyze.
    pub samples: usize,
    pub analysis: Option<Result<crate::analyze::Report, String>>,
    analyzing: Option<mpsc::Receiver<AnalyzeEvent>>,
    pub analyze_progress: (usize, usize),
}

enum AnalyzeEvent {
    Progress(usize, usize),
    Done(Result<crate::analyze::Report, String>),
}

impl Default for State {
    fn default() -> Self {
        State {
            sources: Vec::new(), entries: Vec::new(), errors: Vec::new(),
            selection: Selection::default(), new_folder: String::new(), new_server: String::new(),
            include_text: String::new(), exclude_text: String::new(), only_selected: false,
            rx: None, tx: None, pending: 0,
            output_dir: String::new(), quality: 80, job: None,
            samples: crate::analyze::DEFAULT_SAMPLES, analysis: None, analyzing: None,
            analyze_progress: (0, 0),
        }
    }
}

/// Split a text box into patterns: one per line, blanks and `#` comments out.
pub fn patterns(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from).collect()
}

/// A dufs listing over HTTP. The transport half of `media::scan_dufs`.
///
/// Timeouts are short and explicit: a LAN server that has gone to sleep should
/// cost a few seconds and an error line, not a scan thread stuck forever.
fn http_fetch(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(4))
        .timeout_read(std::time::Duration::from_secs(20))
        .build();
    agent.get(url).call().map_err(|e| e.to_string())?
        .into_string().map_err(|e| e.to_string())
}

impl State {
    fn channel(&mut self) -> mpsc::Sender<(usize, Scan)> {
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        self.tx.clone().unwrap()
    }

    /// Add a source and scan it on a thread.
    pub fn add(&mut self, src: Source) {
        if self.sources.contains(&src) {
            return;
        }
        let idx = self.sources.len();
        self.sources.push(src.clone());
        let tx = self.channel();
        self.pending += 1;
        std::thread::spawn(move || {
            let scan = match &src {
                Source::Local(p) => media::scan_local(p, idx),
                Source::Dufs(url) => media::scan_dufs(url, idx, DUFS_MAX_DIRS, &mut |u| http_fetch(u)),
            };
            let _ = tx.send((idx, scan));
        });
    }

    /// Collect finished scans. Called every frame; cheap when nothing arrived.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((_, scan)) = rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            self.entries.extend(scan.entries);
            self.errors.extend(scan.errors);
        }
        self.entries.sort_by(|a, b| (a.source, &a.rel).cmp(&(b.source, &b.rel)));
    }

    /// Remove a source and everything it contributed.
    pub fn remove(&mut self, idx: usize) {
        if idx >= self.sources.len() {
            return;
        }
        self.sources.remove(idx);
        self.entries.retain(|e| e.source != idx);
        // Indices after the removed one shift down.
        for e in &mut self.entries {
            if e.source > idx { e.source -= 1; }
        }
        self.selection.on = self.selection.on.iter()
            .filter(|(s, _)| *s != idx).map(|(s, r)| (if *s > idx { s - 1 } else { *s }, r.clone())).collect();
        self.selection.off = self.selection.off.iter()
            .filter(|(s, _)| *s != idx).map(|(s, r)| (if *s > idx { s - 1 } else { *s }, r.clone())).collect();
    }

    fn sync_patterns(&mut self) {
        self.selection.include = patterns(&self.include_text);
        self.selection.exclude = patterns(&self.exclude_text);
    }

    /// The local path of an entry, if its source is a folder.
    pub fn local_path(&self, e: &Entry) -> Option<std::path::PathBuf> {
        match self.sources.get(e.source)? {
            Source::Local(root) => Some(root.join(&e.rel)),
            Source::Dufs(_) => None,
        }
    }
}

impl State {
    /// Where ffmpeg reads an entry from: a local path, or a dufs file URL.
    pub fn input_for(&self, e: &Entry) -> Option<String> {
        match self.sources.get(e.source)? {
            Source::Local(root) => Some(root.join(&e.rel).display().to_string()),
            Source::Dufs(base) => Some(media::file_url(base, &e.rel)),
        }
    }

    /// The selected SOURCE videos, with where to read each.
    fn selected_sources(&self) -> Vec<(String, String)> {
        self.entries.iter()
            .filter(|e| e.kind == Kind::SourceVideo && self.selection.selected(e))
            .filter_map(|e| Some((e.rel.clone(), self.input_for(e)?)))
            .collect()
    }

    /// Estimate what a SHARED block dictionary would save across the selected
    /// source videos, on a thread. See `analyze` for the method and its limits.
    pub fn start_analyze(&mut self) {
        let files = self.selected_sources();
        if files.is_empty() || self.analyzing.is_some() { return; }
        let (tx, rx) = mpsc::channel();
        let k = self.samples.max(1);
        let total = files.len();
        std::thread::spawn(move || {
            let mut keep = std::collections::HashMap::new();
            let mut blocks = Vec::new();
            let mut failed = Vec::new();
            for (i, (name, input)) in files.into_iter().enumerate() {
                match crate::analyze::sample_file(&input, &name, k, &mut keep) {
                    Ok(fb) => blocks.push(fb),
                    Err(e) => failed.push(e),
                }
                let _ = tx.send(AnalyzeEvent::Progress(i + 1, total));
            }
            let r = if blocks.is_empty() {
                Err(format!("nothing could be sampled: {}", failed.join("; ")))
            } else {
                Ok(crate::analyze::analyze(&blocks, crate::analyze::bytes_per_block(&keep)))
            };
            let _ = tx.send(AnalyzeEvent::Done(r));
        });
        self.analyzing = Some(rx);
        self.analysis = None;
        self.analyze_progress = (0, total);
    }

    pub fn poll_analyze(&mut self) {
        let Some(rx) = &self.analyzing else { return };
        let mut done = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AnalyzeEvent::Progress(d, t) => self.analyze_progress = (d, t),
                AnalyzeEvent::Done(r) => done = Some(r),
            }
        }
        if let Some(r) = done { self.analysis = Some(r); self.analyzing = None; }
    }

    /// Plan an encode of the selected SOURCE videos. Returns the work and, for
    /// anything skipped, why. Pure over state, so it is tested without ffmpeg.
    ///
    /// Skips rather than guesses:
    ///   * a file whose `.fao` already exists is not re-encoded or overwritten;
    ///   * a server file with no output folder set has nowhere local to go.
    pub fn plan_encode(&self) -> (Vec<EncodeItem>, Vec<String>) {
        let out_dir = self.output_dir.trim();
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        for e in self.entries.iter().filter(|e| e.kind == Kind::SourceVideo && self.selection.selected(e)) {
            let stem = e.name().rsplit_once('.').map(|(s, _)| s).unwrap_or(e.name()).to_string();
            let (input, dir) = match self.sources.get(e.source) {
                Some(Source::Local(root)) => {
                    let p = root.join(&e.rel);
                    let dir = if out_dir.is_empty() {
                        p.parent().map(PathBuf::from).unwrap_or_else(|| root.clone())
                    } else { PathBuf::from(out_dir) };
                    (p.display().to_string(), dir)
                }
                Some(Source::Dufs(_)) => {
                    if out_dir.is_empty() {
                        skipped.push(format!("{}: set an output folder for server files", e.rel));
                        continue;
                    }
                    match self.input_for(e) { Some(u) => (u, PathBuf::from(out_dir)), None => continue }
                }
                None => continue,
            };
            let output = dir.join(format!("{stem}.fao"));
            if output.exists() || items.iter().any(|i: &EncodeItem| i.output == output) {
                skipped.push(format!("{}: {} already exists", e.rel, output.display()));
                continue;
            }
            items.push(EncodeItem { name: e.rel.clone(), input, output });
        }
        (items, skipped)
    }

    /// Start encoding `items` on a thread.
    pub fn start_encode(&mut self, items: Vec<EncodeItem>) {
        if items.is_empty() || self.job.as_ref().is_some_and(|j| j.running) { return; }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let quality = self.quality;
        let total = items.len();
        std::thread::spawn(move || {
            for (i, it) in items.into_iter().enumerate() {
                if stop.load(Ordering::Relaxed) { break; }
                let _ = tx.send(JobEvent::Started(i, it.name.clone()));
                if let Some(parent) = it.output.parent() { let _ = std::fs::create_dir_all(parent); }
                let ptx = tx.clone();
                let mut last = std::time::Instant::now();
                let r = crate::transcode::transcode_to_file_cancellable(&it.input, &it.output, quality, None,
                    |d, t| {
                        // A message per frame floods the channel at 144 fps;
                        // ten a second is plenty for a progress bar.
                        if last.elapsed().as_millis() >= 100 || d == t {
                            last = std::time::Instant::now();
                            let _ = ptx.send(JobEvent::Progress(d, t));
                        }
                    }, Some(&stop));
                let _ = tx.send(JobEvent::Finished(it.output, r));
            }
            let _ = tx.send(JobEvent::AllDone);
        });
        self.job = Some(EncodeJob { rx, cancel, total, index: 0, current: String::new(),
                                    frames: (0, 0), done: 0, log: Vec::new(), running: true });
    }

    /// Collect encode events; add each finished .fao to the collection.
    pub fn poll_encode(&mut self) {
        let mut finished: Vec<PathBuf> = Vec::new();
        if let Some(job) = &mut self.job {
            while let Ok(ev) = job.rx.try_recv() {
                match ev {
                    JobEvent::Started(i, name) => { job.index = i; job.current = name; job.frames = (0, 0); }
                    JobEvent::Progress(d, t) => job.frames = (d, t),
                    JobEvent::Finished(out, Ok(rep)) => {
                        job.done += 1;
                        job.log.push(format!("✔ {} — {} frames, {:.1} dB", out.display(), rep.frames, rep.mean_psnr));
                        finished.push(out);
                    }
                    JobEvent::Finished(out, Err(e)) => job.log.push(format!("✖ {}: {e}", out.display())),
                    JobEvent::AllDone => job.running = false,
                }
            }
        }
        for out in finished { self.add_output(&out); }
    }

    /// Put a finished .fao into the collection: into the source it lives under
    /// if there is one, otherwise by adding its folder as a new source.
    fn add_output(&mut self, out: &std::path::Path) {
        let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
        for (i, s) in self.sources.iter().enumerate() {
            if let Source::Local(root) = s {
                if let Ok(rel) = out.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !self.entries.iter().any(|e| e.source == i && e.rel == rel) {
                        self.entries.push(Entry { source: i, rel, size, kind: Kind::Fao });
                        self.entries.sort_by(|a, b| (a.source, &a.rel).cmp(&(b.source, &b.rel)));
                    }
                    return;
                }
            }
        }
        if let Some(dir) = out.parent() { self.add(Source::Local(dir.to_path_buf())); }
    }

    pub fn cancel_encode(&mut self) {
        if let Some(j) = &self.job { j.cancel.store(true, Ordering::Relaxed); }
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let st = &mut app.media;
    st.poll();
    st.poll_encode();
    st.poll_analyze();
    if st.analyzing.is_some() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(150)); }
    if st.job.as_ref().is_some_and(|j| j.running) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    if st.pending > 0 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }

    ui.heading("Media");
    ui.label(egui::RichText::new(
        "Video files from folders and dufs servers. (The Library tab is the encoder's mesh catalog — a different thing.)")
        .weak().small());
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Folder");
        ui.add(egui::TextEdit::singleline(&mut st.new_folder).id_salt("media-new_folder").hint_text("/home/you/Videos/matches").desired_width(320.0));
        if ui.button("+ Add folder").clicked() && !st.new_folder.trim().is_empty() {
            let p = std::path::PathBuf::from(st.new_folder.trim());
            st.add(Source::Local(p));
            st.new_folder.clear();
        }
    });
    ui.horizontal(|ui| {
        ui.label("dufs");
        ui.add(egui::TextEdit::singleline(&mut st.new_server).id_salt("media-new_server").hint_text("http://192.168.1.20:5000/matches").desired_width(320.0));
        if ui.button("+ Add server").clicked() && !st.new_server.trim().is_empty() {
            st.add(Source::Dufs(st.new_server.trim().to_string()));
            st.new_server.clear();
        }
    });

    let mut remove = None;
    for (i, s) in st.sources.iter().enumerate() {
        ui.horizontal(|ui| {
            let n = st.entries.iter().filter(|e| e.source == i).count();
            let tag = match s { Source::Local(_) => "📁", Source::Dufs(_) => "🌐" };
            ui.label(format!("{tag} {}  ({n} files)", s.label()));
            if ui.small_button("✖").on_hover_text("Remove this source").clicked() { remove = Some(i); }
        });
    }
    if let Some(i) = remove { st.remove(i); }
    if st.pending > 0 { ui.label(format!("scanning {} source(s)…", st.pending)); }

    ui.separator();
    ui.columns(2, |cols| {
        cols[0].label("Include (one pattern per line)");
        cols[0].add(egui::TextEdit::multiline(&mut st.include_text).id_salt("media-include").hint_text("*.fao\nranked/**/*.mkv\n2026-09-*").desired_rows(3));
        cols[1].label("Exclude");
        cols[1].add(egui::TextEdit::multiline(&mut st.exclude_text).id_salt("media-exclude").hint_text("**/scrims/*").desired_rows(3));
    });
    st.sync_patterns();

    let (all_n, all_b) = media::totals(st.entries.iter());
    let sel: Vec<&Entry> = st.entries.iter().filter(|e| st.selection.selected(e)).collect();
    let (sel_n, sel_b) = media::totals(sel.iter().copied());
    ui.horizontal(|ui| {
        ui.label(format!("{sel_n} of {all_n} selected · {} of {}", gb(sel_b), gb(all_b)));
        ui.checkbox(&mut st.only_selected, "show selected only");
    });

    // ── analyze ──
    let n_src = st.selected_sources().len();
    ui.horizontal(|ui| {
        let busy = st.analyzing.is_some();
        let label = format!("🔍 Estimate shared-library savings ({n_src} selected)");
        if ui.add_enabled(!busy && n_src > 0, egui::Button::new(label))
            .on_hover_text("Samples a few frames from each file and measures how many 16x16 blocks \
                            a single shared block dictionary would store once instead of per file.")
            .clicked() { st.start_analyze(); }
        ui.add(egui::Slider::new(&mut st.samples, 2..=48).text("frames / file"));
        if busy { ui.label(format!("sampling {} of {}…", st.analyze_progress.0, st.analyze_progress.1)); }
    });
    match &st.analysis {
        Some(Ok(r)) => {
            let mb = |b: f64| b / (1024.0 * 1024.0);
            ui.group(|ui| {
                ui.label(egui::RichText::new(format!(
                    "Shared dictionary saves {:.0}% of block storage", r.savings_ratio() * 100.0)).strong());
                ui.label(format!("One dictionary per file: {} unique blocks ≈ {:.2} MB",
                                 r.per_file_unique, mb(r.per_file_bytes())));
                ui.label(format!("One shared dictionary:   {} unique blocks ≈ {:.2} MB",
                                 r.shared_unique, mb(r.shared_bytes())));
                ui.label(format!("Blocks repeat {:.1}× inside a file on average; {:.0} bytes per stored block (measured, zstd)",
                                 r.within_file_reuse(), r.bytes_per_block));
                if r.shared_overflows() {
                    ui.colored_label(ui.visuals().warn_fg_color, format!(
                        "⚠ Even this sample exceeds the dictionary's {} block cap. At the cap the encoder \
                         skips dictionary frames entirely — split the selection into groups.",
                        crate::analyze::MAX_BLOCKS));
                }
                ui.label(egui::RichText::new(format!(
                    "Estimate from {} frames across {} files. Unique counts from a sample are a lower \
                     bound; the percentage is the reliable figure.", r.frames, r.files)).weak().small());
                if r.missed > 0 {
                    ui.label(egui::RichText::new(format!(
                        "{} requested frame(s) could not be sampled, so the estimate rests on fewer.", r.missed))
                        .weak().small());
                }
                egui::CollapsingHeader::new("per file").id_salt("analyze_files").show(ui, |ui| {
                    for l in &r.lines {
                        ui.label(egui::RichText::new(format!("{}  —  {} unique, {} shared with others",
                                                             l.name, l.unique, l.shared_with_others)).small());
                    }
                });
            });
        }
        Some(Err(e)) => { ui.colored_label(ui.visuals().error_fg_color, e); }
        None => {}
    }

    // ── encode ──
    ui.horizontal(|ui| {
        ui.label("Output");
        ui.add(egui::TextEdit::singleline(&mut st.output_dir).id_salt("media-output_dir")
            .hint_text("beside each local file (required for server files)").desired_width(280.0));
        ui.add(egui::Slider::new(&mut st.quality, 10..=100).text("quality"));
    });
    let running = st.job.as_ref().is_some_and(|j| j.running);
    let (plan, skipped) = st.plan_encode();
    ui.horizontal(|ui| {
        let label = format!("⚙ Encode {} selected video(s) to .fao", plan.len());
        if ui.add_enabled(!running && !plan.is_empty(), egui::Button::new(label)).clicked() {
            st.start_encode(plan.clone());
        }
        if running && ui.button("■ Stop").clicked() { st.cancel_encode(); }
        if !skipped.is_empty() {
            ui.label(egui::RichText::new(format!("{} skipped", skipped.len())).weak())
                .on_hover_text(skipped.join("\n"));
        }
    });
    if let Some(job) = &st.job {
        if job.running {
            let (d, t) = job.frames;
            ui.label(format!("{} of {}: {}", job.index + 1, job.total, job.current));
            ui.add(egui::ProgressBar::new(if t > 0 { d as f32 / t as f32 } else { 0.0 })
                .text(format!("frame {d} of ~{t}")));
        }
        for line in job.log.iter().rev().take(6) { ui.label(egui::RichText::new(line).small()); }
    }

    if !st.errors.is_empty() {
        egui::CollapsingHeader::new(format!("⚠ {} folder(s) could not be read", st.errors.len()))
            .id_salt("media_errors").show(ui, |ui| {
                for (w, e) in st.errors.iter().take(50) {
                    ui.label(egui::RichText::new(format!("{w}: {e}")).small());
                }
            });
    }

    let mut toggle: Option<Entry> = None;
    let mut play: Option<std::path::PathBuf> = None;
    egui::ScrollArea::vertical().id_salt("media_list").auto_shrink([false, false]).show(ui, |ui| {
        for e in &st.entries {
            let on = st.selection.selected(e);
            if st.only_selected && !on { continue; }
            ui.horizontal(|ui| {
                let mut c = on;
                if ui.checkbox(&mut c, "").changed() { toggle = Some(e.clone()); }
                let icon = match e.kind { Kind::Fao => "▶", Kind::SourceVideo => "🎬" };
                ui.label(format!("{icon} {}", e.rel));
                ui.label(egui::RichText::new(gb(e.size)).weak().small());
                if e.kind == Kind::Fao {
                    if let Some(p) = st.local_path(e) {
                        if ui.small_button("Play").clicked() { play = Some(p); }
                    }
                }
            });
        }
    });
    if let Some(e) = toggle { st.selection.toggle(&e); }
    if let Some(p) = play {
        app.player.a.path = p.display().to_string();
        app.player.a.open();
        app.tab = crate::Tab::Player;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AGAINST A REAL dufs, not the fake. Ignored by default because it needs
    /// the binary: `DUFS_BIN=/path/to/dufs cargo test -- --ignored real_dufs`.
    ///
    /// Covers what only a real server can: the actual JSON (a Dir's `size` is
    /// its ENTRY COUNT, not bytes -- found here, invisible in the fake), a
    /// percent-encoded listing URL for a folder with a space, an emoji file name,
    /// hidden folders, and a byte-range request -- the assumption remote
    /// streaming will rest on.
    #[test]
    #[ignore = "needs a dufs binary: DUFS_BIN=/path/to/dufs cargo test -- --ignored real_dufs"]
    fn real_dufs_is_walked_and_serves_ranges() {
        let bin = std::env::var("DUFS_BIN").expect("set DUFS_BIN");
        let root = std::env::temp_dir().join(format!("real_dufs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // `cup #2`: a folder whose name has a URL-significant character.
        for d in ["ranked/2026", "season 2", "cup #2", ".cache"] { std::fs::create_dir_all(root.join(d)).unwrap(); }
        std::fs::write(root.join("match 1.mkv"), b"0123456789").unwrap();
        std::fs::write(root.join("ranked/final.mp4"), b"a").unwrap();
        std::fs::write(root.join("ranked/2026/deep.fao"), b"b").unwrap();
        std::fs::write(root.join("season 2/g.mp4"), b"c").unwrap();
        std::fs::write(root.join("\u{1F600} clip.webm"), b"d").unwrap();
        std::fs::write(root.join(".cache/hidden.mkv"), b"e").unwrap();
        std::fs::write(root.join("notes.txt"), b"f").unwrap();
        // THE NAMES THAT ACTUALLY NEED `encode_path`. Spaces and emoji do not --
        // ureq's URL parser encodes those itself, which is why a fixture of only
        // those passed with encoding switched OFF. But '#' starts a fragment, '?'
        // a query and a bare '%' is mis-decoded, and ureq leaves all three alone:
        // raw, `match #3.mkv` asks the server for "match ".
        std::fs::write(root.join("match #3.mkv"), b"hash").unwrap();
        std::fs::write(root.join("100% win.mp4"), b"pct").unwrap();
        std::fs::write(root.join("cup #2/r?.mkv"), b"q").unwrap();

        // A port from the pid, so parallel runs do not collide.
        let port = 20_000 + (std::process::id() % 20_000) as u16;
        let mut child = std::process::Command::new(&bin)
            .arg(&root).args(["-p", &port.to_string(), "-b", "127.0.0.1"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().expect("dufs starts");
        let base = format!("http://127.0.0.1:{port}");
        // Wait for it to listen rather than sleeping a guessed amount.
        let up = (0..50).any(|_| { std::thread::sleep(std::time::Duration::from_millis(100));
                                    http_fetch(&format!("{base}/?json")).is_ok() });
        let result = std::panic::catch_unwind(|| {
            assert!(up, "dufs never answered");
            let scan = media::scan_dufs(&base, 0, 100, &mut |u| http_fetch(u));
            let rels: Vec<_> = scan.entries.iter().map(|e| e.rel.clone()).collect();
            assert_eq!(rels, vec!["100% win.mp4", "cup #2/r?.mkv", "match #3.mkv", "match 1.mkv",
                                  "ranked/2026/deep.fao", "ranked/final.mp4",
                                  "season 2/g.mp4", "\u{1F600} clip.webm"], "{:?}", scan.errors);
            assert!(scan.errors.is_empty(), "{:?}", scan.errors);
            // By NAME, not index: adding fixture files that sort first broke an
            // `entries[0]` version of this -- "100% win.mp4" moved ahead.
            let m1 = scan.entries.iter().find(|e| e.rel == "match 1.mkv").unwrap();
            assert_eq!(m1.size, 10, "file size is bytes");
            // A byte range, as the remote stream reader will ask for.
            let url = media::file_url(&base, "match 1.mkv");
            let r = ureq::get(&url).set("Range", "bytes=2-5").call().expect("range");
            assert_eq!(r.status(), 206);
            assert_eq!(r.header("content-range"), Some("bytes 2-5/10"));
            assert_eq!(r.into_string().unwrap(), "2345");
            // And a range on the file whose name has '#': raw, this would
            // fetch "match " and fail.
            let h = ureq::get(&media::file_url(&base, "match #3.mkv"))
                .set("Range", "bytes=0-3").call().expect("range on a '#' name");
            assert_eq!(h.into_string().unwrap(), "hash");
        });
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mp_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Only selected SOURCE videos are planned; an existing .fao is never
    /// overwritten; a server file needs a local output folder.
    #[test]
    fn the_encode_plan_follows_the_rules() {
        let root = tmpdir("plan");
        std::fs::write(root.join("done.fao"), b"x").unwrap();
        let mut st = State::default();
        st.sources = vec![Source::Local(root.clone()), Source::Dufs("http://nas:5000".into())];
        st.entries = vec![
            Entry { source: 0, rel: "a.mkv".into(), size: 1, kind: Kind::SourceVideo },
            Entry { source: 0, rel: "done.mp4".into(), size: 1, kind: Kind::SourceVideo },
            Entry { source: 0, rel: "b.fao".into(), size: 1, kind: Kind::Fao },
            Entry { source: 1, rel: "remote.mp4".into(), size: 1, kind: Kind::SourceVideo },
            Entry { source: 0, rel: "unselected.mkv".into(), size: 1, kind: Kind::SourceVideo },
        ];
        st.include_text = "*".into();
        st.exclude_text = "unselected*".into();
        st.sync_patterns();
        let (items, skipped) = st.plan_encode();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["a.mkv"], "{skipped:?}");
        assert_eq!(items[0].output, root.join("a.fao"), "beside the source by default");
        assert!(skipped.iter().any(|s| s.contains("done.mp4") && s.contains("already exists")));
        assert!(skipped.iter().any(|s| s.contains("remote.mp4") && s.contains("output folder")));

        // With an output folder, the server file is planned -- read over HTTP.
        let out = tmpdir("plan_out");
        st.output_dir = out.display().to_string();
        let (items, _) = st.plan_encode();
        let remote = items.iter().find(|i| i.name == "remote.mp4").expect("planned now");
        assert_eq!(remote.input, "http://nas:5000/remote.mp4");
        assert_eq!(remote.output, out.join("remote.fao"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    /// END TO END: a folder of real video -> Encode -> a .fao that decodes, in
    /// the collection, ready to Play.
    #[test]
    fn encoding_a_real_video_puts_a_playable_fao_in_the_collection() {
        let ok = std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok { eprintln!("no ffmpeg; skipping"); return; }
        let root = tmpdir("e2e");
        let made = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i",
            "testsrc=size=64x48:rate=30:duration=0.3", "-pix_fmt", "yuv420p"])
            .arg(root.join("match.mp4")).status().unwrap().success();
        assert!(made);
        let mut st = State::default();
        st.add(Source::Local(root.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while st.pending > 0 && std::time::Instant::now() < deadline { st.poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
        st.include_text = "*.mp4".into();
        st.sync_patterns();
        let (plan, _) = st.plan_encode();
        assert_eq!(plan.len(), 1);
        st.start_encode(plan);
        while st.job.as_ref().is_some_and(|j| j.running) && std::time::Instant::now() < deadline {
            st.poll_encode();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        st.poll_encode();
        let job = st.job.as_ref().unwrap();
        assert!(!job.running, "encode never finished");
        assert_eq!(job.done, 1, "{:?}", job.log);
        let out = root.join("match.fao");
        assert!(out.exists());
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((f.width(), f.height(), f.frame_count()), (64, 48, 9));
        assert!(st.entries.iter().any(|e| e.rel == "match.fao" && e.kind == Kind::Fao),
                "the result is in the collection, ready to Play");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// THE NETWORKED PATH, end to end: a real dufs serves a real video, and the
    /// Media tab encodes it -- ffprobe and ffmpeg reading the HTTP URL through
    /// the range requests dufs serves -- into a local .fao.
    #[test]
    #[ignore = "needs a dufs binary: DUFS_BIN=/path/to/dufs cargo test -- --ignored real_dufs"]
    fn real_dufs_video_encodes_over_http() {
        let bin = std::env::var("DUFS_BIN").expect("set DUFS_BIN");
        let served = tmpdir("dufs_enc_src");
        let made = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i",
            "testsrc=size=64x48:rate=30:duration=0.3", "-pix_fmt", "yuv420p"])
            .arg(served.join("cup #2 final.mp4")).status().unwrap().success();
        assert!(made);
        let out = tmpdir("dufs_enc_out");
        let port = 30_000 + (std::process::id() % 20_000) as u16;
        let mut child = std::process::Command::new(&bin).arg(&served)
            .args(["-p", &port.to_string(), "-b", "127.0.0.1"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let base = format!("http://127.0.0.1:{port}");
        let up = (0..50).any(|_| { std::thread::sleep(std::time::Duration::from_millis(100));
                                    http_fetch(&format!("{base}/?json")).is_ok() });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert!(up, "dufs never answered");
            let mut st = State::default();
            st.add(Source::Dufs(base.clone()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while st.pending > 0 && std::time::Instant::now() < deadline { st.poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
            assert_eq!(st.entries.len(), 1, "{:?}", st.errors);
            st.include_text = "*.mp4".into();
            st.output_dir = out.display().to_string();
            st.sync_patterns();
            let (plan, skipped) = st.plan_encode();
            assert_eq!(plan.len(), 1, "{skipped:?}");
            // '#' in the name: raw, ffmpeg would ask for "cup " -- encode_path's job.
            assert!(plan[0].input.contains("%23"), "{}", plan[0].input);
            st.start_encode(plan);
            while st.job.as_ref().is_some_and(|j| j.running) && std::time::Instant::now() < deadline {
                st.poll_encode(); std::thread::sleep(std::time::Duration::from_millis(20));
            }
            st.poll_encode();
            let job = st.job.as_ref().unwrap();
            assert_eq!(job.done, 1, "{:?}", job.log);
            let f = crate::decode::FaoFile::parse(std::fs::read(out.join("cup #2 final.fao")).unwrap()).unwrap();
            assert_eq!((f.width(), f.height(), f.frame_count()), (64, 48, 9));
        }));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&served);
        let _ = std::fs::remove_dir_all(&out);
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }

    /// END TO END through the tab: two real clips sharing a HUD, selected,
    /// analysed on the thread, reporting savings. And timed, since "fast" was
    /// the requirement.
    #[test]
    fn analyze_from_the_tab_reports_savings_for_clips_sharing_a_hud() {
        let ok = std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok { eprintln!("no ffmpeg; skipping"); return; }
        let root = tmpdir("analyze_tab");
        for (name, src) in [("a.mp4", "testsrc=size=256x192:rate=10:duration=2"),
                            ("b.mp4", "testsrc2=size=256x192:rate=10:duration=2")] {
            assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", src,
                "-vf", "drawbox=x=0:y=0:w=iw:h=64:color=0x2040a0:t=fill", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-qp", "0"])
                .arg(root.join(name)).status().unwrap().success());
        }
        let mut st = State::default();
        st.add(Source::Local(root.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while st.pending > 0 && std::time::Instant::now() < deadline { st.poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
        st.include_text = "*.mp4".into();
        st.sync_patterns();
        st.samples = 4;
        let t = std::time::Instant::now();
        st.start_analyze();
        while st.analyzing.is_some() && std::time::Instant::now() < deadline {
            st.poll_analyze(); std::thread::sleep(std::time::Duration::from_millis(10));
        }
        eprintln!("analyzed 2 files x 4 frames in {:?}", t.elapsed());
        let r = st.analysis.clone().expect("finished").expect("ok");
        assert_eq!(r.files, 2);
        assert!(r.savings_ratio() > 0.0, "{r:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn patterns_split_by_line_skipping_blanks_and_comments() {
        assert_eq!(patterns("*.fao\n\n  ranked/*  \n# old\n2026-*"),
                   vec!["*.fao", "ranked/*", "2026-*"]);
    }

    /// Removing a source must drop its files AND shift the others' indices, or
    /// every later source's files would be attributed to the wrong folder.
    #[test]
    fn removing_a_source_reindexes_the_rest() {
        let mut st = State::default();
        st.sources = vec![Source::Local("/a".into()), Source::Local("/b".into()), Source::Local("/c".into())];
        st.entries = vec![
            Entry { source: 0, rel: "x.mkv".into(), size: 1, kind: Kind::SourceVideo },
            Entry { source: 1, rel: "y.mkv".into(), size: 1, kind: Kind::SourceVideo },
            Entry { source: 2, rel: "z.fao".into(), size: 1, kind: Kind::Fao },
        ];
        st.selection.on.insert((2, "z.fao".into()));
        st.remove(1);
        assert_eq!(st.sources.len(), 2);
        let pairs: Vec<_> = st.entries.iter().map(|e| (e.source, e.rel.as_str())).collect();
        assert_eq!(pairs, vec![(0, "x.mkv"), (1, "z.fao")]);
        assert!(st.selection.on.contains(&(1, "z.fao".to_string())), "selection follows the reindex");
        assert_eq!(st.local_path(&st.entries[1]), Some(std::path::PathBuf::from("/c/z.fao")));
    }

    #[test]
    fn adding_the_same_source_twice_is_a_no_op() {
        let mut st = State::default();
        let dir = std::env::temp_dir();
        st.add(Source::Local(dir.clone()));
        st.add(Source::Local(dir));
        assert_eq!(st.sources.len(), 1);
    }

    /// End to end on a real folder, through the background thread.
    #[test]
    fn a_folder_added_in_the_ui_arrives_via_the_scan_thread() {
        let root = std::env::temp_dir().join(format!("media_page_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ranked")).unwrap();
        std::fs::write(root.join("ranked/m.fao"), b"1").unwrap();
        std::fs::write(root.join("n.mkv"), b"12").unwrap();
        let mut st = State::default();
        st.add(Source::Local(root.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while st.pending > 0 && std::time::Instant::now() < deadline {
            st.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(st.pending, 0, "scan never came back");
        assert_eq!(st.entries.len(), 2);
        st.include_text = "*.fao".into();
        st.sync_patterns();
        let sel: Vec<_> = st.entries.iter().filter(|e| st.selection.selected(e)).map(|e| e.rel.clone()).collect();
        assert_eq!(sel, vec!["ranked/m.fao"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
