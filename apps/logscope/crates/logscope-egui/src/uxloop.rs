//! uxloop — the UX audit as something a worker bee can call, and iterate against.
//!
//! `uikit_egui::uxaudit` finds mechanical faults on a screen. This is the part that makes it a
//! TOOL for an agent rather than a test helper: a named surface to audit, a report written
//! where the next pass can read it, and a comparison against the previous pass.
//!
//! ## The loop this is for
//!
//! 1. Audit a surface. 2. Read the findings. 3. Change something. 4. Audit again.
//! 5. Compare — did the change fix what it aimed at, and did it break anything else?
//!
//! Step 5 is why the previous report is persisted. An agent that only sees the current
//! findings cannot tell a fix from a reshuffle, and will report success whenever the count
//! drops — including when it removed the widget rather than fixing it.
//!
//! ## What an agent must not conclude from a clean report
//!
//! That the screen is good. The audit finds occluded text, blank panes, overflow, unreadable
//! contrast and shared ids. It has no opinion about whether a layout makes sense, whether the
//! wording is right, or whether the thing is worth building. `Report::describe` says so in its
//! empty case for exactly this reason.

use uikit_egui::uxaudit::{self, Region, Report};

/// Where a pass's findings are kept for the next one to compare against.
///
/// Under the state dir rather than the config dir: this is a working artefact of an agent
/// loop, not something anyone hand-edits, and it is expected to be stale or absent.
pub fn report_path(surface: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("logscope").join("uxaudit").join(format!("{}.txt", slug(surface)))
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "surface".into() } else { t }
}

/// A pass's result, in the shape an agent acts on.
#[derive(Debug, Clone)]
pub struct Pass {
    pub report: Report,
    /// Findings this pass introduced that the previous one did not have.
    pub introduced: Vec<uxaudit::Finding>,
    /// Findings the previous pass had that are now gone.
    pub fixed: Vec<uxaudit::Finding>,
    /// Was there a previous pass at all? A first run has nothing to compare against, and
    /// reporting "0 fixed, 12 introduced" for it would read as a catastrophe rather than a
    /// baseline.
    pub had_baseline: bool,
}

impl Pass {
    /// Did this pass actually improve things?
    ///
    /// Fixed something AND introduced nothing. Not "fewer findings": a pass that fixes two and
    /// adds three has a lower count in neither direction that matters, and a loop that
    /// declared victory on the count would happily delete a widget to make its number go down.
    pub fn is_progress(&self) -> bool {
        !self.fixed.is_empty() && self.introduced.is_empty()
    }

    /// What to tell the agent, in one block.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        if !self.had_baseline {
            s.push_str("first pass on this surface — nothing to compare against yet.\n");
        }
        s.push_str(&self.report.describe());
        if self.had_baseline {
            s.push_str(&format!(
                "\n\nsince the last pass: {} fixed, {} introduced{}",
                self.fixed.len(),
                self.introduced.len(),
                if self.is_progress() { " — progress" } else { "" }
            ));
            for f in &self.introduced {
                s.push_str(&format!("\n  NEW: {:?} at ({:.0},{:.0}): {}", f.kind, f.region.left(), f.region.top(), f.detail));
            }
        }
        s
    }
}

/// Audit a surface and compare it with the previous pass.
///
/// The report is written back only AFTER the comparison, so a pass sees the state before its
/// own run — writing first would make every pass compare against itself and report nothing
/// ever changes.
pub fn run(
    surface: &str,
    size: egui::Vec2,
    bg: egui::Color32,
    regions: &[Region],
    draw: impl FnMut(&egui::Context, egui::Color32),
) -> Pass {
    let report = uxaudit::audit(size, bg, regions, draw);
    let path = report_path(surface);
    let previous = std::fs::read_to_string(&path).ok().map(|t| parse(&t));
    let had_baseline = previous.is_some();
    let before = previous.unwrap_or_default();

    let introduced: Vec<_> = report.introduced(&before).into_iter().cloned().collect();
    let fixed: Vec<_> = report.fixed_since(&before).into_iter().cloned().collect();

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Best effort: an unwritable state dir must not fail the audit. Losing the baseline costs
    // the comparison on the next pass, which is worse than nothing but far better than an
    // audit that refuses to run.
    let _ = std::fs::write(&path, serialise(&report));

    Pass { report, introduced, fixed, had_baseline }
}

/// One id per line — the ids are all a comparison needs, and a format that cannot drift out
/// of step with the details it would otherwise have to re-parse.
pub fn serialise(r: &Report) -> String {
    r.findings.iter().map(|f| f.id.to_string()).collect::<Vec<_>>().join("\n")
}

fn parse(text: &str) -> Report {
    let findings = text
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .map(|id| uxaudit::Finding {
            kind: uxaudit::Kind::Blank,
            region: egui::Rect::NOTHING,
            detail: String::new(),
            id,
        })
        .collect();
    Report { findings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_baseline_round_trips_through_its_ids() {
        let r = Report {
            findings: vec![
                uxaudit::Finding::new(uxaudit::Kind::Blank, egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::Vec2::new(10.0, 10.0)), "a"),
                uxaudit::Finding::new(uxaudit::Kind::Spill, egui::Rect::from_min_size(egui::pos2(40.0, 0.0), egui::Vec2::new(10.0, 10.0)), "b"),
            ],
        };
        let back = parse(&serialise(&r));
        // The ids are all a comparison needs, and a format that cannot drift out of step with
        // details it would otherwise re-parse.
        assert_eq!(back.findings.len(), 2);
        assert_eq!(r.fixed_since(&back).len(), 0, "a round trip lost a finding");
        assert_eq!(r.introduced(&back).len(), 0, "a round trip invented one");

        // Junk in the file is skipped rather than fatal: this is a working artefact and it
        // will be stale, truncated or hand-poked at some point.
        assert_eq!(parse("12\nnot-a-number\n\n34").findings.len(), 2);
    }

    /// A loop that declared victory on the COUNT would happily delete a widget to make its
    /// number go down.
    #[test]
    fn progress_means_fixed_something_and_broke_nothing() {
        let f = |x: f32| uxaudit::Finding::new(uxaudit::Kind::Blank, egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::Vec2::new(10.0, 10.0)), "x");
        let p = Pass { report: Report::default(), introduced: vec![], fixed: vec![f(0.0)], had_baseline: true };
        assert!(p.is_progress());

        // Fixed two, added one: fewer findings overall, and not progress.
        let p = Pass { report: Report::default(), introduced: vec![f(10.0)], fixed: vec![f(0.0), f(20.0)], had_baseline: true };
        assert!(!p.is_progress(), "a net-negative count was called progress");

        // Changed nothing.
        let p = Pass { report: Report::default(), introduced: vec![], fixed: vec![], had_baseline: true };
        assert!(!p.is_progress());
    }

    /// A first run has nothing to compare against, and "0 fixed, 12 introduced" would read as
    /// a catastrophe rather than a baseline.
    #[test]
    fn a_first_pass_says_it_is_a_baseline() {
        let p = Pass { report: Report::default(), introduced: vec![], fixed: vec![], had_baseline: false };
        let d = p.describe();
        assert!(d.contains("first pass"), "{d}");
        assert!(!d.contains("introduced"), "a baseline should not report a diff: {d}");
    }

    /// Working artefact, not config: nobody hand-edits this and it is expected to be stale.
    #[test]
    fn the_baseline_lives_in_the_state_dir() {
        let p = report_path("Chat / right page");
        let s = p.to_string_lossy();
        assert!(s.contains("/logscope/uxaudit/"), "{s}");
        assert!(!s.contains("/.config/"), "an agent's scratch file is not config: {s}");
        assert!(s.ends_with("chat-right-page.txt"), "{s}");
    }
}
