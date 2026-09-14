//! completions — what can this monorepo be asked to do?
//!
//! 72 binary targets and 157 shell scripts, and the daily friction is
//! not remembering a flag -- it is remembering that the script exists
//! and what it was called.
//!
//! ## Why not clap_complete
//!
//! Because it would cover a tenth of the surface. Counted: 8 crates
//! depend on clap and 199 files parse `env::args` by hand. Converting
//! 199 files to clap so that a completion generator can read them is a
//! rewrite justified by tab-completion, which is the tail wagging the
//! dog. `clap_complete` is still the right answer FOR THOSE EIGHT, and
//! this defers to it where it applies.
//!
//! ## What tools already tell you
//!
//! Everything here is scraped from what the tool already says about
//! itself, so nothing has to be maintained twice:
//!
//!   SHELL SCRIPTS   the header comment. `build/build.sh` opens with
//!                   four usage lines listing `--only`, `--target` and
//!                   `--run` -- better documentation than most argument
//!                   parsers produce, and already written.
//!   RUST BINARIES   the `[[bin]]` name, plus `--help` output when the
//!                   binary can be run.
//!   EVERYTHING      its path and its one-line description, which is
//!                   what makes "which script was that" answerable.
//!
//! ## What it will not do
//!
//! Guess flags. A completion offering `--dry-run` to a script that has
//! no such flag is worse than offering nothing: it reads as
//! documentation, and the person believes it. Anything not found in
//! the tool's own text is absent.

use std::collections::BTreeMap;

/// What a tool is FOR, which decides whether completing it helps.
///
/// Completing a script blesses its interface. If a script ought to be
/// split into steps or ported to Rust, tab-completion makes its
/// current shape harder to change -- people build muscle memory on
/// flags that were meant to be temporary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A human is told about this: it appears in prose documentation.
    /// Worth completing.
    EntryPoint,
    /// Only another script calls it. It is a STEP in something else,
    /// and a person typing its name is usually lost. Not completed.
    Step,
    /// Called by scripts AND documented. Both, and completed.
    Both,
    /// Neither documented nor called. Possibly dead; named rather than
    /// completed, because a completion for a script nobody runs is
    /// worse than a question about why it is there.
    Orphan,
}

impl Role {
    /// Should this get a completion?
    pub fn worth_completing(self) -> bool {
        matches!(self, Role::EntryPoint | Role::Both)
    }
    pub fn name(self) -> &'static str {
        match self { Role::EntryPoint => "entry point", Role::Step => "a step in something else",
                     Role::Both => "entry point, also composed", Role::Orphan => "orphan" }
    }
}

/// Classify from what refers to it. `in_docs` is how many prose files
/// mention it; `in_scripts` how many other scripts invoke it.
pub fn role_of(in_docs: usize, in_scripts: usize) -> Role {
    match (in_docs > 0, in_scripts > 0) {
        (true, false) => Role::EntryPoint,
        (true, true) => Role::Both,
        (false, true) => Role::Step,
        (false, false) => Role::Orphan,
    }
}

/// Should this be ported before it is given a stable CLI surface?
///
/// Some scripts have grown an interface that bash is the wrong place
/// for. Completing those first is backwards: it fixes the interface in
/// place at exactly the moment it should be moving. These are the
/// tells, and the recommendation is advice rather than a refusal --
/// the person maintaining it knows things this does not.
pub fn port_first(t: &Tool, lines: usize) -> Option<String> {
    if lines > 300 && t.flags.len() >= 4 {
        return Some(format!(
            "{} lines with {} flags. That is a program written in bash. Completing it now              fixes the interface in place at the moment it should be moving to Rust or              splitting into steps.", lines, t.flags.len()));
    }
    if t.flags.len() >= 8 {
        return Some(format!(
            "{} flags. An interface that wide wants a real argument parser, and              clap_complete would then generate this for free.", t.flags.len()));
    }
    if lines > 500 {
        return Some(format!("{lines} lines. Worth splitting into steps before its                              interface is blessed."));
    }
    None
}

/// Where a tool's interface was learned from, so a reader can judge
/// how much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Learned {
    /// Parsed from the script's own header comment.
    Header,
    /// Captured from `--help`.
    Help,
    /// Only the name is known.
    NameOnly,
    /// The crate uses clap; defer to `clap_complete`.
    Clap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub flag: String,
    /// What it does, if the source said. Never invented.
    pub about: Option<String>,
    /// Does it take a value? Only set when the source made that plain.
    pub takes_value: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// What you type.
    pub name: String,
    /// Where it lives, so a completion can show it and a person can
    /// find it.
    pub path: String,
    pub about: Option<String>,
    pub flags: Vec<Flag>,
    pub subcommands: Vec<String>,
    pub learned: Learned,
    pub role: Role,
}

impl Tool {
    /// A tool with no flags and no description is still worth
    /// completing -- the NAME is the thing people forget.
    pub fn useful(&self) -> bool { !self.name.is_empty() }
}

/// Read a shell script's interface out of its header.
///
/// The header is the block of `#` comments before the first line of
/// code. Scripts here use it properly -- `build/build.sh` documents
/// four invocations with their flags -- and it is already maintained,
/// because it is what the author reads when they come back.
pub fn from_shell(path: &str, source: &str) -> Tool {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let mut about = None;
    let mut flags: BTreeMap<String, Flag> = BTreeMap::new();
    let mut subcommands = Vec::new();
    let mut in_header = true;
    for line in source.lines() {
        let t = line.trim();
        if t.starts_with("#!") { continue; }
        if in_header && !t.starts_with('#') && !t.is_empty() { in_header = false; }
        if in_header {
            let body = t.trim_start_matches('#').trim();
            if body.is_empty() { continue; }
            // the first prose line that is not a usage example is the
            // description
            if about.is_none() && !body.contains("--") && !body.starts_with(&name) {
                let d = body.trim_start_matches(&format!("{name} —"))
                            .trim_start_matches(&format!("{name} -"))
                            .trim().to_string();
                if d.len() > 8 { about = Some(d); }
            }
            // ONLY USAGE LINES, NOT PROSE. The header says "or RUNS
            // it (--run). Real execution is..." and reading that as
            // usage made `--run` take a value of "Real". A usage line
            // invokes the tool; a prose line mentions it.
            if is_usage(body, &name) { collect_flags(body, &mut flags); }
            else { note_flag_names(body, &mut flags); }
        } else {
            // subcommands appear as case arms: `plan|run)` or `"build")`
            if let Some(arm) = case_arm(t) {
                for a in arm { if !subcommands.contains(&a) { subcommands.push(a); } }
            }
        }
    }
    // the description may be on the shebang-following line as `name — x`
    if about.is_none() {
        about = source.lines().nth(1).map(|l| l.trim_start_matches('#').trim())
            .filter(|l| l.len() > 8)
            .map(|l| l.split_once('—').or_else(|| l.split_once(" - "))
                 .map(|(_, d)| d.trim().to_string()).unwrap_or_else(|| l.to_string()));
    }
    Tool {
        name, path: path.to_string(), about,
        flags: flags.into_values().collect(),
        subcommands,
        learned: Learned::Header,
        // callers set this once they know what refers to the script;
        // a scraper reading one file cannot tell
        role: Role::Orphan,
    }
}

/// Does this line show the tool being INVOKED, rather than discussed?
///
/// The distinction decides whether the word after a flag is its value
/// or just the next word of a sentence.
fn is_usage(line: &str, name: &str) -> bool {
    let t = line.trim();
    t.starts_with(name) || t.contains(&format!("/{name}"))
        || t.split_whitespace().next().map(|w| w.ends_with(".sh")).unwrap_or(false)
}

/// A flag seen in prose: record that it EXISTS, but claim nothing
/// about whether it takes a value.
fn note_flag_names(line: &str, into: &mut BTreeMap<String, Flag>) {
    for w in line.split_whitespace() {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if !w.starts_with("--") || w.len() < 4 { continue; }
        if !w[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { continue; }
        into.entry(w.to_string()).or_insert(Flag {
            flag: w.to_string(), about: None, takes_value: false });
    }
}

/// Pull `--flag` tokens out of a line of prose or usage.
fn collect_flags(line: &str, into: &mut BTreeMap<String, Flag>) {
    let bytes: Vec<&str> = line.split_whitespace().collect();
    for (i, w) in bytes.iter().enumerate() {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if !w.starts_with("--") || w.len() < 4 { continue; }
        if !w[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { continue; }
        // A VALUE ONLY IF THE LINE SHOWS ONE. `--only snapclone` takes
        // a value; `--run` does not. Guessing this wrong makes the
        // completion insert a space and wait, or not wait when it
        // should.
        let takes_value = bytes.get(i + 1)
            .map(|n| !n.starts_with('-') && !n.starts_with('#')
                  && n.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false))
            .unwrap_or(false);
        // an inline comment after the example is the description
        let about = line.split_once('#').map(|(_, c)| c.trim().to_string())
            .filter(|c| !c.is_empty() && c.len() < 80);
        into.entry(w.to_string())
            .and_modify(|f| { if !f.takes_value { f.takes_value = takes_value; } })
            .or_insert(Flag { flag: w.to_string(), about, takes_value });
    }
}

/// `plan|run)` or `"build")` -> the arm names.
fn case_arm(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.ends_with(')') || t.contains('(') { return None; }
    let body = t.trim_end_matches(')').trim();
    if body.is_empty() || body == "*" { return None; }
    let parts: Vec<String> = body.split('|')
        .map(|p| p.trim().trim_matches(['"', '\'']).to_string())
        .filter(|p| !p.is_empty() && p != "*"
                 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .collect();
    if parts.is_empty() { None } else { Some(parts) }
}

/// Read a binary's interface from its `--help`.
pub fn from_help(name: &str, path: &str, help: &str) -> Tool {
    let mut flags: BTreeMap<String, Flag> = BTreeMap::new();
    let mut subcommands = Vec::new();
    let mut in_subs = false;
    for line in help.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if lower.starts_with("commands:") || lower.starts_with("subcommands:") {
            in_subs = true; continue;
        }
        if lower.starts_with("options:") || lower.starts_with("flags:") { in_subs = false; }
        if in_subs && !t.is_empty() && !t.starts_with('-') {
            if let Some(w) = t.split_whitespace().next() {
                if w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    subcommands.push(w.to_string());
                }
            }
        }
        collect_flags(t, &mut flags);
    }
    // SKIP THE VERSION BANNER. `av1tuner 0.1` is the first non-empty
    // line of almost every --help and it is not a description.
    let about = help.lines().map(|l| l.trim()).find(|t| {
        !t.is_empty() && !t.starts_with('-') && !t.to_lowercase().starts_with("usage")
            && !looks_like_version(t, name)
    }).map(|l| l.to_string()).filter(|l| l.len() > 8);
    Tool { name: name.into(), path: path.into(), about,
           flags: flags.into_values().collect(), subcommands, learned: Learned::Help,
           role: Role::EntryPoint }
}

/// `av1tuner 0.1` -- the banner, not a description.
fn looks_like_version(line: &str, name: &str) -> bool {
    let w: Vec<&str> = line.split_whitespace().collect();
    w.len() <= 2 && w.first().map(|f| f.eq_ignore_ascii_case(name)).unwrap_or(false)
}

/// Emit a zsh completion for one tool.
///
/// `_arguments` rather than a static word list, so zsh handles the
/// value-taking flags and the descriptions show in the menu.
pub fn zsh_for(t: &Tool) -> String {
    if t.learned == Learned::Clap {
        return format!(
            "# {}: uses clap -- generate with clap_complete instead of this\n", t.name);
    }
    let mut s = format!("#compdef {}\n", t.name);
    if let Some(a) = &t.about { s.push_str(&format!("# {a}\n")); }
    s.push_str(&format!("_{}() {{\n  _arguments -s \\\n", ident(&t.name)));
    for f in &t.flags {
        let desc = f.about.clone().unwrap_or_default().replace(['\'', '"', ':'], " ");
        if f.takes_value {
            s.push_str(&format!("    '{}=[{}]:value:' \\\n", f.flag, desc));
        } else {
            s.push_str(&format!("    '{}[{}]' \\\n", f.flag, desc));
        }
    }
    if t.subcommands.is_empty() {
        s.push_str("    '*:file:_files'\n}\n");
    } else {
        let words = t.subcommands.join(" ");
        s.push_str(&format!("    '1:command:({words})' \\\n    '*:file:_files'\n}}\n"));
    }
    s.push_str(&format!("_{} \"$@\"\n", ident(&t.name)));
    s
}

fn ident(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// One completion that knows every tool in the repo by name.
///
/// This is the part that earns its keep. Nobody remembers that the
/// thing they want is `maintenance/tools/verify-bundle.sh`; they
/// remember it had "bundle" in it. A single command that completes
/// across all 229 answers that, and the per-tool flag completions are
/// a bonus on top.
pub fn zsh_index(tools: &[Tool], command: &str) -> String {
    let mut s = format!("#compdef {command}\n_{}() {{\n  local -a tools\n  tools=(\n",
                        ident(command));
    for t in tools {
        let d = t.about.clone().unwrap_or_else(|| t.path.clone())
            .replace(['\'', '"', ':'], " ");
        let d: String = d.chars().take(70).collect();
        s.push_str(&format!("    '{}:{}'\n", t.name.replace(':', " "), d.trim()));
    }
    s.push_str("  )\n  _describe -t tools 'tool' tools\n}\n");
    s.push_str(&format!("_{} \"$@\"\n", ident(command)));
    s
}

/// What was found, and how well.
pub fn survey(tools: &[Tool]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for t in tools {
        *by.entry(match t.learned {
            Learned::Header => "from a script header",
            Learned::Help => "from --help",
            Learned::Clap => "clap (use clap_complete)",
            Learned::NameOnly => "name only",
        }).or_default() += 1;
    }
    let with_flags = tools.iter().filter(|t| !t.flags.is_empty()).count();
    let mut s = format!("{} tool(s):\n", tools.len());
    for (k, v) in by { s.push_str(&format!("  {v:>4}  {k}\n")); }
    s.push_str(&format!("  {with_flags:>4}  have flags worth completing\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real header of build/build.sh.
    const BUILD_SH: &str = r#"#!/usr/bin/env bash
# build/build.sh — cross-target build orchestrator.
# Reads manifest/projects.toml + targets.toml, expands to (project × target),
# and either PRINTS THE PLAN (default) or RUNS it (--run). Real execution is
# host-target cargo builds; cross targets and C++/cmake/hybrid projects print
# what they'd run (they need cross sysroots / Qt6 / cmake deps — hardware stage).
# Flatpak generation: see build/flatpak.sh.
#   build/build.sh                      # plan, all projects/targets
#   build/build.sh --only snapclone     # one project
#   build/build.sh --target phone       # one target id
#   build/build.sh --run --only peek    # actually build (host cargo only)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN=0; ONLY=""; TGT=""
case "$1" in
  plan|run)
    echo hi
    ;;
  *)
    ;;
esac
"#;

    #[test]
    fn a_script_documents_its_own_flags_in_its_header() {
        // better documentation than most argument parsers produce, and
        // already written
        let t = from_shell("build/build.sh", BUILD_SH);
        assert_eq!(t.name, "build.sh");
        let names: Vec<&str> = t.flags.iter().map(|f| f.flag.as_str()).collect();
        assert!(names.contains(&"--only"), "{names:?}");
        assert!(names.contains(&"--target"), "{names:?}");
        assert!(names.contains(&"--run"), "{names:?}");
        assert_eq!(t.learned, Learned::Header);
        assert!(t.about.as_deref().unwrap_or("").contains("cross-target"),
                "{:?}", t.about);
    }

    #[test]
    fn a_flag_takes_a_value_only_when_the_usage_shows_one() {
        // guessing wrong makes the completion insert a space and wait,
        // or not wait when it should
        let t = from_shell("build/build.sh", BUILD_SH);
        let f = |n: &str| t.flags.iter().find(|f| f.flag == n).unwrap();
        assert!(f("--only").takes_value, "`--only snapclone` shows a value");
        assert!(f("--target").takes_value, "`--target phone` shows a value");
        assert!(!f("--run").takes_value, "`--run --only peek` does not");
    }

    #[test]
    fn subcommands_come_from_the_case_block() {
        let t = from_shell("build/build.sh", BUILD_SH);
        assert!(t.subcommands.contains(&"plan".to_string()), "{:?}", t.subcommands);
        assert!(t.subcommands.contains(&"run".to_string()), "{:?}", t.subcommands);
        assert!(!t.subcommands.iter().any(|s| s == "*"), "the catch-all is not a command");
        // and a line that merely ends in a paren is not a case arm
        assert!(case_arm("ROOT=\"$(cd ..)\"").is_none());
        assert!(case_arm("*)").is_none());
    }

    #[test]
    fn nothing_is_invented_for_a_script_that_says_nothing() {
        // a completion offering --dry-run to a script with no such flag
        // reads as documentation, and the person believes it
        let bare = "#!/bin/sh\nrsync -a \"$1\" \"$2\"\n";
        let t = from_shell("tools/sync.sh", bare);
        assert!(t.flags.is_empty(), "{:?}", t.flags);
        assert!(t.subcommands.is_empty());
        // but the NAME is still worth completing -- that is the thing
        // people forget
        assert!(t.useful());
        assert_eq!(t.name, "sync.sh");
        let z = zsh_for(&t);
        assert!(z.contains("#compdef sync.sh"));
        assert!(z.contains("_files"), "it still completes paths: {z}");
    }

    #[test]
    fn help_output_yields_flags_and_subcommands() {
        let help = "av1tuner 0.1\nBatch AV1 tuner for video libraries\n\n\
                    Usage: av1tuner [OPTIONS] <COMMAND>\n\n\
                    Commands:\n  sweep    Measure the crf curve\n  \
                    encode   Run the batch\n\n\
                    Options:\n  --crf <CRF>    Quality target\n  --dry-run    Plan only\n";
        let t = from_help("av1tuner", "apps/av1tuner", help);
        assert!(t.subcommands.contains(&"sweep".to_string()), "{:?}", t.subcommands);
        assert!(t.subcommands.contains(&"encode".to_string()));
        let f = |n: &str| t.flags.iter().find(|f| f.flag == n);
        assert!(f("--crf").is_some(), "{:?}", t.flags);
        assert!(f("--dry-run").is_some());
        assert!(t.about.as_deref().unwrap_or("").contains("Batch AV1"), "{:?}", t.about);
    }

    #[test]
    fn the_zsh_it_emits_is_shaped_like_zsh() {
        let t = from_shell("build/build.sh", BUILD_SH);
        let z = zsh_for(&t);
        assert!(z.starts_with("#compdef build.sh"), "{z}");
        assert!(z.contains("_arguments -s"), "{z}");
        // a value-taking flag gets the `=` form so zsh waits for one
        assert!(z.contains("'--only=["), "{z}");
        // a boolean does not
        assert!(z.contains("'--run["), "{z}");
        assert!(z.contains("'1:command:(") , "subcommands offered first: {z}");
        // the function name is a legal identifier even from a dotted
        // script name
        assert!(z.contains("_build_sh()"), "{z}");
        assert!(!z.contains("_build.sh()"), "a dot is not legal in a zsh function name");
    }

    #[test]
    fn clap_crates_are_deferred_to_clap_complete_rather_than_scraped() {
        // clap_complete is the right answer for those eight; scraping
        // their --help would produce a worse version of what the
        // library already emits perfectly
        let t = Tool { name: "gitmind".into(), path: "apps/gitmind".into(), about: None,
                       flags: vec![], subcommands: vec![], learned: Learned::Clap, role: Role::EntryPoint };
        let z = zsh_for(&t);
        assert!(z.contains("uses clap"), "{z}");
        assert!(z.contains("clap_complete"), "{z}");
        assert!(!z.contains("_arguments"), "it does not emit a worse one: {z}");
    }

    #[test]
    fn one_index_completes_every_tool_by_name() {
        // nobody remembers `maintenance/tools/verify-bundle.sh`; they
        // remember it had "bundle" in it
        let tools = vec![
            from_shell("build/build.sh", BUILD_SH),
            from_shell("maintenance/tools/verify-bundle.sh",
                       "#!/bin/sh\n# verify-bundle.sh — check a git bundle applies cleanly\n"),
        ];
        let idx = zsh_index(&tools, "mono");
        assert!(idx.contains("#compdef mono"));
        assert!(idx.contains("build.sh:"), "{idx}");
        assert!(idx.contains("verify-bundle.sh:"), "{idx}");
        assert!(idx.contains("check a git bundle"), "descriptions carry: {idx}");
        assert!(idx.contains("_describe -t tools"), "{idx}");
        // a colon in a description would break the zsh spec format
        assert!(!idx.lines().any(|l| l.matches(':').count() > 2 && l.starts_with("    '")),
                "descriptions are sanitised: {idx}");
    }

    #[test]
    fn the_survey_says_how_much_was_actually_learned() {
        let tools = vec![
            from_shell("a.sh", BUILD_SH),
            from_shell("b.sh", "#!/bin/sh\necho hi\n"),
            Tool { name: "c".into(), path: "p".into(), about: None, flags: vec![],
                   subcommands: vec![], learned: Learned::Clap, role: Role::EntryPoint },
        ];
        let s = survey(&tools);
        assert!(s.contains("3 tool(s)"), "{s}");
        assert!(s.contains("from a script header"), "{s}");
        assert!(s.contains("clap (use clap_complete)"), "{s}");
        assert!(s.contains("1  have flags worth completing"), "honest about coverage: {s}");
    }

    #[test]
    fn a_step_in_something_else_is_not_completed() {
        // a person typing the name of an internal step is usually lost,
        // and completing it suggests they were not
        assert_eq!(role_of(0, 3), Role::Step);
        assert!(!role_of(0, 3).worth_completing());
        // measured on the real repo: 25 documented-but-not-invoked
        // against 2 invoked-only, so the signal discriminates
        assert!(role_of(2, 0).worth_completing(), "documented = a human is told about it");
        assert!(role_of(2, 4).worth_completing(), "both is still an entry point");
        // and something neither documented nor called is a question,
        // not a completion
        assert_eq!(role_of(0, 0), Role::Orphan);
        assert!(!role_of(0, 0).worth_completing());
    }

    #[test]
    fn a_program_written_in_bash_is_told_to_move_before_being_blessed() {
        // completing it fixes the interface in place at exactly the
        // moment it should be changing
        let big = Tool {
            name: "deploy.sh".into(), path: "p".into(), about: None,
            flags: (0..5).map(|i| Flag { flag: format!("--f{i}"), about: None,
                                         takes_value: false }).collect(),
            subcommands: vec![], learned: Learned::Header, role: Role::EntryPoint };
        let advice = port_first(&big, 420).expect("advised");
        assert!(advice.contains("program written in bash"), "{advice}");
        assert!(advice.contains("fixes the interface in place"), "{advice}");
        // a wide interface alone is enough, whatever the length
        let wide = Tool { flags: (0..9).map(|i| Flag { flag: format!("--g{i}"), about: None,
                                            takes_value: false }).collect(), ..big.clone() };
        assert!(port_first(&wide, 40).unwrap().contains("clap_complete would then generate"));
        // and a small honest script is left alone
        let small = Tool { flags: vec![Flag { flag: "--run".into(), about: None,
                                              takes_value: false }], ..big.clone() };
        assert!(port_first(&small, 60).is_none());
    }

    #[test]
    fn the_advice_is_advice_and_not_a_refusal() {
        // the person maintaining it knows things this does not
        let big = Tool {
            name: "deploy.sh".into(), path: "p".into(), about: None,
            flags: (0..6).map(|i| Flag { flag: format!("--f{i}"), about: None,
                                         takes_value: false }).collect(),
            subcommands: vec![], learned: Learned::Header, role: Role::EntryPoint };
        assert!(port_first(&big, 400).is_some(), "it says so");
        // ...and still emits a working completion, because refusing
        // would just mean no completion until someone does the port
        let z = zsh_for(&big);
        assert!(z.contains("#compdef deploy.sh"), "{z}");
        assert!(z.contains("_arguments"), "{z}");
    }
}
