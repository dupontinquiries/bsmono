//! modify_tab.rs — clipboard-driven file router (pp-style)
//!
//! Layout (top → bottom):
//!   ┌─ Clipboard preview ─────────────────────────────────────────┐
//!   │ current clipboard content (first ~10 lines)                 │
//!   │ [◀ prev]  entry N of M  [next ▶]         [mode: Replace ▾]  │
//!   ├─ Candidates ────────────────────────────────────────────────┤
//!   │ > file.rs                          [auto]  diff-score: 82%  │
//!   │   file.rs:10-42                    [range] diff-score: 71%  │
//!   │   lib.rs                           [file]                   │
//!   │   [+] new_file.rs                  [create]                 │
//!   ├─ Preview (selected file or range) ──────────────────────────┤
//!   │ <contents of selected entry, diff-highlighted>              │
//!   ├─ Search / input bar ────────────────────────────────────────┤
//!   │ [🔍  fuzzy search / new filename…]          [✦ Paste]       │
//!   └─────────────────────────────────────────────────────────────┘

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, ListBox, ListBoxRow,
    Orientation, PolicyType, ScrolledWindow, SelectionMode,
    TextView, TextBuffer, WrapMode,
};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib;

// ── Entry types ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Entry {
    File(PathBuf),
    Range { path: PathBuf, start: usize, end: usize },
    All(PathBuf),
    Create(String),
}

impl Entry {
    fn label(&self) -> String {
        match self {
            Entry::File(p)  => p.file_name()
                .unwrap_or(p.as_os_str()).to_string_lossy().into(),
            Entry::All(p)   => format!("{}:a", p.display()),
            Entry::Range { path, start, end } =>
                format!("{}:{}-{}", path.display(), start, end),
            Entry::Create(n) => format!("[+] {n}"),
        }
    }

    fn kind_badge(&self) -> &'static str {
        match self {
            Entry::File(_)   => "file",
            Entry::All(_)    => "all",
            Entry::Range {..} => "range",
            Entry::Create(_) => "create",
        }
    }

    fn path(&self) -> Option<&PathBuf> {
        match self {
            Entry::File(p) | Entry::All(p) | Entry::Range { path: p, .. } => Some(p),
            Entry::Create(_) => None,
        }
    }
}

// ── Paste mode ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum PasteMode { Replace, Append, Patch }

impl PasteMode {
    fn label(self) -> &'static str {
        match self {
            PasteMode::Replace => "Replace",
            PasteMode::Append  => "Append",
            PasteMode::Patch   => "Patch",
        }
    }
    fn next(self) -> Self {
        match self {
            PasteMode::Replace => PasteMode::Append,
            PasteMode::Append  => PasteMode::Patch,
            PasteMode::Patch   => PasteMode::Replace,
        }
    }
}

// ── Diff scoring ──────────────────────────────────────────────────────────────

/// Closure density heuristic — balanced braces suggest a complete structural unit
fn closure_density(s: &str) -> f32 {
    let (mut open, mut close) = (0usize, 0usize);
    for c in s.chars() {
        match c { '{' | '(' | '[' => open += 1, '}' | ')' | ']' => close += 1, _ => {} }
    }
    let score = (open * 2 + close * 3) as f32;
    score / (s.len().max(1) as f32)
}

fn closure_balanced(s: &str) -> bool {
    let (mut open, mut close) = (0i32, 0i32);
    for c in s.chars() {
        match c { '{' | '(' | '[' => open += 1, '}' | ')' | ']' => close += 1, _ => {} }
    }
    open == close
}

/// Fast line-level diff score: count matching lines in a sliding window
fn subsection_score(clip: &str, file: &str) -> usize {
    let clip_lines: Vec<&str> = clip.lines().take(64).collect();
    let file_lines: Vec<&str> = file.lines().take(2000).collect();
    if clip_lines.is_empty() || file_lines.is_empty() { return 0; }

    let c_len = clip_lines.len();
    let f_len = file_lines.len();
    if f_len < c_len { return 0; }

    let mut best = 0usize;
    for start in 0..=(f_len - c_len) {
        let window = &file_lines[start..start + c_len];
        let score: usize = clip_lines.iter().zip(window.iter())
            .map(|(a, b)| if a == b { 3 } else if a.trim() == b.trim() { 2 } else { 0 })
            .sum();
        if score > best { best = score; }
        if best >= c_len * 3 { break; } // perfect match early exit
    }
    best
}

/// Find best matching range (start line, end line) in file for the clip
fn best_range(clip: &str, file: &str) -> Option<(usize, usize)> {
    let clip_lines: Vec<&str> = clip.lines().take(64).collect();
    let file_lines: Vec<&str> = file.lines().take(2000).collect();
    if clip_lines.is_empty() || file_lines.is_empty() { return None; }

    let c_len = clip_lines.len();
    let f_len = file_lines.len();
    if f_len < c_len { return None; }

    let mut best_score = 0usize;
    let mut best_start = 0usize;

    for start in 0..=(f_len - c_len) {
        let window = &file_lines[start..start + c_len];
        let score: usize = clip_lines.iter().zip(window.iter())
            .map(|(a, b)| if a == b { 3 } else if a.trim() == b.trim() { 2 } else { 0 })
            .sum();
        if score > best_score { best_score = score; best_start = start; }
    }

    if best_score >= c_len { // at least 1/3 lines match
        Some((best_start, (best_start + c_len).min(f_len)))
    } else {
        None
    }
}

/// Decide automatic paste mode from structure + diff signals
fn auto_mode(clip: &str, file_content: &str) -> PasteMode {
    let score = subsection_score(clip, file_content);
    let density = closure_density(clip);
    let balanced = closure_balanced(clip);

    if balanced && density > 0.015 && score < 10 {
        PasteMode::Replace // complete structural unit, low overlap → replace
    } else if score > 20 {
        PasteMode::Patch   // strong overlap → surgical patch
    } else {
        PasteMode::Replace // default
    }
}

// ── Fuzzy matching ────────────────────────────────────────────────────────────

fn fuzzy_score(query: &str, text: &str) -> usize {
    if query.is_empty() { return 1; }
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    let mut score = 0usize;
    let mut qi = q.as_bytes();
    for c in t.bytes() {
        if qi.first() == Some(&c) {
            score += 2;
            qi = &qi[1..];
        } else {
            score += 0;
        }
    }
    // bonus: exact contains
    if t.contains(&q) { score += 10; }
    score
}

// ── Read clipboard ────────────────────────────────────────────────────────────

fn read_clipboard() -> String {
    // Try wl-paste (Wayland), fall back to xclip (X11)
    for cmd in &[&["wl-paste"][..], &["xclip", "-selection", "clipboard", "-o"][..]] {
        if let Ok(out) = std::process::Command::new(cmd[0]).args(&cmd[1..]).output() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if !s.is_empty() { return s; }
        }
    }
    String::new()
}

fn clipboard_history() -> Vec<String> {
    // KDE Klipper via qdbus
    if let Ok(out) = std::process::Command::new("qdbus")
        .args(["org.kde.klipper", "/klipper",
               "org.kde.klipper.klipper.getClipboardHistory"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let items: Vec<String> = text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();
        if !items.is_empty() { return items; }
    }
    // Fallback: single current clipboard entry
    let cur = read_clipboard();
    if cur.is_empty() { vec![] } else { vec![cur] }
}

fn klipper_get(idx: usize) -> String {
    if let Ok(out) = std::process::Command::new("qdbus")
        .args(["org.kde.klipper", "/klipper",
               "org.kde.klipper.klipper.getClipboardHistoryItem",
               &idx.to_string()])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() { return s; }
    }
    if idx == 0 { read_clipboard() } else { String::new() }
}


// ── Async file scanning ───────────────────────────────────────────────────────

fn scan_entries(clip: &str, query: &str) -> Vec<(Entry, usize)> {
    let mut candidates: Vec<(Entry, usize)> = Vec::new();

    let Ok(rd) = std::fs::read_dir(".") else { return candidates };

    let mut files: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();

    for path in &files {
        let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let fuzz = fuzzy_score(query, &fname);
        if !query.is_empty() && fuzz == 0 { continue; }

        let content = std::fs::read_to_string(path).unwrap_or_default();
        let diff_sc = subsection_score(clip, &content);

        candidates.push((Entry::File(path.clone()), fuzz + diff_sc / 3));

        if diff_sc >= clip.lines().count().max(1) && !clip.trim().is_empty() {
            if let Some((start, end)) = best_range(clip, &content) {
                candidates.push((
                    Entry::Range { path: path.clone(), start, end },
                    fuzz + diff_sc / 2 + 5,
                ));
            }
        }
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.truncate(8);

    // Create entry for new filename
    if !query.is_empty() && !query.contains('/') {
        let exists = candidates.iter().any(|(e, _)|
            e.label().to_lowercase().contains(&query.to_lowercase())
        );
        if !exists {
            candidates.push((Entry::Create(query.to_string()), 0));
        }
    }

    candidates
}


/// Fire an async file scan. Results arrive via `tx`; the 80ms drain timer picks them up.
fn spawn_scan(
    clip: String,
    query: String,
    tx: &crossbeam_channel::Sender<(String, Vec<(Entry, usize)>)>,
) {
    let tx = tx.clone();
    rayon::spawn(move || {
        let entries = scan_entries(&clip, &query);
        let _ = tx.send((query, entries));
    });
}

thread_local! {
    static LAST_CAND_HASH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// ── ModifyTab state ───────────────────────────────────────────────────────────

struct State {
    clipboard:      Vec<String>,
    clip_idx:       usize,
    query:          String,
    entries:        Vec<(Entry, usize)>,  // (entry, diff_score)
    selected:       usize,
    mode:           PasteMode,
    scanning:       bool,
    preview_cache:  String,   // async-loaded file preview; empty until loaded
}

impl State {
    fn new() -> Self {
        // Start with empty clipboard — populated async after construction
        let clipboard = vec![];
        let clip_idx = 0;
        let s = Self {
            clipboard,
            clip_idx,
            query: String::new(),
            entries: Vec::new(),
            selected: 0,
            mode: PasteMode::Replace,
            scanning: false,
            preview_cache: String::new(),
        };
        // entries start empty; spawn_scan is called after the tab realizes
        s
    }

    fn current_clip(&self) -> &str {
        self.clipboard.get(self.clip_idx).map(|s| s.as_str()).unwrap_or("")
    }

    fn clip_len(&self) -> usize { self.clipboard.len().max(1) }

    fn nav_clip(&mut self, delta: isize) {
        let len = self.clip_len() as isize;
        let mut x = self.clip_idx as isize + delta;
        x %= len;
        x += (x >> 63) & len; // branchless negative wrap
        self.clip_idx = x as usize;
        // caller must fire spawn_scan after nav_clip
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected).map(|(e, _)| e)
    }

    fn execute_paste(&self) -> Result<(), String> {
        let clip = self.current_clip();
        let entry = self.selected_entry().ok_or("no entry selected")?;

        match entry {
            Entry::Create(name) => {
                std::fs::write(name, clip).map_err(|e| e.to_string())?;
            }
            Entry::File(path) | Entry::All(path) => {
                match self.mode {
                    PasteMode::Replace => {
                        std::fs::write(path, clip).map_err(|e| e.to_string())?;
                    }
                    PasteMode::Append => {
                        use std::io::Write;
                        let mut f = std::fs::OpenOptions::new()
                            .append(true).create(true).open(path)
                            .map_err(|e| e.to_string())?;
                        write!(f, "{clip}").map_err(|e| e.to_string())?;
                    }
                    PasteMode::Patch => {
                        // For patch mode, try to apply range replacement
                        let content = std::fs::read_to_string(path)
                            .map_err(|e| e.to_string())?;
                        if let Some((start, end)) = best_range(clip, &content) {
                            let mut lines: Vec<&str> = content.lines().collect();
                            let new_lines: Vec<&str> = clip.lines().collect();
                            lines.splice(start..end, new_lines);
                            std::fs::write(path, lines.join("\n")).map_err(|e| e.to_string())?;
                        } else {
                            std::fs::write(path, clip).map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
            Entry::Range { path, start, end } => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| e.to_string())?;
                let mut lines: Vec<&str> = content.lines().collect();
                let new_lines: Vec<&str> = clip.lines().collect();
                lines.splice(*start..*end, new_lines);
                std::fs::write(path, lines.join("\n")).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn preview_content(&self) -> &str {
        // Returns the cached preview (populated asynchronously)
        &self.preview_cache
    }

    /// Returns the path and byte range to load for the current selection.
    fn preview_load_spec(&self) -> Option<(std::path::PathBuf, Option<(usize,usize)>)> {
        match self.selected_entry()? {
            Entry::File(p) | Entry::All(p) => Some((p.clone(), None)),
            Entry::Range { path, start, end } => Some((path.clone(), Some((*start,*end)))),
            Entry::Create(_) => None,
        }
    }
}

// ── Widget construction ───────────────────────────────────────────────────────

pub struct ModifyTab {
    pub root: GtkBox,
}

impl ModifyTab {
    pub fn new() -> Self {
        let state = Rc::new(RefCell::new(State::new()));

        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        // ── Top: clipboard preview bar ─────────────────────────────────────
        let clip_bar = gtk4::ActionBar::new();
        clip_bar.add_css_class("modify-clip-bar");

        let prev_btn = Button::from_icon_name("go-previous-symbolic");
        prev_btn.add_css_class("flat");
        prev_btn.set_tooltip_text(Some("Previous clipboard entry (..p)"));

        let next_btn = Button::from_icon_name("go-next-symbolic");
        next_btn.add_css_class("flat");
        next_btn.set_tooltip_text(Some("Next clipboard entry (..n)"));

        let clip_idx_lbl = Label::new(Some("1 of 1"));
        clip_idx_lbl.add_css_class("dim-label");

        let mode_btn = Button::with_label("Replace");
        mode_btn.add_css_class("flat");
        mode_btn.add_css_class("modify-mode-btn");
        mode_btn.set_tooltip_text(Some("Toggle paste mode: Replace → Append → Patch"));

        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.add_css_class("flat");
        refresh_btn.set_tooltip_text(Some("Refresh clipboard history"));

        clip_bar.pack_start(&prev_btn);
        clip_bar.pack_start(&next_btn);
        clip_bar.pack_start(&clip_idx_lbl);
        clip_bar.pack_end(&mode_btn);
        clip_bar.pack_end(&refresh_btn);

        // Clipboard content preview
        let clip_buf = TextBuffer::new(None);
        let clip_view = TextView::builder()
            .buffer(&clip_buf)
            .editable(false)
            .wrap_mode(WrapMode::WordChar)
            .monospace(true)
            .build();
        clip_view.add_css_class("modify-clip-view");

        let clip_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_height(80)
            .max_content_height(120)
            .child(&clip_view)
            .build();

        root.append(&clip_bar);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));
        root.append(&clip_scroll);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Middle: candidate list + preview side-by-side ─────────────────
        let mid_box = GtkBox::new(Orientation::Horizontal, 0);
        mid_box.set_vexpand(true);

        // Candidate list
        let cand_list = ListBox::new();
        cand_list.set_selection_mode(SelectionMode::Single);
        cand_list.add_css_class("modify-cand-list");

        let cand_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_width(180)
            .max_content_width(280)
            .vexpand(true)
            .child(&cand_list)
            .build();

        // Preview area
        let preview_buf = TextBuffer::new(None);
        let preview_view = TextView::builder()
            .buffer(&preview_buf)
            .editable(false)
            .wrap_mode(WrapMode::WordChar)
            .monospace(true)
            .vexpand(true)
            .hexpand(true)
            .build();
        preview_view.add_css_class("modify-preview-view");

        let preview_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&preview_view)
            .build();

        mid_box.append(&cand_scroll);
        mid_box.append(&gtk4::Separator::new(Orientation::Vertical));
        mid_box.append(&preview_scroll);
        root.append(&mid_box);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Bottom: search bar + paste button ─────────────────────────────
        let bottom_bar = gtk4::ActionBar::new();
        bottom_bar.add_css_class("modify-bottom-bar");

        let search_entry = gtk4::SearchEntry::new();
        search_entry.set_hexpand(true);
        search_entry.set_placeholder_text(Some("fuzzy search  ·  or type new filename"));
        search_entry.set_tooltip_text(Some(
            "Type to filter files. Matches fuzzy — partial strings work.\n\
             Type a new name to create a file from clipboard.\n\
             Arrow keys move selection."
        ));

        let paste_btn = Button::with_label("✦ Paste");
        paste_btn.add_css_class("suggested-action");
        paste_btn.set_tooltip_text(Some("Paste clipboard into selected file (double-tap on mobile)"));

        let status_lbl = Label::new(Some(""));
        status_lbl.add_css_class("dim-label");
        status_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Start);

        bottom_bar.pack_start(&search_entry);
        bottom_bar.pack_end(&paste_btn);
        clip_bar.set_center_widget(Some(&status_lbl));

        root.append(&bottom_bar);

        // ── Wire: populate initial state ───────────────────────────────────
        let clip_buf2    = clip_buf.clone();
        let preview_buf2 = preview_buf.clone();
        let cand_list2   = cand_list.clone();
        let clip_idx_lbl2 = clip_idx_lbl.clone();
        let mode_btn2    = mode_btn.clone();
        let status_lbl2  = status_lbl.clone();

        let refresh_ui = {
            let state = Rc::clone(&state);
            move || {
                let s = state.borrow();

                // Clipboard preview — only update if content changed
                let clip_preview = s.current_clip().lines().take(12)
                    .collect::<Vec<_>>().join("\n");
                {
                    let start = clip_buf2.start_iter();
                    let end   = clip_buf2.end_iter();
                    if clip_buf2.text(&start, &end, false).as_str() != clip_preview {
                        clip_buf2.set_text(&clip_preview);
                    }
                }

                // Index label
                let idx_text = format!("{} of {}", s.clip_idx + 1, s.clipboard.len().max(1));
                if clip_idx_lbl2.text().as_str() != idx_text {
                    clip_idx_lbl2.set_text(&idx_text);
                }

                // Mode button
                if mode_btn2.label().as_deref() != Some(s.mode.label()) {
                    mode_btn2.set_label(s.mode.label());
                }

                // Preview — async-loaded cache, only update if changed
                {
                    let preview = s.preview_content().to_string();
                    let start = preview_buf2.start_iter();
                    let end   = preview_buf2.end_iter();
                    if preview_buf2.text(&start, &end, false).as_str() != preview {
                        preview_buf2.set_text(&preview);
                    }
                }

                // Status
                if let Some((e, sc)) = s.entries.get(s.selected) {
                    let score_str = if *sc > 0 { format!("  score:{sc}") } else { String::new() };
                    status_lbl2.set_text(&format!("{}{score_str}", e.label()));
                } else {
                    status_lbl2.set_text("");
                }

                // Candidates list: only rebuild when content actually changes.
                // Computing a quick fingerprint from entry labels avoids
                // row add/remove (which causes full ListBox relayout) when
                // the same files are re-ranked in the same order.
                let entry_count = s.entries.len();

                let fingerprint: u64 = {
                    use std::hash::{Hash, Hasher, DefaultHasher};
                    let mut h = DefaultHasher::new();
                    for (e, sc) in &s.entries { e.label().hash(&mut h); sc.hash(&mut h); }
                    h.finish()
                };
                // Static cache — checked per refresh_ui invocation
                if LAST_CAND_HASH.with(|c| {
                    let v = c.get();
                    if v == fingerprint { true } else { c.set(fingerprint); false }
                }) {
                    // Nothing changed — skip the expensive list rebuild entirely
                    return;
                }

                let row_count = |cl: &ListBox| -> usize {
                    let mut n = 0i32;
                    while cl.row_at_index(n).is_some() { n += 1; }
                    n as usize
                };
                while row_count(&cand_list2) > entry_count {
                    let n = row_count(&cand_list2);
                    if let Some(r) = cand_list2.row_at_index(n as i32 - 1) {
                        cand_list2.remove(&r);
                    }
                }
                let current_count = row_count(&cand_list2);
                for _ in current_count..entry_count {
                    let row_box = GtkBox::new(Orientation::Horizontal, 8);
                    row_box.set_margin_start(8); row_box.set_margin_end(8);
                    row_box.set_margin_top(6);   row_box.set_margin_bottom(6);
                    let name_lbl = Label::new(Some(""));
                    name_lbl.set_hexpand(true);
                    name_lbl.set_halign(gtk4::Align::Start);
                    name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                    name_lbl.add_css_class("modify-cand-name");
                    let badge_lbl = Label::new(Some(""));
                    badge_lbl.add_css_class("modify-cand-badge");
                    row_box.append(&name_lbl);
                    row_box.append(&badge_lbl);
                    let row = ListBoxRow::new();
                    row.set_child(Some(&row_box));
                    cand_list2.append(&row);
                }

                // Update content of each row in place
                for (i, (entry, _score)) in s.entries.iter().enumerate() {
                    if let Some(row) = cand_list2.row_at_index(i as i32) {
                        if let Some(rb) = row.child().and_then(|w| w.downcast::<GtkBox>().ok()) {
                            if let Some(nl) = rb.first_child().and_then(|w| w.downcast::<Label>().ok()) {
                                nl.set_text(&entry.label());
                            }
                            if let Some(bl) = rb.last_child().and_then(|w| w.downcast::<Label>().ok()) {
                                bl.set_text(entry.kind_badge());
                                // Update badge CSS class
                                for cls in ["modify-badge-file","modify-badge-range",
                                            "modify-badge-all","modify-badge-create"] {
                                    bl.remove_css_class(cls);
                                }
                                bl.add_css_class(&format!("modify-badge-{}", entry.kind_badge()));
                            }
                        }
                        if i == s.selected {
                            cand_list2.select_row(Some(&row));
                        }
                    }
                }
            }
        };

        // Initial populate
        refresh_ui();

        // Shared async scan channel — used by ALL triggers (search, nav, paste, poll)
        let (scan_tx, scan_rx) =
            crossbeam_channel::unbounded::<(String, Vec<(Entry, usize)>)>();

        // ── Wire: prev/next clipboard ──────────────────────────────────────
        {
            let st = Rc::clone(&state);
            let rf = refresh_ui.clone();
            let tx = scan_tx.clone();
            prev_btn.connect_clicked(move |_| {
                st.borrow_mut().nav_clip(-1);
                rf();
            });
        }
        {
            let st = Rc::clone(&state);
            let rf = refresh_ui.clone();
            let tx = scan_tx.clone();
            next_btn.connect_clicked(move |_| {
                st.borrow_mut().nav_clip(1);
                rf();
            });
        }

        // ── Wire: mode toggle ──────────────────────────────────────────────
        {
            let st = Rc::clone(&state);
            let rf = refresh_ui.clone();
            mode_btn.connect_clicked(move |_| {
                let next = st.borrow().mode.next();
                st.borrow_mut().mode = next;
                rf();
            });
        }

        // ── Wire: clipboard refresh ────────────────────────────────────────
        {
            let st = Rc::clone(&state);
            let rf = refresh_ui.clone();
            let tx = scan_tx.clone();
            refresh_btn.connect_clicked(move |_| {
                // clipboard_history() spawns subprocesses — can't cross thread boundary
                // with Rc/GTK types. Instead: spawn thread to fetch only, then handle
                // result back on the main thread via a channel + idle_add.
                let tx2 = tx.clone();
                let st2 = st.clone();
                let rf2 = rf.clone();
                let (htx, hrx) = crossbeam_channel::unbounded::<Vec<String>>();
                std::thread::spawn(move || {
                    let _ = htx.send(clipboard_history());
                });
                glib::idle_add_local_once(move || {
                    if let Ok(hist) = hrx.try_recv() {
                        if !hist.is_empty() {
                            let (clip, query) = {
                                let mut s = st2.borrow_mut();
                                s.clipboard = hist;
                                s.clip_idx = 0;
                                (s.current_clip().to_string(), s.query.clone())
                            };
                            spawn_scan(clip, query, &tx2);
                            rf2();
                        }
                    }
                });
            });
        }

        // Preview load channel: background thread reads file, sends text here
        let (prev_tx, prev_rx) = crossbeam_channel::unbounded::<String>();

        // Drain scan results onto the main thread every 80ms
        {
            let st2  = Rc::clone(&state);
            let rf2  = refresh_ui.clone();
            let prev_tx2 = prev_tx.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
                let mut got = false;
                loop {
                    match scan_rx.try_recv() {
                        Ok((query, entries)) => {
                            let mut s = st2.borrow_mut();
                            if s.query == query {
                                s.entries  = entries;
                                s.selected = 0;
                                s.scanning = false;
                                s.preview_cache = String::new(); // clear stale
                                got = true;
                            }
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(_) => break,
                    }
                }
                if got {
                    // Load preview file asynchronously to avoid blocking main thread
                    let spec = st2.borrow().preview_load_spec();
                    if let Some((path, range)) = spec {
                        let ptx = prev_tx2.clone();
                        std::thread::spawn(move || {
                            let text = std::fs::read_to_string(&path).unwrap_or_default();
                            let preview = match range {
                                None => text.lines().take(40).collect::<Vec<_>>().join("\n"),
                                Some((lo, hi)) => {
                                    let lines: Vec<&str> = text.lines().collect();
                                    let lo = lo.min(lines.len());
                                    let hi = hi.min(lines.len());
                                    lines[lo..hi].join("\n")
                                }
                            };
                            let _ = ptx.send(preview);
                        });
                    }
                    rf2();
                }

                // Apply loaded preview text
                while let Ok(preview) = prev_rx.try_recv() {
                    st2.borrow_mut().preview_cache = preview;
                    rf2();
                }

                glib::ControlFlow::Continue
            });
        }

        // Poll clipboard every 2s via background thread → 500ms drain on main thread
        {
            let st = Rc::clone(&state);
            let rf = refresh_ui.clone();
            let tx = scan_tx.clone();
            let (clip_tx, clip_rx) = crossbeam_channel::unbounded::<Vec<String>>();
            {
                let clip_tx2 = clip_tx.clone();
                std::thread::spawn(move || {
                    // Fire once immediately on startup, then every 2s
                    let hist = clipboard_history();
                    if !hist.is_empty() { let _ = clip_tx2.send(hist); }
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let hist = clipboard_history();
                        if !hist.is_empty() { let _ = clip_tx2.send(hist); }
                    }
                });
            }
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                while let Ok(hist) = clip_rx.try_recv() {
                    let cur = st.borrow().clipboard.first().cloned().unwrap_or_default();
                    if hist.first().map(|s| s.as_str()) != Some(cur.as_str()) {
                        {
                            let mut s = st.borrow_mut();
                            s.clipboard = hist;
                            s.clip_idx = 0;
                        }
                        let (clip, query) = {
                            let s = st.borrow();
                            (s.current_clip().to_string(), s.query.clone())
                        };
                        spawn_scan(clip, query, &tx);
                        rf();
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // ── Wire: paste button ─────────────────────────────────────────────
        {
            let st = Rc::clone(&state);
            let sl = status_lbl.clone();
            paste_btn.connect_clicked(move |_| {
                let result = st.borrow().execute_paste();
                match result {
                    Ok(msg)  => { sl.set_text(&msg); }
                    Err(e)   => { sl.set_text(&format!("Error: {e}")); }
                }
            });
        }

        // ── Wire: search entry ─────────────────────────────────────────────
        {
            let st  = Rc::clone(&state);
            let rf  = refresh_ui.clone();
            let tx  = scan_tx.clone();
            search_entry.connect_changed(move |e| {
                let q = e.text().to_string();
                let clip = {
                    let mut s = st.borrow_mut();
                    s.query = q.clone();
                    s.current_clip().to_string()
                };
                spawn_scan(clip, q, &tx);
                rf();
            });
        }

        // ── Wire: Enter key in search also pastes ──────────────────────────
        {
            let st = Rc::clone(&state);
            let sl = status_lbl.clone();
            search_entry.connect_activate(move |_| {
                let result = st.borrow().execute_paste();
                match result {
                    Ok(msg)  => { sl.set_text(&msg); }
                    Err(e)   => { sl.set_text(&format!("Error: {e}")); }
                }
            });
        }

        // Fire initial scan so candidates are populated immediately
        {
            let (clip, query) = {
                let s = state.borrow();
                (s.current_clip().to_string(), s.query.clone())
            };
            spawn_scan(clip, query, &scan_tx);
        }

        ModifyTab { root }
    }
}
