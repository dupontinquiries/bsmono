//! filewindow.rs — reading a file too large to read.
//!
//! `workspace_access` decides WHETHER a file may be read and `docroute` decides WHICH files. Neither
//! answers the question that actually comes up: **this file is 18000 lines and the budget is 2000 —
//! which 2000?**
//!
//! The strategy used by hand throughout this project is: locate a symbol, read a window around it,
//! repeat. It works. It has also produced every "I checked and it was fine" that turned out to be a
//! check of the wrong function — so the thing that matters is not the windowing, it is **saying what
//! was not read**.
//!
//! ## The failure this exists to prevent
//!
//! A reader given 200 lines out of 18000 with no indication of the ratio believes they have seen the
//! file. Four times this week that belief produced a confident wrong conclusion: two paths existed
//! and only one was in the window.

/// A contiguous run of lines to read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// 1-based, inclusive.
    pub start: usize,
    pub end: usize,
    /// Why this window was chosen, so a reader can judge whether the choice was sound.
    pub because: String,
}

impl Window {
    pub fn lines(&self) -> usize { self.end.saturating_sub(self.start) + 1 }
}

/// What to read, and what is being skipped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub windows: Vec<Window>,
    /// Lines in the file.
    pub total: usize,
    /// Lines the plan will actually show.
    pub shown: usize,
}

impl Plan {
    pub fn covers_everything(&self) -> bool { self.shown >= self.total }

    /// **The line that must appear above the excerpt.**
    ///
    /// A reader given 200 of 18000 lines with no ratio believes they saw the file. Stating it turns
    /// a silent partial read into an explicit one, which is the difference between "I checked" and
    /// "I checked these three places".
    pub fn caveat(&self) -> Option<String> {
        if self.covers_everything() { return None }
        Some(format!(
            "showing {} of {} lines ({}%) across {} window(s) — the rest was NOT read, and anything \
concluded from this excerpt is a claim about those windows rather than about the file",
            self.shown, self.total, self.shown * 100 / self.total.max(1), self.windows.len()))
    }
}

/// Lines of context to keep either side of a match.
///
/// **Twelve.** I briefly set this to 5 on the belief that `rg -C 5` was a calibration; it was an
/// upload-size limit for a different tool, and taking an arbitrary constraint for a considered choice
/// is its own lesson.
///
/// The default is a starting point, not a rule — see `contextwidth`, which picks a width from what
/// the read is FOR and adapts it from whether the reads are working.
pub const CONTEXT: usize = 12;

/// Plan a read.
///
/// `matches` are 1-based line numbers of interest — from a grep, a symbol index, whatever located
/// them. Overlapping windows are merged, because two windows sharing lines read as two separate
/// places and mislead about how scattered the evidence is.
pub fn plan(total: usize, matches: &[usize], budget_lines: usize) -> Plan {
    // A file that fits is read whole. Windowing a small file adds a caveat that is not true and
    // teaches the reader to discount the caveat when it is.
    if total <= budget_lines {
        return Plan {
            windows: vec![Window { start: 1, end: total.max(1),
                                   because: "the file fits the budget".into() }],
            total, shown: total,
        };
    }
    if matches.is_empty() {
        // No anchor. The head is the honest default — it holds the module docs and the imports, which
        // say what the file IS — but it is a weak choice and the reason field says so.
        let end = budget_lines.min(total);
        return Plan {
            windows: vec![Window { start: 1, end,
                because: "no match to anchor on; showing the head, which holds the module docs but \
says nothing about where the relevant code is".into() }],
            total, shown: end,
        };
    }
    let mut sorted = matches.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut windows: Vec<Window> = Vec::new();
    for &m in &sorted {
        let start = m.saturating_sub(CONTEXT).max(1);
        let end = (m + CONTEXT).min(total);
        match windows.last_mut() {
            // Merge rather than emit two: overlapping windows read as two separate places and
            // mislead about how scattered the evidence is.
            Some(w) if start <= w.end + 1 => {
                w.end = w.end.max(end);
                w.because = format!("{} matches", w.because.split(' ').next()
                    .and_then(|n| n.parse::<usize>().ok()).unwrap_or(1) + 1);
            }
            _ => windows.push(Window { start, end, because: "1 matches".into() }),
        }
    }
    // Budget: keep whole windows, in order, until the budget is spent. Truncating a window mid-item
    // is worse than dropping it — a half-read function invites a conclusion about the half.
    let mut shown = 0;
    let mut kept = Vec::new();
    for w in windows {
        if shown + w.lines() > budget_lines && !kept.is_empty() { break }
        shown += w.lines();
        kept.push(w);
    }
    Plan { windows: kept, total, shown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_FITS_is_read_whole_with_no_caveat() {
        // Windowing a small file adds a caveat that is not true, and a caveat that cries wolf gets
        // discounted when it matters.
        let p = plan(80, &[40], 2000);
        assert!(p.covers_everything());
        assert_eq!(p.caveat(), None);
        assert_eq!(p.windows[0], Window { start: 1, end: 80, because: "the file fits the budget".into() });
    }

    #[test]
    fn a_partial_read_STATES_the_ratio() {
        // The failure this module exists for: a reader given 200 of 18000 lines with no ratio
        // believes they saw the file, and four times this week that produced a confident wrong
        // conclusion because two paths existed and only one was in the window.
        let p = plan(18000, &[13880, 13952], 200);
        let c = p.caveat().unwrap();
        assert!(c.contains("of 18000 lines"));
        assert!(c.contains("was NOT read"));
        assert!(c.contains("a claim about those windows rather than about the file"));
    }

    #[test]
    fn OVERLAPPING_windows_are_merged() {
        // Two windows sharing lines read as two separate places and mislead about how scattered the
        // evidence is — which matters when the question is "how many sites does this bug touch".
        let p = plan(1000, &[100, 105, 110], 500);
        assert_eq!(p.windows.len(), 1, "{:?}", p.windows);
        assert_eq!(p.windows[0].start, 88);
        assert_eq!(p.windows[0].end, 122);
    }

    #[test]
    fn DISTANT_matches_stay_separate() {
        // The two Q4_K dequant sites were 72 lines apart and being separate is the finding.
        let p = plan(18000, &[13880, 13952], 500);
        assert_eq!(p.windows.len(), 2);
    }

    #[test]
    fn a_window_is_kept_WHOLE_or_dropped() {
        // Truncating mid-item is worse than dropping: a half-read function invites a conclusion about
        // the half, which is how "I checked that path" becomes wrong.
        // Budget 55 fits two 25-line windows and not a third — kept whole, not trimmed to fit.
        let p = plan(10000, &[100, 5000, 9000], 55);
        assert_eq!(p.windows.len(), 2, "whole windows, not three truncated ones");
        assert!(p.windows.iter().all(|w| w.lines() == 25), "none was trimmed");
    }

    #[test]
    fn with_NO_ANCHOR_the_head_is_shown_and_called_weak() {
        // The head holds the module docs, which say what the file IS — useful, and not evidence about
        // where the relevant code lives. The reason field has to admit that.
        let p = plan(5000, &[], 100);
        assert!(p.windows[0].because.contains("says nothing about where the relevant code is"));
    }

    #[test]
    fn the_first_line_is_never_below_ONE() {
        // A match at line 3 with 12 lines of context would underflow.
        let p = plan(1000, &[3], 500);
        assert_eq!(p.windows[0].start, 1);
    }
}
