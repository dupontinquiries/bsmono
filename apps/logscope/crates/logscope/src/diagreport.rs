//! diagreport.rs — a bounded, sectioned diagnostic dump that is cheap to grep.
//!
//! Every debugging round in this project has followed the same shape: run, paste a log, grep for four
//! or five lines, discard the rest. The rest is not small — a single load prints two `stop=[...]` sets
//! of over a thousand token ids, and a tensor dump per refusal. Reading a raw log costs far more
//! attention than the facts in it are worth.
//!
//! So this writes a report with three properties, in order of how much they matter:
//!
//! 1. **Stable section markers.** Every section begins with `@@NAME` at the start of a line and nothing
//!    else does. `grep -A40 '@@PROFILE'` always works, on every report, forever.
//! 2. **Hard per-section line caps.** A section cannot grow without bound however long the run was.
//!    Truncation says how much it dropped, because a silently shortened list looks like a complete one.
//! 3. **A grep guide inside the file**, so a reader who has never seen one does not have to infer the
//!    format from the content.
//!
//! Field values are capped too: the `stop=[...]` sets are the single largest thing in a logscope log and
//! carry no diagnostic value beyond their length.

/// Sections, in the order they are written. The order is stable so a diff between two reports lines up.
pub const SECTIONS: &[&str] = &[
    "ENV", "ADAPTER", "LEVERS", "MODEL", "STAGES", "LOAD", "PROFILE", "SUBMITS", "GATES", "ERRORS", "TAIL",
];

/// Longest a single value may be before it is elided. The stop-token sets run past 8000 characters.
const MAX_VALUE: usize = 400;
/// Lines any one section may contribute.
const MAX_SECTION_LINES: usize = 60;

/// Shorten a long value, saying how much was dropped.
///
/// A silently truncated list reads as a complete one, and someone will eventually conclude the model has
/// four stop tokens.
pub fn elide(v: &str) -> String {
    if v.len() <= MAX_VALUE { return v.to_string(); }
    let head: String = v.chars().take(MAX_VALUE).collect();
    format!("{head}… [+{} chars elided]", v.len() - head.len())
}

#[derive(Clone, Debug, Default)]
pub struct Report { sections: Vec<(String, Vec<String>)> }

impl Report {
    pub fn new() -> Self { Self::default() }

    /// Append a line WITH a timestamp, so entries can be correlated against journalctl.
    ///
    /// Both clocks, deliberately. The wall clock is what journalctl stamps, so it is the only way to line
    /// a GPU watchdog entry up with what this process was doing; the monotonic offset is what survives an
    /// NTP step and makes durations between our own lines trustworthy. Either alone leaves a gap.
    pub fn push_at(&mut self, section: &str, line: impl Into<String>, wall_secs: u64, mono_ms: u64) {
        let stamped = format!("[{:>8}.{:03}s +{:>7}ms] {}",
            wall_secs, 0, mono_ms, elide(line.into().trim_end()));
        self.push_raw(section, stamped);
    }

    /// Append a line to a section, creating it if needed. Lines past the cap are counted, not stored.
    pub fn push(&mut self, section: &str, line: impl Into<String>) {
        let line = elide(line.into().trim_end());
        self.push_raw(section, line)
    }

    fn push_raw(&mut self, section: &str, line: String) {
        match self.sections.iter_mut().find(|(s, _)| s == section) {
            Some((_, v)) => v.push(line),
            None => self.sections.push((section.to_string(), vec![line])),
        }
    }

    /// A STAGE marker: a named phase with the time it took.
    ///
    /// Load is the phase most worth correlating — a watchdog entry during shard unification means
    /// something different from one during expert quantization, and a flat list of lines cannot tell you
    /// which was running.
    pub fn stage(&mut self, name: &str, ms: u64, detail: &str) {
        self.push_raw("STAGES", format!("{:>8} ms  {name:<28} {}", ms, elide(detail)));
    }

    /// Route a raw engine log line into whichever section it belongs to.
    ///
    /// Classification is by CONTENT rather than by the caller remembering to label it, because the caller
    /// is a log line that already exists in a dozen places and will not be updated.
    pub fn absorb(&mut self, line: &str) {
        let l = line.trim();
        if l.is_empty() { return; }
        let sec = if l.contains("adapter =") || l.contains("preflight") { "ADAPTER" }
            else if l.contains("non-default levers") || l.contains("lever") { "LEVERS" }
            else if l.contains("profile —") { "PROFILE" }
            else if l.contains("submit causes") || l.contains("submit batch") || l.contains("flush") { "SUBMITS" }
            else if l.contains("gate-") { "GATES" }
            else if l.to_lowercase().contains("fail") || l.to_lowercase().contains("error")
                 || l.contains("panic") { "ERRORS" }
            else if l.contains("loaded in") || l.contains("resident memory") || l.contains("quantiz") { "LOAD" }
            else if l.contains("d=") && l.contains("layers=") { "MODEL" }
            else { "TAIL" };
        self.push(sec, l);
    }

    /// Render the report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("### LOGSCOPE DIAGNOSTIC REPORT v1\n");
        out.push_str("### Sections start with '@@NAME' at the start of a line; nothing else does.\n");
        // The recipe is ANCHORED, because this guide itself mentions the markers by name — an unanchored
    // grep matches these instruction lines as well as the real sections.
    out.push_str("### To read one:  grep -A40 '^@@PROFILE' report.log\n");
        out.push_str("### To list all:  grep '^@@' report.log\n");
        out.push_str(&format!("### Sections: {}\n\n", SECTIONS.join(" ")));
        for name in SECTIONS {
            let Some((_, lines)) = self.sections.iter().find(|(s, _)| s == name) else { continue };
            if lines.is_empty() { continue; }
            out.push_str(&format!("@@{name}\n"));
            for l in lines.iter().take(MAX_SECTION_LINES) { out.push_str(l); out.push('\n'); }
            if lines.len() > MAX_SECTION_LINES {
                out.push_str(&format!("[+{} more lines in {name}, capped at {MAX_SECTION_LINES}]\n",
                    lines.len() - MAX_SECTION_LINES));
            }
            out.push('\n');
        }
        out
    }

    /// Sections that actually have content, for a caller that wants to say what it captured.
    pub fn populated(&self) -> Vec<&str> {
        self.sections.iter().filter(|(_, v)| !v.is_empty()).map(|(s, _)| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_token_set_is_elided_and_says_how_much_it_dropped() {
        // The single largest thing in a logscope log: over 1000 token ids, twice per load, carrying no
        // diagnostic value beyond their length. A SILENT truncation would be worse than none — someone
        // would eventually conclude the model has four stop tokens.
        let stop = format!("stop=[{}]", (0..1200).map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
        let e = elide(&stop);
        assert!(e.len() < stop.len() / 4);
        assert!(e.contains("chars elided"), "{e}");
        assert!(e.starts_with("stop=[0, 1, 2"), "the beginning is kept, which is the useful part");
    }

    #[test]
    fn every_section_marker_is_greppable_and_unique() {
        // The property the whole file exists for: `grep -A40 '@@PROFILE'` must work on every report
        // forever, so the marker must not collide with ordinary log text.
        let mut r = Report::new();
        r.absorb("wgpu-engine: [x] profile — 4256 submits (1064/tok), 193 polls");
        r.absorb("wgpu-engine: adapter = Apple M2 Ultra [Vulkan]");
        let out = r.render();
        assert!(out.contains("\n@@PROFILE\n"));
        assert!(out.contains("\n@@ADAPTER\n"));
        // the guide is IN the file, so a first-time reader need not infer the format
        assert!(out.contains("grep -A40 '^@@PROFILE'"), "the recipe must be anchored");
        // exactly one marker line per populated section
        assert_eq!(out.lines().filter(|l| l.starts_with("@@")).count(), 2);
    }

    #[test]
    fn lines_are_routed_by_CONTENT_not_by_the_caller_labelling_them() {
        // The caller is a log line that already exists in a dozen places and will not be updated to
        // label itself. Classification has to work on what is already printed.
        let mut r = Report::new();
        r.absorb("wgpu-engine: submit causes — no-frame 3527 (83%)");
        r.absorb("gate-moe-stream: live streaming decode is BIT-EXACT ✓");
        r.absorb("worker: load failed: architecture 'glm-dsa' isn't supported");
        let p = r.populated();
        assert!(p.contains(&"SUBMITS") && p.contains(&"GATES") && p.contains(&"ERRORS"), "{p:?}");
    }

    #[test]
    fn a_section_cannot_grow_without_bound() {
        // A report from a long run must stay readable, and the cap has to ANNOUNCE itself or the reader
        // will believe they have seen everything.
        let mut r = Report::new();
        for i in 0..500 { r.absorb(&format!("gate-x: check {i} ok")); }
        let out = r.render();
        let body = out.lines().filter(|l| l.starts_with("gate-x")).count();
        assert!(body <= 60, "capped, got {body}");
        assert!(out.contains("more lines in GATES, capped at"), "the cap announces itself");
    }

    #[test]
    fn section_order_is_stable_so_two_reports_diff_cleanly() {
        // Reports get compared across runs. Order following insertion would make an unrelated diff.
        let mut a = Report::new();
        a.absorb("wgpu-engine: [x] profile — 1 submits");
        a.absorb("wgpu-engine: adapter = X");
        let mut b = Report::new();
        b.absorb("wgpu-engine: adapter = X");
        b.absorb("wgpu-engine: [x] profile — 1 submits");
        let (ra, rb) = (a.render(), b.render());
        // Anchored to line-start: the grep GUIDE contains the literal "@@PROFILE" as an example, so an
        // unanchored search matches the header. That is exactly why the documented recipe is `^@@` —
        // and this test failing on the first run is what proved the anchor is load-bearing rather than
        // stylistic.
        assert!(ra.find("\n@@ADAPTER\n") < ra.find("\n@@PROFILE\n"));
        assert_eq!(ra, rb, "insertion order must not change the report");
    }

    #[test]
    fn an_empty_report_still_carries_its_grep_guide() {
        // A report with nothing captured must still teach the reader how to read the next one.
        let out = Report::new().render();
        assert!(out.contains("LOGSCOPE DIAGNOSTIC REPORT"));
        assert!(out.contains("grep '^@@'"));
        assert!(!out.contains("@@ENV"), "but no empty sections");
    }
}
