//! sgladder — how far the subgroup mat-vec can be trusted on THIS driver, learned by trying and
//! remembered across a GPU hang.
//!
//! The subgroup decode kernel is the fastest mat-vec the engine has, and on Asahi/Honeykrisp it can
//! hang the GPU at large `n` (the 128256-wide output projection), which the firmware watchdog turns
//! into a dead GPU. The load-time self-check is a correctness check on a tiny shape, so it passes and
//! the hang still happens. The response so far was all-or-nothing: disable the kernel on that driver
//! entirely, which also gives up the every projection it handles fine.
//!
//! This makes it a bound instead of a switch. A ladder of widths is tried largest-last; the width
//! being attempted is written to disk BEFORE the attempt, so if the GPU dies mid-probe the next boot
//! reads "we were at n=N when this machine stopped answering" and permanently caps below it. That is
//! the only way to learn a hang safely: you cannot catch it, so you leave a note before you risk it.
//!
//! Pure logic here (ladder, persistence, the decision); the GPU call is the caller's.
use std::path::PathBuf;

/// Widths worth probing: the projections a decode actually runs, smallest first. The last entries
/// are the vocabulary-sized ones where the hang was seen.
pub const LADDER: [u32; 7] = [2048, 4096, 8192, 16384, 32768, 131072, 262144];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Use the subgroup kernel for every width.
    All,
    /// Use it only for `n <= max_n`; above that use the barrier-reduction kernel.
    UpTo { max_n: u32 },
    /// Never use it.
    Never,
}

impl Verdict {
    pub fn allows(&self, n: u32) -> bool {
        match self { Verdict::All => true, Verdict::UpTo { max_n } => n <= *max_n, Verdict::Never => false }
    }
    pub fn describe(&self) -> String {
        match self {
            Verdict::All => "subgroup mat-vec: all widths".into(),
            Verdict::UpTo { max_n } => format!("subgroup mat-vec: n <= {max_n} (larger widths use the barrier kernel)"),
            Verdict::Never => "subgroup mat-vec: disabled".into(),
        }
    }
}

/// The on-disk record, one line per state so a half-written file is still readable.
/// `attempting=N`  — a probe of width N was started and has not reported back.
/// `ok=N`          — width N completed and matched the reference.
/// `failed=N`      — width N returned a wrong result (not a hang).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record { pub ok_upto: Option<u32>, pub attempting: Option<u32>, pub failed_at: Option<u32> }

pub fn path_for(adapter: &str) -> PathBuf {
    let key: String = adapter.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let base = std::env::var("LOGSCOPE_CACHE_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")).join("logscope")
    });
    base.join(format!("sgladder-{key}.txt"))
}

pub fn parse(text: &str) -> Record {
    let mut r = Record::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let Ok(n) = v.trim().parse::<u32>() else { continue };
        match k.trim() {
            "ok" => r.ok_upto = Some(r.ok_upto.map_or(n, |o: u32| o.max(n))),
            "attempting" => r.attempting = Some(n),
            "failed" => r.failed_at = Some(r.failed_at.map_or(n, |o: u32| o.min(n))),
            _ => {}
        }
    }
    r
}

pub fn load(adapter: &str) -> Record { parse(&std::fs::read_to_string(path_for(adapter)).unwrap_or_default()) }

fn append(adapter: &str, line: &str) {
    let p = path_for(adapter);
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{line}");
        let _ = f.sync_all();   // the point of the note is that it survives a hard power cycle
    }
}

/// Called immediately BEFORE a probe of width `n`. Returns once the note is durable.
pub fn note_attempt(adapter: &str, n: u32) { append(adapter, &format!("attempting={n}")); }
pub fn note_ok(adapter: &str, n: u32) { append(adapter, &format!("ok={n}")); }
pub fn note_failed(adapter: &str, n: u32) { append(adapter, &format!("failed={n}")); }

/// What the record says to do now.
///
/// An `attempting` with no matching `ok` or `failed` is the signature of a hang: the process never
/// came back to write a result. That width and everything above it is refused permanently, and the
/// widths already proven stay available — the whole point of the ladder.
pub fn verdict(r: &Record, ladder_top: u32) -> Verdict {
    let hung = match (r.attempting, r.ok_upto, r.failed_at) {
        (Some(a), ok, failed) => {
            let acknowledged = ok.map_or(false, |o| o >= a) || failed.map_or(false, |f| f <= a);
            if acknowledged { None } else { Some(a) }
        }
        (None, _, _) => None,
    };
    let bad = [hung, r.failed_at].into_iter().flatten().min();
    match (bad, r.ok_upto) {
        // something broke at `b`: allow only what is proven below it
        (Some(b), Some(ok)) if ok < b => Verdict::UpTo { max_n: ok },
        (Some(_), _) => Verdict::Never,
        (None, Some(ok)) if ok >= ladder_top => Verdict::All,
        (None, Some(ok)) => Verdict::UpTo { max_n: ok },
        (None, None) => Verdict::Never,   // nothing proven yet
    }
}

/// The next width to probe, or None when the ladder is finished or blocked.
pub fn next_probe(r: &Record) -> Option<u32> {
    if matches!(verdict(r, *LADDER.last().unwrap()), Verdict::Never) && r.ok_upto.is_none() && r.attempting.is_some() { return None; }
    let blocked = [r.failed_at, r.attempting.filter(|a| r.ok_upto.map_or(true, |o| o < *a) && r.failed_at.is_none())].into_iter().flatten().min();
    LADDER.into_iter().find(|&n| r.ok_upto.map_or(true, |o| n > o) && blocked.map_or(true, |b| n < b))
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOP: u32 = 262144;

    #[test]
    fn nothing_proven_means_never() {
        assert_eq!(verdict(&Record::default(), TOP), Verdict::Never);
        assert_eq!(next_probe(&Record::default()), Some(2048));
    }

    #[test]
    fn a_full_ladder_unlocks_every_width() {
        let r = parse("ok=2048\nok=4096\nok=8192\nok=16384\nok=32768\nok=131072\nok=262144\n");
        assert_eq!(verdict(&r, TOP), Verdict::All);
        assert_eq!(next_probe(&r), None);
    }

    #[test]
    fn an_unanswered_attempt_is_read_as_a_hang_and_caps_below_it() {
        // the machine died while probing 131072; 32768 was already proven
        let r = parse("ok=2048\nok=8192\nok=32768\nattempting=131072\n");
        assert_eq!(verdict(&r, TOP), Verdict::UpTo { max_n: 32768 });
        let v = verdict(&r, TOP);
        assert!(v.allows(4096) && v.allows(32768) && !v.allows(131072) && !v.allows(262144));
        assert_eq!(next_probe(&r), None, "the ladder stops: nothing between 32768 and the width that hung");
    }

    #[test]
    fn an_answered_attempt_is_not_a_hang() {
        let r = parse("attempting=8192\nok=8192\n");
        assert_eq!(verdict(&r, TOP), Verdict::UpTo { max_n: 8192 });
        assert_eq!(next_probe(&r), Some(16384));
        let f = parse("ok=4096\nattempting=8192\nfailed=8192\n");
        assert_eq!(verdict(&f, TOP), Verdict::UpTo { max_n: 4096 }, "a wrong answer caps like a hang, without the reboot");
        assert_eq!(next_probe(&f), None);
    }

    #[test]
    fn a_hang_on_the_very_first_rung_disables_it_entirely() {
        let r = parse("attempting=2048\n");
        assert_eq!(verdict(&r, TOP), Verdict::Never);
        assert!(!verdict(&r, TOP).allows(1));
    }

    #[test]
    fn a_partial_write_is_still_readable() {
        // a power cut mid-line must not lose the earlier lines
        let r = parse("ok=2048\nok=4096\nattempt");
        assert_eq!(r.ok_upto, Some(4096));
        assert_eq!(verdict(&r, TOP), Verdict::UpTo { max_n: 4096 });
    }

    #[test]
    fn the_record_survives_a_round_trip_through_a_file() {
        let dir = std::env::temp_dir().join(format!("sgladder-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("LOGSCOPE_CACHE_DIR", &dir);
        let adapter = "Apple M2 Ultra (Honeykrisp)";
        let _ = std::fs::remove_file(path_for(adapter));
        note_ok(adapter, 2048);
        note_attempt(adapter, 4096);
        let r = load(adapter);
        assert_eq!(r.ok_upto, Some(2048));
        assert_eq!(verdict(&r, TOP), Verdict::UpTo { max_n: 2048 }, "an in-flight attempt caps until it reports");
        note_ok(adapter, 4096);
        assert_eq!(verdict(&load(adapter), TOP), Verdict::UpTo { max_n: 4096 });
        std::env::remove_var("LOGSCOPE_CACHE_DIR");
    }
}

/// The runbook the local model follows (`LOCAL-AI-PROBING.md`) names gates and environment knobs. A
/// renamed flag there is worse than a missing one: the model runs the command, gets the default, and
/// reports a number as though it measured something. This test fails when the doc drifts.
#[cfg(test)]
mod runbook_tests {
    fn doc() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/LOCAL-AI-PROBING.md")).expect("LOCAL-AI-PROBING.md")
    }
    fn sources() -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        std::fs::read_dir(dir).unwrap().flatten()
            .filter(|e| e.path().extension().map(|x| x == "rs").unwrap_or(false))
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn every_gate_the_runbook_tells_the_model_to_run_exists() {
        let (d, src) = (doc(), sources());
        let mut missing = Vec::new();
        for tok in d.split_whitespace() {
            let t = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if !t.starts_with("--gate-") && t != "--bench" { continue; }
            if !src.contains(&format!("\"{t}\"")) { missing.push(t.to_string()); }
        }
        missing.sort(); missing.dedup();
        assert!(missing.is_empty(), "the runbook names gates that do not exist: {missing:?}");
    }

    #[test]
    fn every_env_knob_the_runbook_sets_is_actually_read() {
        let (d, src) = (doc(), sources());
        let mut missing = Vec::new();
        for line in d.lines() {
            // Some knobs are read by the APP (logscope-cli / logscope-egui), not by this crate; the
            // runbook marks those by invoking the app on the same line. Only engine commands are
            // this crate's to guarantee.
            if line.contains("logscope-cli") || line.contains("logscope-egui") { continue; }
            for tok in line.split(|c: char| c.is_whitespace() || c == '`' || c == '(' || c == ')') {
                let name = tok.split('=').next().unwrap_or("");
                if !name.starts_with("LOGSCOPE_") { continue; }
                // only knobs the doc tells the model to SET (they appear as NAME=value)
                if !tok.contains('=') { continue; }
                if !src.contains(&format!("\"{name}\"")) { missing.push(name.to_string()); }
            }
        }
        missing.sort(); missing.dedup();
        assert!(missing.is_empty(), "the runbook sets knobs this crate never reads: {missing:?}");
    }
}
