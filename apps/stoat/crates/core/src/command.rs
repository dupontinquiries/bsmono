//! One vocabulary for "do this to the session", whatever produced it: a typed
//! line, a palette row, a recognised utterance. Shells execute `Command`s; they do
//! not each grow their own little language (the CLI did, first).
//!
//! Typed and spoken input are parsed differently on purpose. Typed input is
//! terse and exact (`p n->next`, `b main.rs:12`). Speech is neither: a recogniser
//! returns "Step over." or "print my var", never punctuation you can rely on and
//! never an expression with operators. So the spoken side matches phrases, turns
//! number words into numbers, and resolves names against what is in scope.
//!
//! The rule that matters most is `resumes_target`. A debugger stop is state that
//! cannot be recovered once the program runs on, and a recogniser that hears
//! "continue" in "can't you…" destroys it. Shells must not auto-run a SPOKEN
//! command for which this returns true; they show it and wait for a confirmation.

use crate::backend::StepKind;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Continue,
    Interrupt,
    /// Start, or restart, the loaded program.
    Run,
    Step(StepKind),
    /// Toward the caller by N frames.
    Up(u32),
    Down(u32),
    Frame(u32),
    Thread(u32),
    Print(String),
    Break(String),
    /// A line for the backend's own command language.
    Raw(String),
    // Loading something to debug. A shell handles these: they replace the session.
    Build,
    /// Debug the candidate whose file name best matches this.
    Debug(String),
    Attach(u32),
    OpenCore(PathBuf),
}

impl Command {
    /// True when executing this lets the program run, which discards the
    /// current stop. See the module docs for why speech must not do that unasked.
    pub fn resumes_target(&self) -> bool {
        matches!(self, Command::Continue | Command::Run | Command::Step(_))
    }

    /// Loaders replace the session rather than act on it.
    pub fn is_loader(&self) -> bool {
        matches!(self, Command::Build | Command::Debug(_) | Command::Attach(_) | Command::OpenCore(_))
    }

    pub fn label(&self) -> String {
        match self {
            Command::Continue => "continue".into(),
            Command::Interrupt => "interrupt".into(),
            Command::Run => "run / restart".into(),
            Command::Step(StepKind::Over) => "step over".into(),
            Command::Step(StepKind::Into) => "step into".into(),
            Command::Step(StepKind::Out) => "step out".into(),
            Command::Up(1) => "up (toward the caller)".into(),
            Command::Up(n) => format!("up {n} frames"),
            Command::Down(1) => "down (toward the callee)".into(),
            Command::Down(n) => format!("down {n} frames"),
            Command::Frame(n) => format!("frame {n}"),
            Command::Thread(n) => format!("thread {n}"),
            Command::Print(e) => format!("print {e}"),
            Command::Break(l) => format!("break at {l}"),
            Command::Raw(l) => format!("gdb: {l}"),
            Command::Build => "build".into(),
            Command::Debug(n) => format!("debug {n}"),
            Command::Attach(pid) => format!("attach to {pid}"),
            Command::OpenCore(p) => format!("open core {}", p.display()),
        }
    }
}

/// Argument-free commands with the extra words a fuzzy search should hit.
pub fn catalog() -> Vec<(Command, &'static str)> {
    vec![
        (Command::Step(StepKind::Over), "next line n"),
        (Command::Continue, "go resume c"),
        (Command::Step(StepKind::Into), "s enter function"),
        (Command::Step(StepKind::Out), "finish return fin"),
        (Command::Up(1), "u caller outer"),
        (Command::Down(1), "d callee inner"),
        (Command::Interrupt, "pause stop i"),
        (Command::Run, "r start again"),
        (Command::Build, "compile make cargo"),
    ]
}

/// A typed line, gdb-flavoured. `None` means "not a command", not "error": the
/// caller usually falls back to searching with the same text.
pub fn parse(line: &str) -> Option<Command> {
    let line = line.trim();
    if let Some(raw) = line.strip_prefix('!') {
        return Some(Command::Raw(raw.trim().to_string())).filter(|_| !raw.trim().is_empty());
    }
    let (head, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(h, r)| (h, r.trim()));
    let count = || if rest.is_empty() { Some(1) } else { rest.parse::<u32>().ok() };
    let arg = || Some(rest.to_string()).filter(|r| !r.is_empty());
    Some(match head.to_ascii_lowercase().as_str() {
        "c" | "cont" | "continue" if rest.is_empty() => Command::Continue,
        "i" | "interrupt" | "pause" if rest.is_empty() => Command::Interrupt,
        "r" | "run" | "restart" if rest.is_empty() => Command::Run,
        "n" | "next" | "over" if rest.is_empty() => Command::Step(StepKind::Over),
        "s" | "step" | "into" if rest.is_empty() => Command::Step(StepKind::Into),
        "fin" | "finish" | "out" if rest.is_empty() => Command::Step(StepKind::Out),
        "u" | "up" => Command::Up(count()?),
        "d" | "down" => Command::Down(count()?),
        "f" | "frame" => Command::Frame(rest.parse().ok()?),
        "t" | "thread" => Command::Thread(rest.parse().ok()?),
        "p" | "print" => Command::Print(arg()?),
        "b" | "break" => Command::Break(arg()?),
        "gdb" => Command::Raw(arg()?),
        "build" if rest.is_empty() => Command::Build,
        "debug" => Command::Debug(arg()?),
        "attach" => Command::Attach(rest.parse().ok()?),
        "core" => Command::OpenCore(arg()?.into()),
        _ => return None,
    })
}

fn number_word(w: &str) -> Option<u32> {
    const WORDS: [&str; 13] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
    w.parse().ok().or_else(|| WORDS.iter().position(|x| *x == w).map(|i| i as u32)).or(match w {
        // What recognisers actually return for the short ones.
        "to" | "too" => Some(2),
        "for" => Some(4),
        _ => None,
    })
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A spoken name resolved against what is in scope: "my var" -> `my_var`,
/// "counter" heard as "counters" -> `counter`. Falls back to snake_case.
fn resolve_name(words: &[&str], names_in_scope: &[String]) -> String {
    let joined = words.join("_");
    let flat = words.join("");
    let norm = |n: &str| n.to_ascii_lowercase().replace('_', "");
    if let Some(exact) = names_in_scope.iter().find(|n| norm(n) == flat) {
        return exact.clone();
    }
    names_in_scope
        .iter()
        .map(|n| (edit_distance(&norm(n), &flat), n))
        .filter(|(d, n)| *d <= 1 + n.len() / 5)
        .min_by_key(|(d, _)| *d)
        .map_or(joined, |(_, n)| n.clone())
}

/// What a recogniser heard, as a command. `names_in_scope` are the selected
/// frame's locals. `None` when nothing matched; the caller should show the text
/// rather than guess.
pub fn interpret_spoken(utterance: &str, names_in_scope: &[String]) -> Option<Command> {
    let cleaned: String = utterance.to_lowercase().chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { ' ' }).collect();
    const FILLER: [&str; 12] = ["please", "uh", "um", "okay", "ok", "the", "a", "now", "just", "can", "you", "me"];
    let w: Vec<&str> = cleaned.split_whitespace().filter(|w| !FILLER.contains(w)).collect();
    let num_at = |i: usize| w.get(i).and_then(|x| number_word(x));
    let name_from = |i: usize| (w.len() > i).then(|| resolve_name(&w[i..], names_in_scope));
    Some(match w.as_slice() {
        ["continue"] | ["resume"] | ["go"] | ["keep", "going"] | ["carry", "on"] | ["go", "on"] => Command::Continue,
        ["stop"] | ["pause"] | ["interrupt"] | ["hold", "on"] | ["halt"] => Command::Interrupt,
        ["restart"] | ["run"] | ["run", "again"] | ["start", "over"] | ["start", "again"] => Command::Run,
        ["next"] | ["step"] | ["step", "over"] | ["next", "line"] | ["over"] => Command::Step(StepKind::Over),
        ["step", "in"] | ["step", "into"] | ["into"] | ["step", "inside"] => Command::Step(StepKind::Into),
        ["step", "out"] | ["finish"] | ["return"] | ["out"] => Command::Step(StepKind::Out),
        ["build"] | ["rebuild"] | ["compile"] => Command::Build,
        ["up"] | ["go", "up"] | ["caller"] | ["outer"] => Command::Up(1),
        ["down"] | ["go", "down"] | ["callee"] | ["inner"] => Command::Down(1),
        ["up", ..] | ["go", "up", ..] => Command::Up(num_at(w.len() - 1).or_else(|| num_at(w.len().saturating_sub(2)))?),
        ["down", ..] | ["go", "down", ..] => Command::Down(num_at(w.len() - 1).or_else(|| num_at(w.len().saturating_sub(2)))?),
        ["frame", "number", ..] => Command::Frame(num_at(2)?),
        ["frame", ..] => Command::Frame(num_at(1)?),
        ["thread", "number", ..] => Command::Thread(num_at(2)?),
        ["thread", ..] => Command::Thread(num_at(1)?),
        ["print", ..] | ["show", ..] | ["evaluate", ..] | ["inspect", ..] => Command::Print(name_from(1)?),
        ["what", "is", ..] | ["value", "of", ..] | ["what", "s", ..] => Command::Print(name_from(2)?),
        ["break", "at", "line", ..] | ["break", "on", "line", ..] | ["stop", "at", "line", ..] => Command::Break(num_at(3)?.to_string()),
        ["break", "at", ..] | ["break", "on", ..] | ["break", "in", ..] | ["stop", "at", ..] | ["breakpoint", "at", ..] | ["breakpoint", "on", ..] => Command::Break(w[2..].join("_")).into(),
        _ => return None,
    })
    .filter(|c| !matches!(c, Command::Break(l) if l.is_empty()))
}
