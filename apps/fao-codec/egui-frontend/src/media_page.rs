//! media_page — the Media tab: the egui shell over `media`.
//!
//! Add local folders and dufs servers, pick files by pattern or one at a time,
//! and open a `.fao` straight into the player. All the logic lives in `media`
//! and is tested there; this file is layout and wiring.

use crate::media::{self, Entry, Kind, Scan, Selection, Source};
use std::sync::mpsc;

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
}

impl Default for State {
    fn default() -> Self {
        State {
            sources: Vec::new(), entries: Vec::new(), errors: Vec::new(),
            selection: Selection::default(), new_folder: String::new(), new_server: String::new(),
            include_text: String::new(), exclude_text: String::new(), only_selected: false,
            rx: None, tx: None, pending: 0,
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

fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let st = &mut app.media;
    st.poll();
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
        ui.add(egui::TextEdit::singleline(&mut st.new_folder).hint_text("/home/you/Videos/matches").desired_width(320.0));
        if ui.button("+ Add folder").clicked() && !st.new_folder.trim().is_empty() {
            let p = std::path::PathBuf::from(st.new_folder.trim());
            st.add(Source::Local(p));
            st.new_folder.clear();
        }
    });
    ui.horizontal(|ui| {
        ui.label("dufs");
        ui.add(egui::TextEdit::singleline(&mut st.new_server).hint_text("http://192.168.1.20:5000/matches").desired_width(320.0));
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
        cols[0].add(egui::TextEdit::multiline(&mut st.include_text).hint_text("*.fao\nranked/**/*.mkv\n2026-09-*").desired_rows(3));
        cols[1].label("Exclude");
        cols[1].add(egui::TextEdit::multiline(&mut st.exclude_text).hint_text("**/scrims/*").desired_rows(3));
    });
    st.sync_patterns();

    let (all_n, all_b) = media::totals(st.entries.iter());
    let sel: Vec<&Entry> = st.entries.iter().filter(|e| st.selection.selected(e)).collect();
    let (sel_n, sel_b) = media::totals(sel.iter().copied());
    ui.horizontal(|ui| {
        ui.label(format!("{sel_n} of {all_n} selected · {} of {}", gb(sel_b), gb(all_b)));
        ui.checkbox(&mut st.only_selected, "show selected only");
    });

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
