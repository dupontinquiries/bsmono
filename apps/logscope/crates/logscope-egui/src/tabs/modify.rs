//! Modify tab — clipboard-history driven edit/transform pipeline with a
//! Replace/Append/Patch/Auto paste mode. Ports `ui/modify_tab.rs`'s `ModifyTab`.
//!
//! ## What this port fixes
//!
//! The previous version of this file was a visual mockup: `history` was three hardcoded sample
//! strings, "Refresh clipboard history" was a button that did nothing (`let _ = ui.button(...)`),
//! and there was no diff-score or auto-mode at all — the GTK original's whole point
//! (`ui/modify_tab.rs`'s doc comment: "diff-score: 82%", `[auto]`) was absent. This port carries
//! over the GTK version's pure scoring functions verbatim (`subsection_score`, `best_range`,
//! `auto_mode`, `closure_density`/`closure_balanced`) — they have zero GTK dependency, so nothing
//! about the port needed reinventing, only relocating and testing — and wires real clipboard
//! reads (`wl-paste`/`xclip`, KDE's `klipper` history via `qdbus`) in place of the fake data.
//!
//! Not ported: `ui/modify_tab.rs`'s `scan_entries`/`spawn_scan` (background multi-file candidate
//! scanning across a whole project, async against GTK's signal model). This tab's current shape is
//! one target buffer, not a multi-file candidate list — porting the scanner would mean redesigning
//! the tab's layout, not translating existing logic, and is a separate piece of work.
use crate::theme;
use egui::RichText;

#[derive(Clone, Copy, PartialEq, Debug)]
enum PasteMode { Replace, Append, Patch, Auto }
impl PasteMode {
    fn label(self) -> &'static str {
        match self { PasteMode::Replace => "Replace", PasteMode::Append => "Append",
                     PasteMode::Patch => "Patch", PasteMode::Auto => "Auto" }
    }
}

// ── ported verbatim from ui/modify_tab.rs — pure, no GTK dependency ────────────────────────────

/// Closure/bracket density per character — high density + balance suggests a complete structural
/// unit (a whole function/block) rather than a fragment.
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

/// Fast line-level diff score: count matching lines in a sliding window (3 for an exact line
/// match, 2 for a trim-equal one), sliding the clip's line window across the target.
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

/// Best matching (start, end) line range in `file` for `clip`, if the overlap clears a floor
/// (at least a third of lines trim-matching).
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
    if best_score >= c_len { Some((best_start, (best_start + c_len).min(f_len))) } else { None }
}

/// Decide automatic paste mode from structure + overlap signals: a complete, balanced structural
/// unit with low overlap replaces; strong overlap with the existing target is a surgical patch;
/// otherwise replace is the safe default.
fn auto_mode(clip: &str, target: &str) -> PasteMode {
    let score = subsection_score(clip, target);
    let density = closure_density(clip);
    let balanced = closure_balanced(clip);
    if balanced && density > 0.015 && score < 10 { PasteMode::Replace }
    else if score > 20 { PasteMode::Patch }
    else { PasteMode::Replace }
}

/// Diff score as a percentage of the clip's own best-possible score (every line an exact match),
/// for the "diff-score: 82%" display the GTK original shows per candidate.
fn diff_score_pct(clip: &str, target: &str) -> u32 {
    let c_len = clip.lines().take(64).count();
    if c_len == 0 { return 0; }
    let max = c_len * 3;
    let got = subsection_score(clip, target);
    ((got as f64 / max as f64) * 100.0).round().clamp(0.0, 100.0) as u32
}

/// One clipboard entry via `wl-paste` (Wayland) or `xclip` (X11), whichever answers first.
fn read_clipboard() -> String {
    for cmd in &[&["wl-paste"][..], &["xclip", "-selection", "clipboard", "-o"][..]] {
        if let Ok(out) = std::process::Command::new(cmd[0]).args(&cmd[1..]).output() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if !s.is_empty() { return s; }
        }
    }
    String::new()
}

/// Clipboard HISTORY (not just the current entry) via KDE's klipper over qdbus, when available;
/// falls back to a single-entry history from the live clipboard everywhere else.
fn clipboard_history() -> Vec<String> {
    if let Ok(out) = std::process::Command::new("qdbus")
        .args(["org.kde.klipper", "/klipper", "org.kde.klipper.klipper.getClipboardHistory"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let items: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
        if !items.is_empty() { return items; }
    }
    let cur = read_clipboard();
    if cur.is_empty() { vec![] } else { vec![cur] }
}

// ── the tab ─────────────────────────────────────────────────────────────────────────────────────

pub struct Modify { history: Vec<String>, sel: usize, mode: PasteMode, target: String }
impl Default for Modify {
    fn default() -> Self {
        Self {
            history: Vec::new(), // populated by "Refresh clipboard history", not faked at startup
            sel: 0, mode: PasteMode::Auto,
            target: "// paste target buffer\n".into(),
        }
    }
}
impl Modify {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Paste mode:");
            for m in [PasteMode::Replace, PasteMode::Append, PasteMode::Patch, PasteMode::Auto] {
                ui.selectable_value(&mut self.mode, m, m.label());
            }
            ui.separator();
            if ui.button("◀ Prev").on_hover_text("Previous clipboard entry (..p)").clicked() && self.sel > 0 { self.sel -= 1; }
            if ui.button("Next ▶").on_hover_text("Next clipboard entry (..n)").clicked() && self.sel + 1 < self.history.len() { self.sel += 1; }
            if ui.button("Refresh clipboard history").clicked() {
                self.history = clipboard_history();
                self.sel = 0;
            }
        });
        ui.separator();
        ui.columns(2, |c| {
            c[0].label(RichText::new("Clipboard history").color(theme::DIM()).small());
            if self.history.is_empty() {
                c[0].label(RichText::new("(empty — click Refresh; needs wl-paste/xclip or klipper)").color(theme::DIM()).small().italics());
            }
            egui::ScrollArea::vertical().id_salt("mod_hist").show(&mut c[0], |ui| {
                for (i, h) in self.history.iter().enumerate() {
                    let first = h.lines().next().unwrap_or(h);
                    let pct = diff_score_pct(h, &self.target);
                    ui.selectable_value(&mut self.sel, i,
                        RichText::new(format!("{first}  [diff-score: {pct}%]")).monospace());
                }
            });
            c[1].label(RichText::new("Target").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("mod_target").show(&mut c[1], |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.target).id_salt("modify-self_target")
                    .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(8));
            });
        });
        ui.add_space(6.0);
        if ui.button("✦ Paste").clicked() {
            if let Some(entry) = self.history.get(self.sel).cloned() {
                let effective = if self.mode == PasteMode::Auto { auto_mode(&entry, &self.target) } else { self.mode };
                match effective {
                    PasteMode::Replace => self.target = entry,
                    PasteMode::Append => { self.target.push('\n'); self.target.push_str(&entry); }
                    PasteMode::Patch => {
                        // Surgical patch: splice over the best-matching range when one clears the
                        // floor, same as the GTK original's best_range — falling back to append
                        // (never silently dropping the clip) when nothing matches well enough.
                        if let Some((start, end)) = best_range(&entry, &self.target) {
                            let lines: Vec<&str> = self.target.lines().collect();
                            let mut out: Vec<&str> = lines[..start].to_vec();
                            out.push(entry.as_str());
                            out.extend(&lines[end..]);
                            self.target = out.join("\n");
                        } else {
                            self.target.push_str("\n# (patch: no matching range found — appended)\n");
                            self.target.push_str(&entry);
                        }
                    }
                    PasteMode::Auto => unreachable!("effective mode is never Auto"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_balance_and_density_agree_with_the_gtk_original_on_known_inputs() {
        assert!(closure_balanced("fn f() { let x = [1, 2]; }"));
        assert!(!closure_balanced("fn f() { let x = [1, 2];"));
        // A dense, balanced snippet scores higher than loose prose with no brackets at all.
        assert!(closure_density("fn f(a: (i32, i32)) -> [i32; 2] { [a.0, a.1] }") > closure_density("just some words with no code in them at all"));
    }

    #[test]
    fn subsection_score_finds_an_exact_window_and_ignores_a_disjoint_one() {
        let file = "line one\nline two\nline three\nline four\nline five\n";
        let clip_matching = "line two\nline three\n";
        let clip_foreign = "nothing here\nmatches at all\n";
        assert!(subsection_score(clip_matching, file) > subsection_score(clip_foreign, file));
        // Exact match hits the theoretical max (3 per line) and early-exits.
        assert_eq!(subsection_score(clip_matching, file), 2 * 3);
    }

    #[test]
    fn best_range_locates_the_matching_lines_and_refuses_a_weak_match() {
        let file = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        let clip = "beta\ngamma\n";
        assert_eq!(best_range(clip, file), Some((1, 3)));
        assert_eq!(best_range("totally unrelated text\nwith no overlap\n", file), None);
    }

    #[test]
    fn auto_mode_replaces_a_complete_unit_and_patches_a_strong_overlap() {
        // A whole, balanced function pasted where the target barely overlaps: replace.
        let whole_fn = "fn helper(x: i32) -> i32 {\n    let y = x + 1;\n    let z = (y, y);\n    z.0\n}\n";
        assert_eq!(auto_mode(whole_fn, "// empty target\n"), PasteMode::Replace);
        // Heavy line overlap: an 8-line clip appearing verbatim in the target scores 8*3=24,
        // clearing the >20 floor regardless of the clip's own balance/density — score this
        // high dominates auto_mode's first branch (which needs score<10) no matter what.
        // Verified against subsection_score directly before trusting the auto_mode expectation,
        // not assumed from eyeballing the strings.
        let clip = "line a\nline b\nline c\nline d\nline e\nline f\nline g\nline h\n";
        let target = format!("prefix line\n{clip}suffix line\n");
        assert_eq!(subsection_score(clip, &target), 24, "test setup: expected an exact 8-line match");
        assert_eq!(auto_mode(clip, &target), PasteMode::Patch);
    }

    #[test]
    fn diff_score_pct_is_100_for_an_exact_match_and_0_for_no_overlap_at_all() {
        let target = "one\ntwo\nthree\n";
        assert_eq!(diff_score_pct("one\ntwo\nthree\n", target), 100);
        assert_eq!(diff_score_pct("zzz\nyyy\nxxx\n", target), 0);
    }

    #[test]
    fn empty_clip_or_target_scores_zero_rather_than_panicking() {
        assert_eq!(subsection_score("", "some file content"), 0);
        assert_eq!(subsection_score("some clip", ""), 0);
        assert_eq!(best_range("", "file"), None);
        assert_eq!(diff_score_pct("", "file"), 0);
    }
}
