//! tripwire.rs — cheap, deterministic scanners over a model's *in-progress* code output that catch the
//! mechanical failures a small model makes mid-generation, so the runtime can offer a review point instead
//! of letting the mistake ride to the end of a long generation.
//!
//! Motivation (from real 4B transcripts): a small model chooses good architecture but loses its own
//! invariants after a few dozen lines — a struct field terminated with `;` instead of `,`, a block left
//! open by truncation, a `use`-before-def, a malformed dependency version. These are NOT semantic type
//! errors (those need a compiler — that's what the sandbox `cargo check` at a subtask boundary is for).
//! They are *string-level* defects with high precision: when a scanner here fires, the code is almost
//! certainly broken, so it's safe to nudge a review.
//!
//! Design contract:
//!   * PURE + deterministic + unit-tested — no model, no GPU, no I/O. The trigger, not the fix.
//!   * HIGH PRECISION over recall — a false interject costs a branch/pause and risks derailing a fine
//!     generation, so each scanner only fires when it's nearly certain. Missing a real bug is acceptable
//!     (the sandbox `cargo check` is the ground-truth backstop); crying wolf is not.
//!   * The output is a *suggestion* (`Signal::Interject(reason)`), never an override. Per the design
//!     discussion: the main model is ENCOURAGED to pause and self-review; the tripwire just earns it a
//!     natural review point. It does not splice a correction.
//!
//! Cadence: the caller runs `scan` on the growing generated text at natural beats (end of a fenced code
//! block, end of a function) — not every token — mirroring how a coder looks back after finishing a unit.

/// What the scan concluded about the code produced so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// Nothing mechanical looks wrong — keep generating.
    WaitAndSee,
    /// A mechanical defect is nearly certain; offer the model a review point. Carries a specific,
    /// actionable reason so the review (or a small friend model) has something concrete to check.
    Interject(Reason),
}

/// Why the scanner wants a review — specific enough to hand to the model or a friend model verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Delimiters don't balance across the whole output (more openers than closers or vice-versa).
    UnbalancedDelimiters { kind: char, open: usize, close: usize },
    /// A fenced code block was opened (```lang) and never closed — the generation likely truncated.
    StaleOpenFence,
    /// An identifier is USED but was never DEFINED earlier in the generated code (the classic small-model
    /// invariant break: `With<Player>` where `Player` is never declared). Rust-oriented, conservative.
    UndefinedSymbolUsed { symbol: String },
    /// The output ends mid-construct (trailing `{`, `(`, `,`, `=`, or an unterminated line) — truncation.
    TruncationSuspected { tail: String },
    /// A `name = "..."` dependency version that isn't a valid semver-ish string (`"v0.8"`, empty, `=`).
    MalformedDepVersion { name: String, value: String },
}

impl Reason {
    /// One-line human/friend-model-facing description, e.g. to seed a review prompt.
    pub fn describe(&self) -> String {
        match self {
            Reason::UnbalancedDelimiters { kind, open, close } =>
                format!("delimiter '{kind}' is unbalanced: {open} opened, {close} closed"),
            Reason::StaleOpenFence =>
                "a ``` code fence was opened and never closed (output likely truncated)".into(),
            Reason::UndefinedSymbolUsed { symbol } =>
                format!("`{symbol}` is used but never defined earlier in the code"),
            Reason::TruncationSuspected { tail } =>
                format!("output ends mid-construct (…{tail}) — looks truncated"),
            Reason::MalformedDepVersion { name, value } =>
                format!("dependency `{name}` has a malformed version {value:?}"),
        }
    }
}

/// Run every scanner over the code generated so far, returning the FIRST (most certain) signal. Order is
/// by precision: truncation/unbalanced/fence are near-certain; undefined-symbol is conservative and last.
pub fn scan(code: &str) -> Signal {
    // A tail scan is the cheapest + highest-precision truncation check.
    if let Some(r) = truncation(code) { return Signal::Interject(r); }
    if let Some(r) = open_fence(code) { return Signal::Interject(r); }
    if let Some(r) = unbalanced(code) { return Signal::Interject(r); }
    if let Some(r) = malformed_dep_version(code) { return Signal::Interject(r); }
    if let Some(r) = undefined_symbol(code) { return Signal::Interject(r); }
    Signal::WaitAndSee
}

/// Strip fenced code-block bodies' string/char literals and line comments so delimiter counting doesn't
/// trip on `"}"` or `// (`. Deliberately simple: good enough for high-precision balance checks on Rust.
fn strip_noise(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_str = false;
    let mut in_char = false;
    while let Some(c) = chars.next() {
        if in_str {
            if c == '\\' { chars.next(); continue; }
            if c == '"' { in_str = false; }
            continue;
        }
        if in_char {
            if c == '\\' { chars.next(); continue; }
            if c == '\'' { in_char = false; }
            continue;
        }
        match c {
            '"' => { in_str = true; }
            // a lone ' starts a char literal only if it looks like one (not a lifetime); keep it simple:
            // treat '\'' pairs as literals, lifetimes (&'a) rarely contain delimiters so counting is safe.
            '/' if chars.peek() == Some(&'/') => { for n in chars.by_ref() { if n == '\n' { out.push('\n'); break; } } }
            _ => out.push(c),
        }
    }
    out
}

/// Delimiter balance across the whole output. Fires only when a delimiter is *over-closed* (more closers
/// than openers — unambiguously wrong) or, for a COMPLETE-looking output, left open. To stay
/// high-precision we only report over-close here; unclosed-at-end is handled as truncation/fence.
fn unbalanced(code: &str) -> Option<Reason> {
    let s = strip_noise(code);
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        let o = s.matches(open).count();
        let c = s.matches(close).count();
        if c > o { return Some(Reason::UnbalancedDelimiters { kind: close, open: o, close: c }); }
    }
    None
}

/// A ``` fence opened and never closed → the code block (and probably the whole message) was truncated.
fn open_fence(code: &str) -> Option<Reason> {
    if code.matches("```").count() % 2 == 1 { Some(Reason::StaleOpenFence) } else { None }
}

/// The output ends mid-construct. Looks at the last non-empty, non-comment line's final significant char.
fn truncation(code: &str) -> Option<Reason> {
    let trimmed = code.trim_end();
    if trimmed.is_empty() { return None; }
    // last meaningful line
    let last = trimmed.lines().rev().find(|l| { let t = l.trim(); !t.is_empty() && !t.starts_with("//") })?;
    let t = last.trim_end();
    let end = t.chars().last()?;
    // A line ending in an opener or a binder is mid-construct. `{` alone is normal (block open), so we only
    // flag `{`/`(` when the WHOLE output doesn't balance (checked by pairing with a tail-only heuristic):
    // trailing `,` `=` `+` `-` `:` `.` `<` or an opener with an unbalanced overall body = truncated.
    let mid_binder = matches!(end, ',' | '=' | '+' | '<' | '.' | ':' | '&' | '|');
    let opener = matches!(end, '(' | '{' | '[');
    if mid_binder {
        return Some(Reason::TruncationSuspected { tail: tail_snip(t) });
    }
    if opener {
        // an opener at the very end is only suspicious if the whole body is unbalanced open
        let s = strip_noise(code);
        let unbal = ['(', '{', '['].iter().zip([')', '}', ']']).any(|(&o, c)| s.matches(o).count() > s.matches(c).count());
        if unbal { return Some(Reason::TruncationSuspected { tail: tail_snip(t) }); }
    }
    // Blocks left open: if the body has more openers than closers AND the last line doesn't itself close a
    // block (doesn't end in a closer), the generation stopped inside an unclosed block → truncated. This
    // catches `…{ TileType::Ground` (ends on an ident, but 3 unclosed `{`).
    let closer = matches!(end, ')' | '}' | ']' | ';');
    if !closer {
        let s = strip_noise(code);
        let unbal = ['(', '{', '['].iter().zip([')', '}', ']']).any(|(&o, c)| s.matches(o).count() > s.matches(c).count());
        if unbal { return Some(Reason::TruncationSuspected { tail: tail_snip(t) }); }
    }
    None
}

fn tail_snip(s: &str) -> String {
    let n = s.chars().count();
    if n <= 24 { s.to_string() } else { s.chars().skip(n - 24).collect() }
}

/// Cargo `name = "version"` where the version isn't semver-ish. Catches the transcript's `bevy =
/// { version = "v0.8" }` (leading v) and empty/`=`-only values. Only scans lines that look like deps.
fn malformed_dep_version(code: &str) -> Option<Reason> {
    for line in code.lines() {
        let t = line.trim();
        // inline table form: name = { version = "..." }
        if let Some(vpos) = t.find("version") {
            if let Some(q1) = t[vpos..].find('"') {
                let rest = &t[vpos + q1 + 1..];
                if let Some(q2) = rest.find('"') {
                    let val = &rest[..q2];
                    let name = t.split(|c: char| c == '=' || c == ' ').next().unwrap_or("").trim();
                    if !name.is_empty() && !valid_semverish(val) {
                        return Some(Reason::MalformedDepVersion { name: name.to_string(), value: val.to_string() });
                    }
                }
            }
        }
    }
    None
}

/// A permissive semver-ish check: starts with a digit, only digits/dots/pre-release chars. Rejects `v0.8`,
/// empty, and junk — but accepts `0.8`, `1.2.3`, `0.14.0-rc.1`, `*`.
fn valid_semverish(v: &str) -> bool {
    if v == "*" { return true; }
    let mut chars = v.chars();
    match chars.next() { Some(c) if c.is_ascii_digit() => {} _ => return false }
    v.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | '_' | 'a'..='z' | 'A'..='Z'))
}

/// CONSERVATIVE undefined-symbol check for Rust: a `With<Ident>` / `Query<&Ident>` / `Res<Ident>` generic
/// argument (Bevy's system params — the exact shape the transcript broke) whose type isn't defined by a
/// `struct`/`enum`/`type`/`use ... Ident` anywhere in the code. High precision: only these known wrapper
/// forms, only PascalCase idents, and only when there's no plausible definition or import.
fn undefined_symbol(code: &str) -> Option<Reason> {
    let defined = defined_types(code);
    // find generic args inside the known ECS wrappers
    for wrapper in ["With<", "Without<", "Res<", "ResMut<"] {
        let mut from = 0;
        while let Some(p) = code[from..].find(wrapper) {
            let start = from + p + wrapper.len();
            let arg: String = code[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            from = start;
            if arg.is_empty() { continue; }
            // only PascalCase user types (skip std/bevy built-ins heuristically: require it NOT be a common one)
            if !arg.chars().next().unwrap().is_ascii_uppercase() { continue; }
            if BUILTINS.contains(&arg.as_str()) { continue; }
            if !defined.contains(&arg) {
                return Some(Reason::UndefinedSymbolUsed { symbol: arg });
            }
        }
    }
    None
}

/// Types defined or imported in the code: `struct X`, `enum X`, `type X`, `use ...::X`, `X { ... }` decls.
fn defined_types(code: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for line in code.lines() {
        let t = line.trim();
        for kw in ["struct ", "enum ", "type ", "trait ", "union "] {
            if let Some(rest) = t.strip_prefix(kw).or_else(|| t.strip_prefix(&format!("pub {kw}"))) {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() { set.insert(name); }
            }
        }
        // imports: use a::b::Name; or use a::{X, Y};
        if t.starts_with("use ") {
            for tok in t.trim_start_matches("use ").split(|c: char| matches!(c, ':' | '{' | '}' | ',' | ';' | ' ')) {
                if tok.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) { set.insert(tok.to_string()); }
            }
        }
    }
    set
}

/// Built-in/known wrapper-arg types that a `With<T>` might legitimately reference without a local def.
const BUILTINS: &[&str] = &[
    "Transform", "Sprite", "Camera", "Name", "Entity", "Time", "Handle", "Mesh", "Image", "Window",
    "Camera2d", "Camera3d", "GlobalTransform", "Visibility", "Text", "Node", "Style", "Color",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_code_waits() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn struct_field_semicolon_is_not_flagged_as_delim_but_truncation_catches_incomplete() {
        // The transcript's `pub struct Tile { pub type_: TileType; }` — the `;` is a syntax error but
        // delimiters DO balance, so this scanner won't cry wolf on the brace count (correct: high
        // precision, we don't guess semantics). It's the truncation/fence checks that catch the real
        // breakage in that transcript (the block was cut off mid-file).
        let code = "#[derive(Component)]\npub struct Tile { pub type_: TileType; }\n";
        // balanced + complete-looking → WaitAndSee (we do NOT false-positive on the semicolon)
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn truncated_mid_function_is_caught() {
        // straight from the feedback: ends on `let tile_type = if x == 5 && y < 3 { TileType::Ground`
        let code = "fn setup_terrain(mut commands: Commands) {\n    for y in 0..10 {\n        for x in 0..12 {\n            let tile_type = if x == 5 && y < 3 { TileType::Ground\n";
        match scan(code) { Signal::Interject(Reason::TruncationSuspected { .. }) | Signal::Interject(Reason::UnbalancedDelimiters { .. }) => {} other => panic!("expected truncation/unbalanced, got {other:?}") }
    }

    #[test]
    fn open_fence_flagged() {
        let code = "Here is the code:\n```rust\nfn main() {}\n"; // fence opened, never closed
        assert_eq!(scan(code), Signal::Interject(Reason::StaleOpenFence));
    }

    #[test]
    fn balanced_fences_ok() {
        let code = "```rust\nfn main() {}\n```\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn overclosed_brace_flagged() {
        let code = "fn main() {\n    foo();\n}}\n"; // one extra }
        match scan(code) { Signal::Interject(Reason::UnbalancedDelimiters { kind: '}', .. }) => {} other => panic!("got {other:?}") }
    }

    #[test]
    fn malformed_version_v_prefix_flagged() {
        // the transcript's `bevy = { version = "v0.8", ... }`
        let code = "[dependencies]\nbevy = { version = \"v0.8\", features = [\"wgpu\"] }\n";
        match scan(code) { Signal::Interject(Reason::MalformedDepVersion { name, value }) => { assert_eq!(name, "bevy"); assert_eq!(value, "v0.8"); } other => panic!("got {other:?}") }
    }

    #[test]
    fn valid_version_ok() {
        let code = "bevy = { version = \"0.14.0\" }\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn undefined_player_flagged() {
        // `With<Player>` but Player never defined (the earlier transcript's exact break)
        let code = "fn player_movement(q: Query<&mut Transform, With<Player>>) {}\n";
        match scan(code) { Signal::Interject(Reason::UndefinedSymbolUsed { symbol }) => assert_eq!(symbol, "Player"), other => panic!("got {other:?}") }
    }

    #[test]
    fn defined_player_ok() {
        let code = "#[derive(Component)]\nstruct Player;\nfn f(q: Query<&mut Transform, With<Player>>) {}\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn builtin_with_arg_not_flagged() {
        // With<Transform> is a bevy builtin — must not false-positive
        let code = "fn f(q: Query<Entity, With<Transform>>) {}\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn imported_type_ok() {
        let code = "use crate::enemy::Enemy;\nfn f(q: Query<&mut Transform, With<Enemy>>) {}\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn string_with_braces_does_not_trip_balance() {
        let code = "fn main() {\n    let s = \"}}}}\";\n    println!(\"{}\", s);\n}\n";
        assert_eq!(scan(code), Signal::WaitAndSee);
    }

    #[test]
    fn reason_descriptions_are_actionable() {
        let r = Reason::UndefinedSymbolUsed { symbol: "Player".into() };
        assert!(r.describe().contains("Player"));
    }
}
