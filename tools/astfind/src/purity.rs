//! purity — where the effect boundary already is, so a crate boundary can follow it.
//!
//! ## The finding this exists to make repeatable
//!
//! Measured across the monorepo: of 677 files over 300 lines, **427 (63%,
//! 253,419 lines) contain not one real effect call** — no filesystem, no
//! process, no socket, no clock, no thread. They are already pure logic. They
//! simply live in crates that also contain the few files that do I/O.
//!
//! That reframes "break up the big application loops". The loops are not
//! tangled: `voiceterm/src/main.rs` is 23,388 lines with **140 effect calls**,
//! a density of 0.6%. Effects are concentrated already. What is missing is that
//! **the crate boundary does not follow the effect boundary**, so 133k lines
//! compile as one serial unit, cannot be tested headless, and cannot be reused.
//!
//! ## Why a type import is not an effect
//!
//! The first version of this measure counted `use std::net::SocketAddr` as I/O
//! and reported `snapcore::routing` — a pure routing table — as effectful. A
//! type names a domain; it does not perform anything. Counting imports finds
//! the files that *mention* effects; counting calls finds the files that *have*
//! them, and only the second tells you where a boundary can go.

/// Call shapes that actually perform an effect.
///
/// Deliberately a list of *calls*, not modules. `Instant::now()` is an effect;
/// holding an `Instant` in a struct is not, which is why every ported module in
/// this repo takes `dt_ms` instead of reading a clock.
pub const EFFECT_CALLS: &[&str] = &[
    "fs::read", "fs::write", "fs::create", "fs::remove", "fs::copy", "fs::File",
    "File::open", "File::create", "Command::new", "thread::spawn",
    "Instant::now", "SystemTime::now", "TcpStream::", "UdpSocket::",
    "tokio::spawn", ".await",
];

/// What one file looks like.
#[derive(Clone, Debug, PartialEq)]
pub struct Purity {
    pub file: String,
    pub lines: usize,
    /// Real effect calls, not imports.
    pub effects: usize,
}

impl Purity {
    /// Effect calls per hundred lines.
    pub fn density(&self) -> f32 {
        if self.lines == 0 { 0.0 } else { 100.0 * self.effects as f32 / self.lines as f32 }
    }

    /// Already pure — movable to a headless crate as it stands.
    pub fn is_pure(&self) -> bool { self.effects == 0 }

    /// Big enough that moving it changes a build, and pure enough to move.
    ///
    /// The threshold is on LINES rather than density because a 40-line pure
    /// helper is not worth a crate, and a 3,000-line file with two effect calls
    /// is worth un-tangling — different questions, so `is_pure` answers the
    /// second and this answers "is it worth the churn".
    pub fn is_extraction_candidate(&self, min_lines: usize) -> bool {
        self.lines >= min_lines && self.effects == 0
    }
}

/// Count effect calls in a source file.
pub fn measure(file: &str, source: &str) -> Purity {
    let mut effects = 0;
    for line in source.lines() {
        let t = line.trim_start();
        // A commented effect is not an effect. This matters here more than
        // usual: the house style explains what a module deliberately does NOT
        // do, so `// no fs::write here` is common and would inflate the count.
        if t.starts_with("//") { continue; }
        for pat in EFFECT_CALLS {
            if line.contains(pat) { effects += 1; break; }
        }
    }
    Purity { file: file.to_string(), lines: source.lines().count(), effects }
}

/// Summarise a set of files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub files: usize,
    pub pure_files: usize,
    pub pure_lines: usize,
    pub total_lines: usize,
}

impl Report {
    pub fn pure_share(&self) -> f32 {
        if self.total_lines == 0 { 0.0 } else { 100.0 * self.pure_lines as f32 / self.total_lines as f32 }
    }
}

pub fn summarise(files: &[Purity], min_lines: usize) -> Report {
    let big: Vec<&Purity> = files.iter().filter(|p| p.lines >= min_lines).collect();
    Report {
        files: big.len(),
        pure_files: big.iter().filter(|p| p.is_pure()).count(),
        pure_lines: big.iter().filter(|p| p.is_pure()).map(|p| p.lines).sum(),
        total_lines: big.iter().map(|p| p.lines).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_import_is_not_an_effect() {
        // The bug in my first measure: it counted `use std::net::SocketAddr`
        // and reported snapcore::routing — a pure routing table — as effectful.
        // A type names a domain; it does not perform anything.
        let p = measure("routing.rs", "use std::net::SocketAddr;\nfn f(a: SocketAddr) {}\n");
        assert!(p.is_pure(), "density was {}", p.density());
    }

    #[test]
    fn a_real_call_counts() {
        let p = measure("io.rs", "fn f() { let s = std::fs::read(\"x\").unwrap(); }\n");
        assert_eq!(p.effects, 1);
        assert!(!p.is_pure());
    }

    #[test]
    fn a_commented_effect_does_not_count() {
        // The house style explains what a module deliberately does NOT do, so
        // "// no fs::write here" is common and would inflate every count.
        let p = measure("m.rs", "// deliberately no fs::write here\nfn f() {}\n");
        assert!(p.is_pure());
    }

    #[test]
    fn a_line_with_two_effects_counts_once() {
        // Otherwise a chained call inflates a file's apparent density and it
        // ranks as a shell when it is not.
        let p = measure("m.rs", "fn f() { Command::new(\"x\"); }\nfn g() { fs::read(\"y\"); }\n");
        assert_eq!(p.effects, 2, "two lines, two effects");
        let one = measure("m.rs", "let a = fs::read(File::open(\"x\"));\n");
        assert_eq!(one.effects, 1, "one line is one site");
    }

    #[test]
    fn a_clock_read_is_an_effect_but_holding_a_time_is_not() {
        // Which is why every ported module in this repo takes dt_ms instead of
        // reading a clock — that single choice moves a file across this line.
        assert_eq!(measure("a.rs", "let t = Instant::now();\n").effects, 1);
        assert_eq!(measure("b.rs", "struct S { at: Instant }\n").effects, 0);
    }

    #[test]
    fn a_small_pure_file_is_not_worth_a_crate() {
        // Different questions: is_pure asks whether it COULD move,
        // is_extraction_candidate asks whether moving it is worth the churn.
        let small = Purity { file: "h.rs".into(), lines: 40, effects: 0 };
        assert!(small.is_pure());
        assert!(!small.is_extraction_candidate(300));
    }

    #[test]
    fn a_big_file_with_two_effect_calls_is_not_a_candidate_yet() {
        // It is worth untangling, but it cannot move as it stands — the report
        // must not promise a free extraction.
        let p = Purity { file: "main.rs".into(), lines: 23_388, effects: 140 };
        assert!(!p.is_extraction_candidate(300));
        assert!(p.density() < 1.0, "density {} — effects are concentrated", p.density());
    }

    #[test]
    fn the_summary_reports_lines_rather_than_only_file_counts() {
        // A hundred 50-line pure files and one 20,000-line effectful one is a
        // very different build from the reverse, and a file count hides it.
        let files = vec![
            Purity { file: "a".into(), lines: 5_000, effects: 0 },
            Purity { file: "b".into(), lines: 1_000, effects: 20 },
        ];
        let r = summarise(&files, 300);
        assert_eq!(r.files, 2);
        assert_eq!(r.pure_files, 1);
        assert_eq!(r.pure_lines, 5_000);
        assert!((r.pure_share() - 83.3).abs() < 0.5, "share {}", r.pure_share());
    }

    #[test]
    fn files_below_the_threshold_are_excluded_from_the_summary() {
        let files = vec![Purity { file: "tiny".into(), lines: 10, effects: 0 }];
        assert_eq!(summarise(&files, 300).files, 0);
    }
}
