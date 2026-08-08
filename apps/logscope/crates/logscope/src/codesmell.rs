//! codesmell.rs — the specific ways small-model Rust is broken, detected without compiling.
//!
//! "Write cleaner code" is advice, and a model that could act on it would not have needed it. What
//! works is naming the fault: **a follow-up that says "line 57 constructs a Game and drops it" gets
//! fixed, and one that says "improve the structure" gets a rewrite of something else.**
//!
//! Every pattern here is from real output. From one Llama-4 transcript:
//!
//! ```text
//! let player = Player::new();
//! Game { width: 30, height: 40 };          // constructed and dropped
//! update_player_position(&mut player);     // player is not mut
//! fn get_usr_inp() { }                     // empty stub, never called
//! ```
//!
//! ## Why not just compile it
//!
//! Because the compiler stops at the first error and these arrive in clusters, because a small model
//! often emits code that will not parse at all, and because the message matters: rustc says
//! "cannot borrow as mutable", and what a follow-up turn needs is "you declared it without mut three
//! lines up".

/// A fault detectable in the source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Smell {
    pub line: usize,
    pub kind: Kind,
    /// What to say in the follow-up. **Specific enough to act on** — the whole point.
    pub fix: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A struct literal as a statement: built and immediately dropped.
    UnboundConstruction,
    /// Borrowed mutably but declared without `mut`.
    MissingMut,
    /// A function with an empty body that nothing fills in.
    EmptyStub,
    /// A function taking no arguments that must act on state.
    StatelessMutator,
    /// Defined and never called, or called and never defined. **The fault my own test found**: the
    /// transcript calls `handle_input()` and defines `get_usr_inp()`, which are the same intent
    /// under two names — and neither half is wrong on its own line.
    OrphanedName,
}

/// Scan source for faults a follow-up can name.
pub fn scan(src: &str) -> Vec<Smell> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();

    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        let n = i + 1;

        // `Game { ... };` as a statement. The trailing semicolon after a closing brace with no
        // binding is the tell, and it is the fault most likely to be read as working code because it
        // LOOKS like initialisation.
        if t.ends_with("};") && t.contains('{') && !t.contains('=')
            && !t.starts_with("let") && !t.starts_with('}')
            && t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            out.push(Smell { line: n, kind: Kind::UnboundConstruction, fix: format!(
                "line {n} builds a value and drops it immediately — bind it with `let` or the \
initialisation does nothing") });
        }

        // An empty body. Two braces with nothing between them, on a fn.
        if t.starts_with("fn ") && (t.ends_with("{}") || t.ends_with("{ }")) {
            out.push(Smell { line: n, kind: Kind::EmptyStub, fix: format!(
                "line {n} is an empty function. Either implement it now or delete it — a stub that \
is never filled reads as finished work") });
        }

        // A mutator taking nothing. `fn handle_input()` cannot change anything it does not receive.
        if t.starts_with("fn ") && t.contains("()") {
            let name = t.trim_start_matches("fn ").split('(').next().unwrap_or("");
            for verb in ["update", "handle", "move", "set", "apply", "advance"] {
                if name.starts_with(verb) {
                    out.push(Smell { line: n, kind: Kind::StatelessMutator, fix: format!(
                        "line {n}: `{name}` takes no arguments but its name says it changes \
something. It can only act on globals — pass the state it should modify") });
                    break;
                }
            }
        }
    }

    // `let x = ...` with no `mut`, later borrowed as `&mut x`. Scanned as a pair rather than per
    // line, because neither half is wrong alone.
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        let Some(rest) = t.strip_prefix("let ") else { continue };
        if rest.starts_with("mut ") { continue }
        let Some(name) = rest.split([' ', '=', ':']).next().filter(|s| !s.is_empty()) else { continue };
        let borrow = format!("&mut {name}");
        if lines[i + 1..].iter().any(|l| l.contains(&borrow)) {
            out.push(Smell { line: i + 1, kind: Kind::MissingMut, fix: format!(
                "line {}: `{name}` is declared without `mut` and borrowed as `&mut {name}` below — \
add `mut` at the declaration rather than at the use", i + 1) });
        }
    }
    // Defined-but-never-called and called-but-never-defined. Found while testing the checker: the
    // transcript calls `handle_input()` and defines `get_usr_inp()`, one intent under two names. A
    // compiler catches the missing definition and says nothing about the orphan beside it.
    let defined: Vec<(usize, String)> = lines.iter().enumerate()
        .filter_map(|(i, l)| l.trim().strip_prefix("fn ")
            .and_then(|r| r.split('(').next())
            .filter(|n| !n.is_empty() && *n != "main")
            .map(|n| (i + 1, n.to_string())))
        .collect();
    for (line, name) in &defined {
        let called = lines.iter().enumerate()
            .any(|(i, l)| i + 1 != *line && l.contains(&format!("{name}(")));
        if !called {
            out.push(Smell { line: *line, kind: Kind::OrphanedName, fix: format!(
                "line {line}: `{name}` is defined and never called. Either wire it up or delete it — and check whether something else calls a DIFFERENT name for the same thing") });
        }
    }
    out.sort_by_key(|s| s.line);
    out
}

/// The follow-up message, from the faults found.
///
/// **Names lines and fixes rather than asking for quality.** "Write cleaner code" is advice a model
/// that could act on it would not have needed; "line 57 builds a value and drops it" is a task.
pub fn followup(smells: &[Smell]) -> String {
    if smells.is_empty() {
        return "no structural faults found — anything further is a judgement about design rather \
than a defect, and needs a person".into();
    }
    let mut s = String::from("Fix these, in place, without rewriting the rest:\n");
    for sm in smells { s.push_str(&format!("  - {}\n", sm.fix)); }
    s.push_str("Do not restate the file. Change only these lines.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "\
fn main(){
   let player = Player::new();
   Game {width :30 , height :40};
     loop {
        handle_input();
       update_player_position(&mut player);
    }
}
fn get_usr_inp(){}
";

    #[test]
    fn the_UNBOUND_construction_is_found() {
        // The fault most likely to be read as working code, because it looks like initialisation.
        let s = scan(REAL);
        let u = s.iter().find(|x| x.kind == Kind::UnboundConstruction).expect("missed it");
        assert_eq!(u.line, 3);
        assert!(u.fix.contains("drops it immediately"));
    }

    #[test]
    fn MISSING_MUT_is_found_as_a_PAIR_not_a_line() {
        // Neither half is wrong alone: `let player = ...` is fine, and `&mut player` is fine. Only
        // together are they a fault, which is why a per-line scan misses it.
        let m = scan(REAL).into_iter().find(|x| x.kind == Kind::MissingMut).expect("missed it");
        assert_eq!(m.line, 2);
        assert!(m.fix.contains("add `mut` at the declaration"));
    }

    #[test]
    fn an_EMPTY_STUB_is_named() {
        let e = scan(REAL).into_iter().find(|x| x.kind == Kind::EmptyStub).expect("missed it");
        assert!(e.fix.contains("reads as finished work"));
    }

    #[test]
    fn an_ORPHANED_NAME_is_flagged_and_my_own_test_found_it() {
        // I wrote a test expecting `fn handle_input()` to be flagged as a stateless mutator, and the
        // fixture has no such DEFINITION — it CALLS handle_input() and DEFINES get_usr_inp(). Two
        // names for one intent, which no per-line check sees and which a compiler reports only as a
        // missing function, saying nothing about the orphan beside it.
        let s = scan(REAL);
        let o = s.iter().find(|x| x.kind == Kind::OrphanedName).expect("missed the orphan");
        assert!(o.fix.contains("get_usr_inp"));
        assert!(o.fix.contains("a DIFFERENT name for the same thing"));
    }

    #[test]
    fn CLEAN_code_produces_nothing() {
        // A checker that fires on good code gets ignored, and then misses the bad.
        let clean = "fn main() {\n    let mut p = Player::new();\n    advance(&mut p);\n}\n\
fn advance(p: &mut Player) { p.x += 1; }\n";
        assert!(scan(clean).is_empty(), "{:?}", scan(clean));
    }

    #[test]
    fn the_followup_names_LINES_rather_than_asking_for_quality() {
        // "Write cleaner code" is advice a model that could act on it would not have needed.
        let f = followup(&scan(REAL));
        assert!(f.contains("line 3"));
        assert!(f.contains("Change only these lines"));
        assert!(!f.to_lowercase().contains("cleaner"));
    }

    #[test]
    fn NO_FAULTS_says_the_rest_needs_a_person() {
        // The honest boundary: this finds defects, not bad design, and pretending otherwise would
        // make it a style bot.
        assert!(followup(&[]).contains("needs a person"));
    }
}
