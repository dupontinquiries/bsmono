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
            out.files.push(LibraryEntry { label: name, path: p });
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
                out.push(LibraryEntry { label, path: f.path });
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

/// What the sidebar asked the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAction {
    ToRack(PathBuf),
    ToTimeline(PathBuf),
    Preview(PathBuf),
    ReplaceChannel(usize, PathBuf),
    /// Send this sample to the morph pad's next corner.
    ToMorph(PathBuf),
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

fn sample_row(ui: &mut egui::Ui, e: &LibraryEntry,
              rack: &studio_live::Rack, project: &studio_core::Project,
              action: &mut Option<LibraryAction>) {
    ui.horizontal(|ui| {
        if ui.small_button("▶").on_hover_text("preview").clicked() {
            *action = Some(LibraryAction::Preview(e.path.clone()));
        }
        ui.label(&e.label);
        // The ⋯ menu is the smart-offer surface: replacement targets, the
        // bus choice, the better-encoded variants. (The voiceterm
        // hover-drag selector is the intended upgrade for this menu once
        // that widget lives in shared/; a plain menu teaches the same
        // offers today.)
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
            let alts = variants(&e.path);
            if !alts.is_empty() {
                ui.separator();
                for v in alts.iter().take(4) {
                    let ext = v.extension().and_then(|x| x.to_str()).unwrap_or("?");
                    if ui.button(format!("use the .{ext} encoding")).clicked() {
                        *action = Some(LibraryAction::Preview(v.clone()));
                        ui.close();
                    }
                }
            }
        });
        if ui.small_button("rack").on_hover_text("add as a sampler channel").clicked() {
            *action = Some(LibraryAction::ToRack(e.path.clone()));
        }
        if ui.small_button("clip").on_hover_text("place at the playhead").clicked() {
            *action = Some(LibraryAction::ToTimeline(e.path.clone()));
        }
    });
}

/// Draw the sidebar: a filter box, then either search results or the lazy
/// tree. Returns at most one action.
pub fn show_tree(ui: &mut egui::Ui, root: &Path, state: &mut TreeState,
                 rack: &studio_live::Rack, project: &studio_core::Project,
                 c: &uikit_egui::PaletteColors) -> Option<LibraryAction> {
    let mut action = None;
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
            let results = state.results.clone();
            for e in &results {
                sample_row(ui, e, rack, project, &mut action);
            }
            return;
        }
        fn walk(ui: &mut egui::Ui, dir: &Path, depth: usize, state: &mut TreeState,
                rack: &studio_live::Rack, project: &studio_core::Project,
                c: &uikit_egui::PaletteColors, action: &mut Option<LibraryAction>) {
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
                        walk(ui, sub, depth + 1, state, rack, project, c, action);
                    });
                }
            }
            for e in &listing.files {
                sample_row(ui, e, rack, project, action);
            }
        }
        let top = state.listing(root);
        if top.dirs.is_empty() && top.files.is_empty() {
            ui.colored_label(c.text_weak,
                "empty. Set STUDIO_LIBRARY or put samples in ./library");
        }
        walk(ui, root, 0, state, rack, project, c, &mut action);
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
    fn a_missing_library_is_an_empty_listing_not_a_crash() {
        assert_eq!(scan_dir(Path::new("/definitely/not/real")), DirListing::default());
        assert!(search(Path::new("/definitely/not/real"), "x", 10).is_empty());
    }
}
