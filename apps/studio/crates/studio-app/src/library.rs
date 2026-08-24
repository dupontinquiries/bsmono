//! library — the sound-library sidebar.
//!
//! A folder of samples IS the instrument for a sample-based workflow; making
//! the app the place you browse it removes the file-manager round trip that
//! drag-and-drop still requires. The sidebar lists what `STUDIO_LIBRARY`
//! (default `./library`) holds and offers each file the same two doors a
//! drop already has: a rack sampler channel (a drum hit) or a timeline clip
//! at the playhead (a melody loop). One scan function, pure and tested; the
//! egui around it stays dumb.
//!
//! Audition-on-click hears a sample before committing it: the sink grew
//! additive one-shot voices for played notes, and previewing a file is the
//! same operation, so the sidebar's ▶ button uses it rather than hijacking
//! the loop buffer the way a lesser shortcut would.

use std::path::{Path, PathBuf};

/// Extensions studio-media can decode. The sniffing decoder would accept
/// more, but a browser that lists files it cannot open teaches mistrust.
pub const AUDIO_EXTS: &[&str] = &["opus", "wav", "flac", "ogg", "mp3", "m4a", "aac"];

/// One listed sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Shown name: path relative to the library root, so `kicks/808.opus`
    /// reads as the folder structure the user built.
    pub label: String,
    pub path: PathBuf,
    /// Bytes, from the same `read_dir` pass that found the file — the
    /// weight heuristic's only input besides the extension, because
    /// probing 23,000 files for a real bitrate is not a browse, it is a
    /// batch job.
    pub bytes: u64,
}

/// One level of the library: subdirectories and audio files, each sorted.
/// The tree is scanned LAZILY, one directory as it is opened — a 23,000-file
/// library (a real one) must cost nothing until browsed, and a mistyped
/// `STUDIO_LIBRARY=/` must never hang the app.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirListing {
    pub dirs: Vec<(String, PathBuf)>,
    pub files: Vec<LibraryEntry>,
}

fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| e.eq_ignore_ascii_case(a)))
        .unwrap_or(false)
}
fn hidden(name: &str) -> bool { name.starts_with('.') }

/// One directory, one syscall pass.
pub fn scan_dir(dir: &Path) -> DirListing {
    let mut out = DirListing::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if hidden(&name) { continue; }
        if p.is_dir() {
            out.dirs.push((name, p));
        } else if is_audio(&p) {
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.files.push(LibraryEntry { label: name, path: p, bytes });
        }
    }
    out.dirs.sort();
    out.files.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Case-insensitive substring search over the whole tree, bounded twice:
/// depth (a sample library is deep, a filesystem loop is deeper) and result
/// count (500 hits is a browse, 5,000 is a wall). Deterministic order.
pub fn search(root: &Path, needle: &str, cap: usize) -> Vec<LibraryEntry> {
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 10 || out.len() >= cap { break; }
        let listing = scan_dir(&dir);
        for f in listing.files {
            if f.label.to_lowercase().contains(&needle) {
                let label = f.path.strip_prefix(root).unwrap_or(&f.path)
                    .display().to_string();
                out.push(LibraryEntry { label, path: f.path, bytes: f.bytes });
                if out.len() >= cap { break; }
            }
        }
        // Reverse so the stack pops in sorted order — the result list must
        // not depend on OS readdir whims.
        for (_, d) in listing.dirs.into_iter().rev() {
            stack.push((d, depth + 1));
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Other encodings of THIS sample: same stem, different audio extension, in
/// the same directory or an immediate subdirectory — exactly the layout a
/// batch converter leaves (`X.wav` beside `FFaudioConverter/X.opus` and
/// `FFaudioConverterFLAC/X.flac`). Lossless first, so "the better file" is
/// always the first offer.
pub fn variants(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { return vec![] };
    const PREF: [&str; 7] = ["wav", "flac", "aiff", "opus", "ogg", "m4a", "mp3"];
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() { dirs.push(e.path()); }
        }
    }
    for d in dirs {
        for ext in PREF {
            let cand = d.join(format!("{}.{ext}", stem.to_string_lossy()));
            if cand != *path && cand.is_file() && !found.contains(&cand) {
                found.push(cand);
            }
        }
    }
    found.sort_by_key(|p| {
        let e = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        PREF.iter().position(|x| *x == e).unwrap_or(PREF.len())
    });
    found
}

/// What a sample IS, guessed from its name — the taxonomy real pack folders
/// already use, so the guess is usually just reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    Bass808, Kick, Snare, Clap, Hat, Cymbal, Perc, Bell,
    Vox, Melody, Loop, Fx, Unknown,
}

impl SampleKind {
    pub fn is_drum(self) -> bool {
        matches!(self, SampleKind::Bass808 | SampleKind::Kick | SampleKind::Snare
                 | SampleKind::Clap | SampleKind::Hat | SampleKind::Cymbal
                 | SampleKind::Perc)
    }
}

/// Token order is the whole algorithm, and every ordering below is a real
/// filename from a real pack: "808 1 (LAYER WITH KICK 1)" is an 808 before
/// it is a kick; "Vox_Loop_10" is a vocal before it is a loop; "Hi-Hat 6
/// (Water...)" is a hat. A wrong guess costs one wrong SUGGESTION, never a
/// wrong action — suggestions are offered, not taken.
pub fn classify(name: &str) -> SampleKind {
    let n = name.to_lowercase();
    let has = |t: &str| n.contains(t);
    if has("808") { return SampleKind::Bass808; }
    if has("vox") || has("vocal") || has("chant") || has("acapella") { return SampleKind::Vox; }
    if has("kick") { return SampleKind::Kick; }
    if has("snare") || has("snap") || has("rim") { return SampleKind::Snare; }
    if has("clap") { return SampleKind::Clap; }
    if has("hat") { return SampleKind::Hat; }
    if has("crash") || has("cymbal") || has("ride") { return SampleKind::Cymbal; }
    if has("perc") || has("shaker") || has("tom") || has("conga") || has("bongo")
        || has("tabla") || has("triangle") || has("cowbell") { return SampleKind::Perc; }
    if has("bell") { return SampleKind::Bell; }
    if has("melody") || has("chord") || has("keys") || has("piano") { return SampleKind::Melody; }
    if has("loop") || has("bpm") { return SampleKind::Loop; }
    if has("fx") || has("riser") || has("rise") || has("impact") || has("sweep")
        || has("stomp") { return SampleKind::Fx; }
    SampleKind::Unknown
}

/// One smart offer: what the ⋯ menu shows, and what the shell does when it
/// is chosen. Data, not behavior — pure and testable, the grid's contract.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub act: SuggestAct,
}
#[derive(Debug, Clone, PartialEq)]
pub enum SuggestAct {
    /// Swap this sample into rack channel `i`, keeping its steps and rolls
    /// — "try this snare in the pattern I already have".
    ReplaceChannel(usize),
    NewChannel,
    ClipAtPlayhead,
    /// A clip on a new audio track routed to `Some(bus)`, or to a NEW
    /// "Drums" bus when `None` — the smart bus choice for drum material.
    ClipToBus(Option<studio_core::BusId>),
}

/// The offers for one sample, best first. Replacement targets come from the
/// rack's own channel names run through the same classifier — a sample
/// library and a rack that both say "snare" mean the same thing.
pub fn suggest(kind: SampleKind, rack: &studio_live::Rack,
               project: &studio_core::Project) -> Vec<Suggestion> {
    let mut out = Vec::new();
    for (i, ch) in rack.channels.iter().enumerate() {
        if kind != SampleKind::Unknown && classify(&ch.name) == kind {
            out.push(Suggestion {
                label: format!("replace channel '{}'", ch.name),
                act: SuggestAct::ReplaceChannel(i),
            });
        }
    }
    out.push(Suggestion { label: "new rack channel".into(), act: SuggestAct::NewChannel });
    out.push(Suggestion { label: "clip at the playhead".into(),
                          act: SuggestAct::ClipAtPlayhead });
    if kind.is_drum() {
        let drum_bus = project.busses.iter().find(|(_, b)| {
            let n = b.name.to_lowercase();
            n.contains("drum") || n.contains("beat") || n.contains("perc")
        });
        match drum_bus {
            Some((id, b)) => out.push(Suggestion {
                label: format!("clip on a new track → bus '{}'", b.name),
                act: SuggestAct::ClipToBus(Some(*id)),
            }),
            None => out.push(Suggestion {
                label: "clip on a new track → new 'Drums' bus".into(),
                act: SuggestAct::ClipToBus(None),
            }),
        }
    }
    out
}

/// This sample's same-kind neighbours in ITS OWN directory — the pack's
/// natural family ("Snare 1" beside "Snare 14"), up to `cap`, converter
/// subdirs excluded because a format-variant of the SAME sound is not a
/// neighbour. Feeds the pad-loadout suggestion: proposed, never loaded on
/// the pad's own authority.
pub fn similar_in_dir(path: &Path, cap: usize) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { return vec![] };
    let kind = classify(&stem.to_string_lossy());
    if kind == SampleKind::Unknown { return vec![]; }
    scan_dir(dir).files.into_iter()
        .filter(|e| e.path != *path)
        .filter(|e| e.path.file_stem() != Some(stem)) // a variant is not a neighbour
        .filter(|e| classify(&e.label) == kind)
        .map(|e| e.path)
        .take(cap)
        .collect()
}

// ── narrow-display presentation ─────────────────────────────────────────

/// The identicon's text: first four and last four characters of the stem,
/// elided in the middle, plus the extension — so
/// `Vox_Dry_Waduwap_high_A#.opus` reads as `Vox_…h_A#` on a phone where
/// the full name would push every button off the screen. Short stems are
/// shown whole rather than padded: eliding a six-character name to save
/// two is theatre.
pub fn identicon(label: &str) -> (String, String) {
    let path = Path::new(label);
    let stem: String = path.file_stem().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let chars: Vec<char> = stem.chars().collect();
    let short = if chars.len() <= 9 {
        stem
    } else {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    };
    (short, ext)
}

/// Bytes per second above which a file is "heavy" for a phone-first
/// library. Sits deliberately between the two cases this was specified
/// against: a 219 kb/s Opus (~27 kB/s) is light; a 6144 kb/s 96 kHz float
/// WAV (~768 kB/s) is heavy. 96 kB/s ≈ 768 kb/s — comfortably above any
/// transparent lossy file, comfortably below any lossless one.
pub const HEAVY_BYTES_PER_SEC: u64 = 96_000;

/// A rough duration for a sample file, from bytes and format — enough to
/// turn a size into a rate. Deliberately crude: this decides whether to
/// draw a small icon, not what to encode.
fn assumed_bytes_per_sec(ext: &str, bytes: u64) -> u64 {
    match ext {
        // Uncompressed and legacy formats are heavy by KIND, and their
        // rate is knowable without opening them: 16-bit stereo 48k is
        // ~192 kB/s, and anything float or 96 kHz is multiples of that.
        "wav" | "aiff" | "aif" => 192_000,
        "mp3" => 40_000, // not heavy by rate — heavy by being mp3, below
        // Compressed: a rate cannot be inferred from size without a
        // duration, so size stands in. A big FLAC is a big file whatever
        // its rate; a 200 kB opus loop is not.
        _ => {
            if bytes > 12_000_000 { HEAVY_BYTES_PER_SEC + 1 } else { 1 }
        }
    }
}

/// Should this file wear the anvil? Heavy means "this costs real storage
/// and real decode on a Fairphone": uncompressed by kind (`wav`, `aiff`),
/// legacy (`mp3` — kept in the pack for compatibility, never the file you
/// want), or simply large. Everything transparent-and-small — an Opus
/// one-shot, a short FLAC — stays unmarked, which is the point: the mark
/// has to be rare to mean anything.
pub fn is_heavy(path: &Path, bytes: u64) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "wav" | "aiff" | "aif" | "mp3") { return true; }
    assumed_bytes_per_sec(&ext, bytes) > HEAVY_BYTES_PER_SEC
}

/// Per-project sample preferences: what floats to the top, what sinks.
/// Swiped, not clicked (see [`Swipe`]) — the gesture is the point, since
/// this is triage done at speed while listening, not configuration.
///
/// These live in the project's workspace sidecar, NOT in the project's op
/// log: a pin is how one person keeps their head straight in one session,
/// not a fact about the music, and pushing it over the wire would mean a
/// collaborator's triage rearranges yours mid-take. Said plainly rather
/// than discovered: your pins are yours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prefs {
    pub pinned: std::collections::BTreeSet<PathBuf>,
    pub demoted: std::collections::BTreeSet<PathBuf>,
}

impl Prefs {
    /// Pin and demote are opposites, so setting one clears the other —
    /// a sample cannot be both a favourite and buried, and making the
    /// user un-demote before pinning would be pure ceremony.
    pub fn pin(&mut self, p: &Path) {
        self.demoted.remove(p);
        if !self.pinned.insert(p.to_path_buf()) { self.pinned.remove(p); }
    }
    pub fn demote(&mut self, p: &Path) {
        self.pinned.remove(p);
        if !self.demoted.insert(p.to_path_buf()) { self.demoted.remove(p); }
    }
    pub fn rank(&self, p: &Path) -> i8 {
        if self.pinned.contains(p) { -1 }
        else if self.demoted.contains(p) { 1 }
        else { 0 }
    }
}

/// Order a listing by preference: pinned first, demoted last, everything
/// else in the order it arrived (alphabetical from the scan). Stable, so
/// a pin moves ONE row and nothing else jumps.
pub fn arrange(entries: &[LibraryEntry], prefs: &Prefs) -> Vec<LibraryEntry> {
    let mut out = entries.to_vec();
    out.sort_by_key(|e| prefs.rank(&e.path));
    out
}

/// Group a flat result list under folder headers, so a search across a
/// deep library shows `zaytoven/808s & Kicks` ONCE with bare filenames
/// under it, instead of repeating the folder on every row and forcing the
/// sidebar wide enough for the longest path in the pack.
pub fn sections(entries: &[LibraryEntry]) -> Vec<(String, Vec<LibraryEntry>)> {
    let mut out: Vec<(String, Vec<LibraryEntry>)> = Vec::new();
    for e in entries {
        let (dir, file) = match e.label.rsplit_once('/') {
            Some((d, f)) => (d.to_string(), f.to_string()),
            None => (String::new(), e.label.clone()),
        };
        let short = LibraryEntry { label: file, ..e.clone() };
        match out.last_mut() {
            Some((h, list)) if *h == dir => list.push(short),
            _ => out.push((dir, vec![short])),
        }
    }
    out
}

/// A horizontal drag on a row, resolved to an intent. Right pins, left
/// demotes — the direction matches the result (up the list / down it) the
/// way a phone user already expects, and the threshold keeps a shaky
/// scroll from re-ranking a library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swipe { Pin, Demote }

pub const SWIPE_PX: f32 = 36.0;

pub fn swipe_of(dx: f32) -> Option<Swipe> {
    if dx >= SWIPE_PX { Some(Swipe::Pin) }
    else if dx <= -SWIPE_PX { Some(Swipe::Demote) }
    else { None }
}

/// What the sidebar asked the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAction {
    ToRack(PathBuf),
    ToTimeline(PathBuf),
    Preview(PathBuf),
    ReplaceChannel(usize, PathBuf),
    /// Send this sample to the morph pad's next corner.
    ToMorph(PathBuf),
    /// Load the pad WHOLE: this sample plus its proposed neighbours —
    /// the breakout's opening move, taken only because it was clicked.
    ToMorphSet(Vec<PathBuf>),
    /// Swiped right (or chosen from ⋯): float this sample to the top of
    /// its listing for this project. Toggles.
    Pin(PathBuf),
    /// Swiped left: sink it. Toggles.
    Demote(PathBuf),
    ToNewTrackBus(PathBuf, Option<studio_core::BusId>),
}

/// The sidebar's own state: which folders are open, what has been scanned,
/// and the live filter. Scans are cached until `refresh` — a per-frame
/// readdir of an open tree would be the syscall storm laziness exists to
/// avoid.
#[derive(Default)]
pub struct TreeState {
    pub expanded: std::collections::BTreeSet<PathBuf>,
    cache: std::collections::BTreeMap<PathBuf, DirListing>,
    pub filter: String,
    results: Vec<LibraryEntry>,
    last_filter: String,
}
impl TreeState {
    pub fn refresh(&mut self) {
        self.cache.clear();
        self.last_filter.clear();
        self.results.clear();
    }
    fn listing(&mut self, dir: &Path) -> DirListing {
        if let Some(l) = self.cache.get(dir) { return l.clone(); }
        let l = scan_dir(dir);
        self.cache.insert(dir.to_path_buf(), l.clone());
        l
    }
}

/// One row: identicon on the LEFT, name in the middle, every button on
/// the RIGHT — so the buttons line up in a single column the eye can
/// track instead of wandering with each name's length, and a narrow
/// sidebar clips the name rather than the controls.
fn sample_row(ui: &mut egui::Ui, e: &LibraryEntry, compact: bool,
              rack: &studio_live::Rack, project: &studio_core::Project,
              prefs: &Prefs, c: &uikit_egui::PaletteColors,
              action: &mut Option<LibraryAction>) {
    let resp = ui.horizontal(|ui| {
        // ── left: the identicon ─────────────────────────────────────────
        let (short, ext) = identicon(&e.label);
        let tint = match ext.as_str() {
            "opus" | "ogg" => c.success,
            "flac" => c.accent,
            "wav" | "aiff" | "aif" => c.warning,
            _ => c.text_weak,
        };
        let heavy = is_heavy(&e.path, e.bytes);
        let rank = prefs.rank(&e.path);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if rank < 0 { ui.colored_label(c.accent, "★"); }
            ui.colored_label(tint, format!("{}", ext.to_uppercase()))
                .on_hover_text(format!("{} · {:.1} MB", e.label, e.bytes as f64 / 1e6));
            // The anvil: heavy on a phone. Rare on purpose — a mark
            // everything wears is a mark nobody reads.
            if heavy {
                ui.colored_label(c.warning, "⚒")
                    .on_hover_text("large or uncompressed — costs real storage and decode");
            }
        });
        // ── middle: the name, elided on narrow displays ────────────────
        let name = if compact { short } else { e.label.clone() };
        let faded = if rank > 0 { c.text_weak } else { c.text };
        ui.colored_label(faded, name).on_hover_text(&e.label);
        // ── right: every button, in one column ─────────────────────────
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("⋯", |ui| {
                let kind = classify(&e.label);
                for s in suggest(kind, rack, project) {
                    if ui.button(&s.label).clicked() {
                        *action = Some(match s.act {
                            SuggestAct::ReplaceChannel(i) =>
                                LibraryAction::ReplaceChannel(i, e.path.clone()),
                            SuggestAct::NewChannel => LibraryAction::ToRack(e.path.clone()),
                            SuggestAct::ClipAtPlayhead =>
                                LibraryAction::ToTimeline(e.path.clone()),
                            SuggestAct::ClipToBus(b) =>
                                LibraryAction::ToNewTrackBus(e.path.clone(), b),
                        });
                        ui.close();
                    }
                }
                if ui.button("add to morph pad").clicked() {
                    *action = Some(LibraryAction::ToMorph(e.path.clone()));
                    ui.close();
                }
                let kin = similar_in_dir(&e.path, 3);
                if !kin.is_empty() {
                    if ui.button(format!("pad: load with {} similar", kin.len())).clicked() {
                        let mut set = vec![e.path.clone()];
                        set.extend(kin);
                        *action = Some(LibraryAction::ToMorphSet(set));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(if prefs.rank(&e.path) < 0 { "unpin" } else { "pin" }).clicked() {
                    *action = Some(LibraryAction::Pin(e.path.clone()));
                    ui.close();
                }
                if ui.button(if prefs.rank(&e.path) > 0 { "restore" } else { "demote" }).clicked() {
                    *action = Some(LibraryAction::Demote(e.path.clone()));
                    ui.close();
                }
                let alts = variants(&e.path);
                if !alts.is_empty() {
                    ui.separator();
                    for v in alts.iter().take(4) {
                        let vext = v.extension().and_then(|x| x.to_str()).unwrap_or("?");
                        if ui.button(format!("use the .{vext} encoding")).clicked() {
                            *action = Some(LibraryAction::Preview(v.clone()));
                            ui.close();
                        }
                    }
                }
            });
            if !compact {
                if ui.small_button("clip").on_hover_text("place at the playhead").clicked() {
                    *action = Some(LibraryAction::ToTimeline(e.path.clone()));
                }
                if ui.small_button("rack").on_hover_text("add as a sampler channel").clicked() {
                    *action = Some(LibraryAction::ToRack(e.path.clone()));
                }
            }
            if ui.small_button("▶").on_hover_text("preview").clicked() {
                *action = Some(LibraryAction::Preview(e.path.clone()));
            }
        });
    }).response;

    // The swipe: triage at listening speed. Resolved on RELEASE from the
    // total drag, so a slow scroll that wanders sideways never re-ranks
    // the library — and the same intents exist in the ⋯ menu, because a
    // gesture nobody discovers is a feature nobody has.
    let dragged = ui.interact(resp.rect, resp.id.with("swipe"), egui::Sense::drag());
    if dragged.drag_stopped() {
        if let Some(sw) = swipe_of(dragged.drag_delta().x) {
            *action = Some(match sw {
                Swipe::Pin => LibraryAction::Pin(e.path.clone()),
                Swipe::Demote => LibraryAction::Demote(e.path.clone()),
            });
        }
    }
}

/// Draw the sidebar: a filter box, then either search results or the lazy
/// tree. Returns at most one action.
pub fn show_tree(ui: &mut egui::Ui, root: &Path, state: &mut TreeState,
                 rack: &studio_live::Rack, project: &studio_core::Project,
                 prefs: &Prefs, c: &uikit_egui::PaletteColors) -> Option<LibraryAction> {
    let mut action = None;
    // Below this the row cannot hold long names AND its buttons, so the
    // name elides to its identicon and the two word-buttons fold into ⋯ —
    // the controls survive, which is the right thing to keep.
    let compact = ui.available_width() < 260.0;
    ui.label(format!("library — {}", root.display()));
    ui.horizontal(|ui| {
        ui.label("find:");
        ui.text_edit_singleline(&mut state.filter);
        if ui.small_button("⟳").on_hover_text("rescan").clicked() { state.refresh(); }
    });
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if !state.filter.trim().is_empty() {
            if state.filter != state.last_filter {
                state.results = search(root, state.filter.trim(), 500);
                state.last_filter = state.filter.clone();
            }
            if state.results.is_empty() {
                ui.colored_label(c.text_weak, "nothing matches");
            }
            if state.results.len() == 500 {
                ui.colored_label(c.text_weak, "first 500 matches — narrow the search");
            }
            // Search results carry their folder in the label, which is
            // what made this sidebar so wide. Grouped under one header
            // each, the rows only need the filename.
            let results = arrange(&state.results.clone(), prefs);
            for (header, list) in sections(&results) {
                if !header.is_empty() {
                    ui.add_space(2.0);
                    ui.colored_label(c.text_weak, header);
                }
                for e in &list {
                    sample_row(ui, e, compact, rack, project, prefs, c, &mut action);
                }
            }
            return;
        }
        #[allow(clippy::too_many_arguments)]
        fn walk(ui: &mut egui::Ui, dir: &Path, depth: usize, state: &mut TreeState,
                rack: &studio_live::Rack, project: &studio_core::Project,
                prefs: &Prefs, compact: bool, c: &uikit_egui::PaletteColors,
                action: &mut Option<LibraryAction>) {
            let listing = state.listing(dir);
            for (name, sub) in &listing.dirs {
                let open = state.expanded.contains(sub);
                let glyph = if open { "▾" } else { "▸" };
                if ui.selectable_label(open, format!("{glyph} {name}")).clicked() {
                    if open { state.expanded.remove(sub); }
                    else { state.expanded.insert(sub.clone()); }
                }
                if open && depth < 12 {
                    ui.indent(sub, |ui| {
                        walk(ui, sub, depth + 1, state, rack, project, prefs,
                             compact, c, action);
                    });
                }
            }
            for e in &arrange(&listing.files, prefs) {
                sample_row(ui, e, compact, rack, project, prefs, c, action);
            }
        }
        let top = state.listing(root);
        if top.dirs.is_empty() && top.files.is_empty() {
            ui.colored_label(c.text_weak,
                "empty. Set STUDIO_LIBRARY or put samples in ./library");
        }
        walk(ui, root, 0, state, rack, project, prefs, compact, c, &mut action);
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of the REAL library this feature was asked against:
    /// deep folders, a batch converter's variant subdirs, drum taxonomy.
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("studio-lib2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deep = root.join("zaytoven/808s & Kicks");
        std::fs::create_dir_all(deep.join("FFaudioConverter")).unwrap();
        std::fs::create_dir_all(deep.join("FFaudioConverterFLAC")).unwrap();
        std::fs::write(deep.join("808 1 (LAYER WITH KICK 1).wav"), b"x").unwrap();
        std::fs::write(deep.join("FFaudioConverter/808 1 (LAYER WITH KICK 1).opus"), b"x").unwrap();
        std::fs::write(deep.join("FFaudioConverterFLAC/808 1 (LAYER WITH KICK 1).flac"), b"x").unwrap();
        std::fs::write(deep.join("Kick 2 (Beast Mode).wav"), b"x").unwrap();
        let vox = root.join("Vox - Clean/FFaudioConverter");
        std::fs::create_dir_all(&vox).unwrap();
        std::fs::write(vox.join("Vox_Dry_Flex_1_Em.opus"), b"x").unwrap();
        std::fs::write(root.join(".hidden.wav"), b"x").unwrap();
        std::fs::write(root.join("Read_Me.pdf"), b"x").unwrap();
        root
    }

    #[test]
    fn the_tree_is_lazy_and_the_search_reaches_the_whole_depth() {
        let root = fixture();
        let top = scan_dir(&root);
        assert_eq!(top.dirs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
                   vec!["Vox - Clean", "zaytoven"], "hidden and non-audio never listed");
        assert!(top.files.is_empty());
        // The two-level scan this replaces could not see any of these.
        let hits = search(&root, "flex", 100);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].label.contains("Vox - Clean"), "labels carry the path: {}", hits[0].label);
        assert_eq!(search(&root, "808", 100).len(), 3, "search is case-blind and deep");
        assert_eq!(search(&root, "808", 2).len(), 2, "and capped");
    }

    #[test]
    fn variants_find_the_batch_converters_siblings_lossless_first() {
        let root = fixture();
        let wav = root.join("zaytoven/808s & Kicks/808 1 (LAYER WITH KICK 1).wav");
        let v = variants(&wav);
        let exts: Vec<_> = v.iter()
            .map(|p| p.extension().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(exts, vec!["flac", "opus"], "the better encoding is the first offer");
        // From the opus side, the wav and flac are the upgrades.
        let opus = root.join("zaytoven/808s & Kicks/FFaudioConverter/808 1 (LAYER WITH KICK 1).opus");
        assert!(variants(&opus).is_empty(),
                "a file inside the converter dir has no same-level variants — \
                 the layout is parent-owns-original, and pretending otherwise \
                 would offer the same file back");
    }

    #[test]
    fn the_classifier_reads_the_real_names_in_their_real_priority() {
        // Every one of these is verbatim from the library this was asked
        // against.
        assert_eq!(classify("808 1 (LAYER WITH KICK 1).wav"), SampleKind::Bass808,
                   "an 808 before it is a kick");
        assert_eq!(classify("Kick 2 (Beast Mode).wav"), SampleKind::Kick);
        assert_eq!(classify("Vox_Loop_10_F_160bpm.opus"), SampleKind::Vox,
                   "a vocal loop is a vocal before it is a loop");
        assert_eq!(classify("Melody_20_McGregor_Fm_160bpm_FULL.wav"), SampleKind::Melody);
        assert_eq!(classify("Hi-Hat 6 (Water Bitch I - Migos).wav"), SampleKind::Hat);
        assert_eq!(classify("Snap 1 (Bakers Man - Migos).wav"), SampleKind::Snare);
        assert_eq!(classify("Triangle 1 (Hobby - Chief Keef).wav"), SampleKind::Perc);
        assert_eq!(classify("FX - Sweet chaos.wav"), SampleKind::Fx);
        assert_eq!(classify("GUCCI BASS.wav"), SampleKind::Unknown,
                   "no token, no guess — Unknown is honest");
    }

    #[test]
    fn suggestions_lead_with_replacement_and_choose_the_bus_by_name() {
        let rack = studio_live::Rack::trap_kit();
        let project = studio_core::Project::default();
        let s = suggest(SampleKind::Snare, &rack, &project);
        assert!(s[0].label.contains("replace channel"),
                "a matching rack channel is the FIRST offer: {:?}", s[0]);
        assert!(matches!(s.last().unwrap().act, SuggestAct::ClipToBus(None)),
                "no drum bus yet: offer to make one");
        let mut with_bus = project.clone();
        with_bus.apply(&studio_core::Op::AddBus { bus: studio_core::Bus {
            id: studio_core::BusId(4), name: "Drum Buss".into(), parent: None,
            gain: 1.0, chain: vec![], sends: vec![], muted: false } }).unwrap();
        let s = suggest(SampleKind::Clap, &rack, &with_bus);
        assert!(s.iter().any(|x| matches!(x.act,
            SuggestAct::ClipToBus(Some(studio_core::BusId(4))))),
            "an existing drum bus is chosen by its name");
        let s = suggest(SampleKind::Vox, &rack, &with_bus);
        assert!(!s.iter().any(|x| matches!(x.act, SuggestAct::ClipToBus(_))),
                "a vocal is not offered a drum bus");
    }

    #[test]
    fn neighbours_are_same_kind_same_dir_and_never_a_format_variant() {
        let root = fixture();
        let deep = root.join("zaytoven/808s & Kicks");
        // Give the 808 real neighbours plus a decoy.
        std::fs::write(deep.join("808 5.wav"), b"x").unwrap();
        std::fs::write(deep.join("zaytoven_808_2.wav"), b"x").unwrap();
        let kin = similar_in_dir(&deep.join("808 1 (LAYER WITH KICK 1).wav"), 3);
        assert_eq!(kin.len(), 2, "the two 808s, not the kick: {kin:?}");
        assert!(kin.iter().all(|p| classify(&p.to_string_lossy())
                                   == SampleKind::Bass808));
        assert!(!kin.iter().any(|p| p.to_string_lossy().contains("FFaudio")),
                "a format variant of the same sound is not a neighbour");
        assert!(similar_in_dir(&root.join("Read_Me.pdf"), 3).is_empty());
    }

    #[test]
    fn the_identicon_elides_the_middle_and_keeps_the_extension() {
        // Real names from the library this was specified against.
        let (short, ext) = identicon("Vox_Dry_Waduwap_high_A#.opus");
        assert_eq!(short, "Vox_…h_A#");
        assert_eq!(ext, "opus");
        // Short names are shown whole: eliding six characters to save two
        // is theatre.
        assert_eq!(identicon("Hat 2.wav"), ("Hat 2".into(), "wav".into()));
        assert_eq!(identicon("808 1 (LAYER WITH KICK 1).flac").0, "808 …K 1)");
    }

    #[test]
    fn the_anvil_marks_the_two_cases_it_was_specified_against() {
        // '96000 Hz, 2 channels, flt, 6144 kb/s' — a float WAV, heavy by
        // KIND, whatever its size.
        assert!(is_heavy(Path::new("Vox_Dry_Ya_A.wav"), 500_000));
        // 'bitrate: 219 kb/s 48k' opus — light, and must STAY light at any
        // plausible one-shot size, or the mark means nothing.
        assert!(!is_heavy(Path::new("Vox_Dry_Ya_A.opus"), 6_000));
        assert!(!is_heavy(Path::new("Melody_1_Memories_G_126bpm_FULL.opus"), 900_000));
        // FLAC is light until it is simply big.
        assert!(!is_heavy(Path::new("Snare 3.flac"), 400_000));
        assert!(is_heavy(Path::new("Melody long.flac"), 40_000_000));
        // mp3 is marked by kind: kept for compatibility, never the file
        // you want on a phone.
        assert!(is_heavy(Path::new("zaytoven_808_1.mp3"), 100_000));
    }

    #[test]
    fn pinning_floats_demoting_sinks_and_they_are_opposites() {
        let mk = |name: &str| LibraryEntry { label: name.into(),
            path: PathBuf::from(name), bytes: 1 };
        let list = vec![mk("a.opus"), mk("b.opus"), mk("c.opus"), mk("d.opus")];
        let mut p = Prefs::default();
        p.pin(Path::new("c.opus"));
        p.demote(Path::new("a.opus"));
        let order: Vec<String> = arrange(&list, &p).iter().map(|e| e.label.clone()).collect();
        assert_eq!(order, vec!["c.opus", "b.opus", "d.opus", "a.opus"],
                   "pinned first, demoted last, the rest undisturbed");
        // Pinning something demoted lifts it rather than making it both.
        p.pin(Path::new("a.opus"));
        assert_eq!(p.rank(Path::new("a.opus")), -1);
        assert!(!p.demoted.contains(Path::new("a.opus")));
        // And both toggle.
        p.pin(Path::new("a.opus"));
        assert_eq!(p.rank(Path::new("a.opus")), 0);
    }

    #[test]
    fn sections_lift_the_folder_out_of_every_row() {
        // The width problem stated plainly: three hits in one deep folder
        // repeated its path three times. One header, three bare names.
        let mk = |label: &str| LibraryEntry { label: label.into(),
            path: PathBuf::from(label), bytes: 1 };
        let flat = vec![
            mk("zaytoven/808s & Kicks/808 1.wav"),
            mk("zaytoven/808s & Kicks/808 5.wav"),
            mk("Vox - Clean/FFaudioConverter/Vox_Dry_Ya_A.opus"),
            mk("loose.opus"),
        ];
        let s = sections(&flat);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].0, "zaytoven/808s & Kicks");
        assert_eq!(s[0].1.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
                   vec!["808 1.wav", "808 5.wav"], "rows carry only the filename");
        assert_eq!(s[2].0, "", "a root-level file needs no header");
        assert_eq!(s[0].1[0].path, PathBuf::from("zaytoven/808s & Kicks/808 1.wav"),
                   "the path survives the shortening — actions still work");
    }

    #[test]
    fn a_swipe_needs_real_travel_in_a_clear_direction() {
        assert_eq!(swipe_of(SWIPE_PX + 1.0), Some(Swipe::Pin));
        assert_eq!(swipe_of(-SWIPE_PX - 1.0), Some(Swipe::Demote));
        // A scroll that wanders sideways must not re-rank the library.
        assert_eq!(swipe_of(12.0), None);
        assert_eq!(swipe_of(-12.0), None);
    }

    #[test]
    fn a_missing_library_is_an_empty_listing_not_a_crash() {
        assert_eq!(scan_dir(Path::new("/definitely/not/real")), DirListing::default());
        assert!(search(Path::new("/definitely/not/real"), "x", 10).is_empty());
    }
}
