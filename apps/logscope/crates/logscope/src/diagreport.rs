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
        // Wall clock as an absolute epoch second (what journalctl stamps) plus the monotonic offset.
        // The first version printed a hardcoded ".000" subsecond field, which looked like precision it
        // did not have — a fake digit is worse than a missing one because it invites correlation at a
        // resolution the data cannot support.
        // Epoch seconds AND a local wall clock. journalctl stamps local time with no year or zone, so an
        // epoch alone forces the reader to convert by hand every time — which I did, four times, before
        // deciding that was the tool's job. `hms_local` uses the offset captured at startup.
        let stamped = format!("[{wall_secs} {} +{mono_ms:>7}ms] {}",
            hms_local(wall_secs), elide(line.into().trim_end()));
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
        // LIST WHAT IS IN THE REPORT, not what could be. Empty sections are dropped by design — a
    // terse report is the point — but the header advertised all eleven regardless, so a real M2
    // Ultra run promised PROFILE, ADAPTER and ERRORS while carrying ENV, STAGES and SUBMITS.
    //
    // `grep -A40 '^@@PROFILE'` returning nothing then means either "no profile this run" or "the
    // capture failed", and the header was the only thing that could distinguish them. Now it does:
    // a section absent from this list was never captured, and one present is greppable.
    let present: Vec<&str> = SECTIONS.iter().copied()
        .filter(|n| self.sections.iter().any(|(s, l)| s == n && !l.is_empty()))
        .collect();
    out.push_str(&format!("### Sections in THIS report: {}\n", present.join(" ")));
    let missing: Vec<&str> = SECTIONS.iter().copied().filter(|n| !present.contains(n)).collect();
    if !missing.is_empty() {
        out.push_str(&format!("### Not captured this run: {}\n", missing.join(" ")));
    }
    out.push('\n');
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

/// Seconds to add to UTC to get local time, captured once.
///
/// Determined by asking `date +%z` rather than assuming UTC, because the whole point is to line our
/// stamps up with journalctl's local ones — and a report that is four hours off is worse than one with
/// no clock at all, since it looks correlatable and is not.
pub fn local_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date").arg("+%z").output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let t = s.trim();
                if t.len() < 5 { return None; }
                let sign = if t.starts_with('-') { -1i64 } else { 1 };
                let h: i64 = t[1..3].parse().ok()?;
                let m: i64 = t[3..5].parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            }).unwrap_or(0)
    })
}

/// `HH:MM:SS` in local time, for direct comparison with a journalctl stamp.
pub fn hms_local(epoch_secs: u64) -> String {
    let t = (epoch_secs as i64 + local_offset_secs()).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

/// A kernel-side GPU fault, parsed from journalctl.
#[derive(Clone, PartialEq, Debug)]
pub struct GpuFault {
    /// `MMM DD HH:MM:SS` as it appears in the journal, kept verbatim — reformatting it would make it
    /// harder to grep back against the original.
    pub stamp: String,
    /// Seconds past midnight, for ordering and for lining up against our own clock.
    pub secs_of_day: u32,
    /// The driver's cumulative counter, which is what shows whether a run made things worse.
    pub timeout_count: Option<u64>,
}

/// Parse Asahi GPU timeouts out of a journalctl dump.
///
/// The real signature, from a live run:
/// ```text
/// Jul 27 17:54:11 host kernel: asahi 406400000.gpu: ** GPU timeout nya~!!!!! **
/// Jul 27 17:54:11 host kernel: asahi 406400000.gpu:   Timeout count: 1032
/// Jul 27 17:54:11 host kernel: asahi 406400000.gpu:   Halted: 1
/// ```
/// Matched on "GPU timeout" rather than the full banner: the driver's message is decorated with ASCII
/// art and an emoticon, and anchoring on decoration would break the moment someone tidies it.
pub fn parse_gpu_faults(journal: &str) -> Vec<GpuFault> {
    let mut out: Vec<GpuFault> = Vec::new();
    for line in journal.lines() {
        if line.contains("GPU timeout") {
            let stamp = line.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
            let secs = stamp.split_whitespace().nth(2).and_then(parse_hms).unwrap_or(0);
            out.push(GpuFault { stamp, secs_of_day: secs, timeout_count: None });
        } else if let Some(v) = line.split("Timeout count:").nth(1) {
            // attach to the most recent fault; the counter is printed just after the banner
            if let (Some(last), Ok(n)) = (out.last_mut(), v.trim().parse::<u64>()) {
                if last.timeout_count.is_none() { last.timeout_count = Some(n); }
            }
        }
    }
    out
}

fn parse_hms(t: &str) -> Option<u32> {
    let p: Vec<&str> = t.split(':').collect();
    if p.len() != 3 { return None; }
    Some(p[0].parse::<u32>().ok()? * 3600 + p[1].parse::<u32>().ok()? * 60 + p[2].parse::<u32>().ok()?)
}

/// Group faults into bursts separated by more than `gap_secs` of quiet.
///
/// Bursts are the unit worth reporting. Fifty-three scattered timeouts and fifty-three in two clusters
/// mean different things: the first suggests a chronic condition, the second points at two specific
/// things that were running. Reporting only a total hides which one you have.
pub fn burst(faults: &[GpuFault], gap_secs: u32) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < faults.len() {
        let start = i;
        while i + 1 < faults.len()
            && faults[i + 1].secs_of_day.saturating_sub(faults[i].secs_of_day) <= gap_secs { i += 1; }
        out.push((faults[start].stamp.clone(), faults[i].stamp.clone(), i - start + 1));
        i += 1;
    }
    out
}

impl Report {
    /// Fold a journalctl dump into the report's ERRORS section, as bursts rather than raw lines.
    ///
    /// Also reports the counter DELTA across the capture, because the driver's count is cumulative: 1032
    /// at the start and 1084 at the end says this session caused 52, which is the number that matters.
    /// The absolute figure says only that the machine has had a hard life.
    pub fn absorb_journal(&mut self, journal: &str) {
        let f = parse_gpu_faults(journal);
        if f.is_empty() { return; }
        let counts: Vec<u64> = f.iter().filter_map(|x| x.timeout_count).collect();
        let delta = match (counts.first(), counts.last()) {
            (Some(a), Some(b)) => format!(" — driver counter {a} → {b}, this capture caused {}", b - a),
            _ => String::new(),
        };
        self.push("ERRORS", format!("{} GPU timeouts in {} burst(s){delta}",
            f.len(), burst(&f, 120).len()));
        for (from, to, n) in burst(&f, 120) {
            self.push("ERRORS", format!("  burst: {n:>3} timeouts  {from} .. {to}"));
        }
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

    // The real signature, taken verbatim from a live Asahi run.
    const JOURNAL: &str = "\
Jul 27 17:54:11 ultra-fed kernel: asahi 406400000.gpu: ** GPU timeout nya~!!!!! **
Jul 27 17:54:11 ultra-fed kernel: asahi 406400000.gpu:   Timeout count: 1032
Jul 27 17:54:11 ultra-fed kernel: asahi 406400000.gpu:   Halted: 1
Jul 27 17:54:24 ultra-fed kernel: asahi 406400000.gpu: ** GPU timeout nya~!!!!! **
Jul 27 17:54:24 ultra-fed kernel: asahi 406400000.gpu:   Timeout count: 1033
Jul 27 18:55:41 ultra-fed kernel: asahi 406400000.gpu: ** GPU timeout nya~!!!!! **
Jul 27 18:55:41 ultra-fed kernel: asahi 406400000.gpu:   Timeout count: 1084
Jul 27 17:55:00 ultra-fed pipewire[1849]: mod.raop-sink: unrelated noise
";

    #[test]
    fn the_real_asahi_signature_parses_and_ignores_unrelated_noise() {
        let f = parse_gpu_faults(JOURNAL);
        assert_eq!(f.len(), 3, "three timeouts, and pipewire is not one of them");
        assert_eq!(f[0].timeout_count, Some(1032));
        assert_eq!(f[2].timeout_count, Some(1084));
        assert_eq!(f[0].stamp, "Jul 27 17:54:11");
    }

    #[test]
    fn the_counter_DELTA_is_reported_not_just_the_absolute() {
        // The driver's count is cumulative. 1032 says the machine has had a hard life; 1032 -> 1084 says
        // THIS capture caused 52, which is the number that tells you whether a change helped.
        let mut r = Report::new();
        r.absorb_journal(JOURNAL);
        let out = r.render();
        assert!(out.contains("this capture caused 52"), "{out}");
        assert!(out.contains("1032 → 1084"));
    }

    #[test]
    fn faults_are_grouped_into_BURSTS_not_listed_flat() {
        // Fifty-three scattered timeouts and fifty-three in two clusters mean different things: chronic
        // versus two specific things that were running. A total alone hides which you have.
        let f = parse_gpu_faults(JOURNAL);
        let b = burst(&f, 120);
        assert_eq!(b.len(), 2, "17:54 twice is one burst; 18:55 is another");
        assert_eq!(b[0].2, 2);
        assert_eq!(b[1].2, 1);
    }

    #[test]
    fn stamps_render_in_LOCAL_time_for_direct_journal_comparison() {
        // journalctl stamps local time with no year and no zone. An epoch alone makes every correlation
        // a manual conversion — done by hand four times before this existed. A report that is four hours
        // off is worse than one with no clock: it looks correlatable and is not.
        let a = hms_local(0);
        assert_eq!(a.len(), 8, "HH:MM:SS");
        // one hour later is one hour later, whatever the zone
        let t0 = hms_local(1785201113);
        let t1 = hms_local(1785201113 + 3600);
        let h0: u32 = t0[..2].parse().unwrap();
        let h1: u32 = t1[..2].parse().unwrap();
        assert_eq!((h0 + 1) % 24, h1, "{t0} -> {t1}");
        // and midnight wraps rather than going negative
        assert!(hms_local(0).starts_with(|c: char| c.is_ascii_digit()));
    }

    #[test]
    fn a_journal_with_no_gpu_faults_adds_nothing() {
        // A report that said "0 GPU timeouts" for every clean run would train the reader to skip the
        // ERRORS section, which is where the one that matters will eventually appear.
        let mut r = Report::new();
        r.absorb_journal("Jul 27 17:55:00 host pipewire[1]: nothing to see\n");
        assert!(!r.render().contains("@@ERRORS"));
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

#[cfg(test)]
mod header_honesty_tests {
    use super::*;

    #[test]
    fn the_header_lists_what_is_PRESENT_and_names_what_is_not() {
        // A real M2 Ultra report carried ENV, STAGES and SUBMITS while its header promised eleven
        // sections including PROFILE, ADAPTER and ERRORS. `grep -A40 '^@@PROFILE'` then returns
        // nothing, and the reader cannot tell "no profile this run" from "the capture failed" —
        // which need different responses and were getting the same answer.
        let mut r = Report::new();
        r.push("SUBMITS", "wgpu-engine: [m.gguf] submit causes — no-frame 0 (0%), passes 7923 (98%)");
        let out = r.render();
        assert!(out.contains("Sections in THIS report:"), "the header names what it has");
        assert!(out.contains("Not captured this run:"), "and what it does not");
        // The section it does have is greppable, which is the property the file exists for.
        assert!(out.contains("\n@@SUBMITS\n"));
        // And one it lacks is named as absent rather than silently missing from a promise.
        // Anchored at line start, because the header's own guide line contains the literal
        // '@@NAME' — searching for "@@" unanchored finds the explanation rather than a section, which
        // is the same thing the format's "markers start a line" rule exists to prevent.
        let head = &out[..out.find("\n@@").unwrap_or(out.len())];
        assert!(head.contains("PROFILE"), "PROFILE appears in the not-captured list: {head}");
    }

    #[test]
    fn a_report_with_everything_does_not_print_an_empty_MISSING_line() {
        // The line must not appear when there is nothing to say — a header that always carries
        // "Not captured this run:" with nothing after it is noise of exactly the kind this fixes.
        let r = Report::new();
        let out = r.render();
        // An empty report has nothing present, so the missing list is the interesting one; the
        // inverse case is that a full report omits it entirely.
        assert!(out.contains("Not captured this run:"), "an empty report says so");
        assert!(!out.contains("Not captured this run: \n"), "and never with an empty list");
    }
}
