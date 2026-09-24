//! Parsing what the user typed into what dotool should do.
//!
//! Two kinds of token, told apart by shape rather than by a flag:
//!
//! - **A key**: `enter`, `C-a`, `M-x`, `ctrl+c`, `C-{x c}`.
//! - **Everything else**: text to be typed.
//!
//! ## Emacs notation, because that is what fingers already know
//!
//! | prefix | modifier | note |
//! |---|---|---|
//! | `C-` | ctrl | |
//! | `M-` | alt | "meta", which on a PC keyboard is Alt |
//! | `S-` | shift | capital S |
//! | `s-` | super | lowercase s — the case is the whole difference |
//! | `A-` | alt | Emacs' separate Alt key does not exist on a PC; folded into alt |
//!
//! `S-` and `s-` differing only in case is a genuine footgun, and it is
//! Emacs' own convention, so it is kept rather than invented around. A bare
//! `s` is never ambiguous in practice because a modifier prefix must be
//! followed by `-`.
//!
//! Modifiers stack: `C-M-a`, `C-S-tab`.
//!
//! ## Braces distribute a prefix
//!
//! `C-{x c}` is `C-x C-c` — the Emacs quit chord, written once. Anything
//! inside inherits the prefix unless it carries its own:
//!
//! ```text
//! C-{a e}       ->  ctrl+a  ctrl+e
//! C-{x M-w}     ->  ctrl+x  alt+w        (the inner prefix wins)
//! M-{1 2 3}     ->  alt+1   alt+2  alt+3
//! ```
//!
//! Nesting is not supported and is reported rather than half-handled.

use std::fmt;

/// One thing to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Type these characters.
    Text(String),
    /// Press this combination, as dotool spells it (`ctrl+a`, `return`).
    Key(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub token: String,
    pub why: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.token, self.why)
    }
}

/// Keys people name in words. Left side is what a person writes, right side
/// is what dotool calls it.
const ALIASES: &[(&str, &str)] = &[
    ("enter", "return"),
    ("ret", "return"),
    ("escape", "esc"),
    ("del", "delete"),
    ("bs", "backspace"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("ins", "insert"),
    ("spc", "space"),
];

/// Bare words that are keys rather than text.
const BARE_KEYS: &[&str] = &[
    "return", "enter", "ret", "tab", "esc", "escape", "space", "spc", "backspace", "bs", "delete", "del", "insert", "ins", "home", "end", "pageup", "pgup", "pagedown", "pgdn", "up", "down", "left", "right", "capslock", "menu", "print", "printscreen", "pause",
];

fn is_function_key(s: &str) -> bool {
    s.len() >= 2 && s.starts_with('f') && s[1..].parse::<u8>().map(|n| (1..=24).contains(&n)).unwrap_or(false)
}

/// Is this token a key rather than text? Used to split a command line.
pub fn looks_like_key(tok: &str) -> bool {
    let t = tok.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains('+') || t.contains('{') {
        return true;
    }
    if modifier_prefix(t).is_some() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    BARE_KEYS.contains(&lower.as_str()) || is_function_key(&lower)
}

/// `C-` at the head of `s` yields ("ctrl", rest). Case matters: `S-` is
/// shift, `s-` is super.
fn modifier_prefix(s: &str) -> Option<(&'static str, &str)> {
    let rest = s.get(2..)?;
    if !s.is_char_boundary(2) || !s[1..2].starts_with('-') {
        return None;
    }
    let m = match &s[0..1] {
        "C" => "ctrl",
        "M" => "alt",
        "A" => "alt",
        "S" => "shift",
        "s" => "super",
        _ => return None,
    };
    Some((m, rest))
}

fn canon_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    ALIASES.iter().find(|(from, _)| *from == lower).map(|(_, to)| (*to).to_string()).unwrap_or(lower)
}

/// Parse one key token into dotool combinations. Usually one; a brace group
/// yields several.
pub fn parse_key(tok: &str) -> Result<Vec<String>, ParseError> {
    let err = |why: &str| ParseError { token: tok.to_string(), why: why.to_string() };
    let mut mods: Vec<&'static str> = Vec::new();
    let mut rest = tok.trim();

    // Leading Emacs modifiers.
    while let Some((m, r)) = modifier_prefix(rest) {
        if !mods.contains(&m) {
            mods.push(m);
        }
        rest = r;
    }

    // A brace group distributes whatever modifiers we collected.
    if let Some(open) = rest.find('{') {
        if open != 0 {
            return Err(err("a brace group must follow the modifiers directly, as in C-{x c}"));
        }
        let close = rest.rfind('}').ok_or_else(|| err("unclosed {"))?;
        let inner = &rest[1..close];
        if !rest[close + 1..].trim().is_empty() {
            return Err(err("trailing text after }"));
        }
        if inner.contains('{') {
            return Err(err("nested braces are not supported"));
        }
        if inner.trim().is_empty() {
            return Err(err("empty brace group"));
        }
        let mut out = Vec::new();
        for part in inner.split_whitespace() {
            // An inner token with its own modifier overrides the outer one:
            // C-{x M-w} is ctrl+x then alt+w.
            let combos = if modifier_prefix(part).is_some() {
                parse_key(part)?
            } else {
                vec![combo(&mods, &canon_key(part))]
            };
            out.extend(combos);
        }
        return Ok(out);
    }

    // `ctrl+c` style, which dotool already understands; normalise the key
    // half so `ctrl+enter` becomes `ctrl+return`.
    if rest.contains('+') {
        let mut parts: Vec<String> = rest.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
        let last = parts.pop().ok_or_else(|| err("empty combination"))?;
        if last.is_empty() {
            return Err(err("a combination must end in a key, as in ctrl+c"));
        }
        for m in &mods {
            if !parts.iter().any(|p| p == m) {
                parts.insert(0, (*m).to_string());
            }
        }
        parts.push(canon_key(&last));
        return Ok(vec![parts.join("+")]);
    }

    if rest.is_empty() {
        return Err(err("a modifier needs a key after it, as in C-a"));
    }
    let k = canon_key(rest);
    if k.chars().count() != 1 && !BARE_KEYS.contains(&k.as_str()) && !is_function_key(&k) {
        return Err(err("not a known key name"));
    }
    Ok(vec![combo(&mods, &k)])
}

/// Modifiers and a key, as dotool spells a combination.
fn combo(mods: &[&'static str], key: &str) -> String {
    if mods.is_empty() {
        key.to_string()
    } else {
        format!("{}+{}", mods.join("+"), key)
    }
}

/// Parse a whole command line into steps.
///
/// `literal` skips key detection entirely, for when the words themselves are
/// the text you want typed.
pub fn parse(args: &[String], literal: bool) -> Result<Vec<Step>, ParseError> {
    if literal {
        return Ok(vec![Step::Text(args.join(" "))]);
    }
    let mut steps: Vec<Step> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    // A brace group can be written with spaces, so rejoin the pieces first.
    for tok in regroup(args) {
        if looks_like_key(&tok) {
            if !text.is_empty() {
                steps.push(Step::Text(text.join(" ")));
                text.clear();
            }
            for combo in parse_key(&tok)? {
                steps.push(Step::Key(combo));
            }
        } else {
            text.push(tok);
        }
    }
    if !text.is_empty() {
        steps.push(Step::Text(text.join(" ")));
    }
    Ok(steps)
}

/// The shell splits `C-{x c}` into `C-{x` and `c}`. Put it back together.
fn regroup(args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    for a in args {
        match &mut open {
            Some(buf) => {
                buf.push(' ');
                buf.push_str(a);
                if a.contains('}') {
                    out.push(open.take().unwrap());
                }
            }
            None => {
                if a.contains('{') && !a.contains('}') {
                    open = Some(a.clone());
                } else {
                    out.push(a.clone());
                }
            }
        }
    }
    // An unclosed group is passed through so parse_key reports it properly.
    if let Some(buf) = open {
        out.push(buf);
    }
    out
}

/// The dotool script for a plan.
pub fn to_dotool(steps: &[Step]) -> String {
    let mut s = String::new();
    for st in steps {
        match st {
            Step::Text(t) => {
                s.push_str("type ");
                s.push_str(t);
                s.push('\n');
            }
            Step::Key(k) => {
                s.push_str("key ");
                s.push_str(k);
                s.push('\n');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(t: &str) -> Vec<String> {
        parse_key(t).unwrap()
    }
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn emacs_modifiers_map_to_a_pc_keyboard() {
        assert_eq!(keys("C-a"), ["ctrl+a"]);
        assert_eq!(keys("M-x"), ["alt+x"], "meta is Alt here");
        assert_eq!(keys("A-x"), ["alt+x"], "and so is Emacs' Alt, which PCs lack");
        assert_eq!(keys("C-M-a"), ["ctrl+alt+a"]);
        assert_eq!(keys("C-v"), ["ctrl+v"]);
        // The case of S is the whole difference, as in Emacs.
        assert_eq!(keys("S-tab"), ["shift+tab"]);
        assert_eq!(keys("s-l"), ["super+l"]);
        assert_ne!(keys("S-l"), keys("s-l"));
        // Repeating a modifier is not an error, just redundant.
        assert_eq!(keys("C-C-a"), ["ctrl+a"]);
    }

    #[test]
    fn braces_distribute_the_prefix() {
        assert_eq!(keys("C-{x c}"), ["ctrl+x", "ctrl+c"], "the quit chord, written once");
        assert_eq!(keys("C-{a e}"), ["ctrl+a", "ctrl+e"]);
        assert_eq!(keys("M-{1 2 3}"), ["alt+1", "alt+2", "alt+3"]);
        // An inner modifier overrides rather than stacking: C-x then M-w.
        assert_eq!(keys("C-{x M-w}"), ["ctrl+x", "alt+w"]);
        // A group with no prefix is just a sequence.
        assert_eq!(keys("{a b}"), ["a", "b"]);
        // Word keys inside a group still get their aliases.
        assert_eq!(keys("C-{x enter}"), ["ctrl+x", "ctrl+return"]);
    }

    #[test]
    fn word_keys_become_what_dotool_calls_them() {
        assert_eq!(keys("enter"), ["return"]);
        assert_eq!(keys("escape"), ["esc"]);
        assert_eq!(keys("pgdn"), ["pagedown"]);
        assert_eq!(keys("ctrl+enter"), ["ctrl+return"], "normalised inside a + combination too");
        assert_eq!(keys("f12"), ["f12"]);
        assert_eq!(keys("C-f5"), ["ctrl+f5"]);
    }

    #[test]
    fn text_and_keys_are_told_apart_by_shape() {
        assert_eq!(parse(&args("warp enter"), false).unwrap(), vec![Step::Text("warp".into()), Step::Key("return".into())]);
        assert_eq!(parse(&args("hello world"), false).unwrap(), vec![Step::Text("hello world".into())]);
        // Text resumes after a key, rather than everything after the first
        // key becoming keys.
        assert_eq!(
            parse(&args("git status enter make C-c"), false).unwrap(),
            vec![Step::Text("git status".into()), Step::Key("return".into()), Step::Text("make".into()), Step::Key("ctrl+c".into())]
        );
        // Literal mode types the words themselves.
        assert_eq!(parse(&args("enter tab"), true).unwrap(), vec![Step::Text("enter tab".into())]);
    }

    #[test]
    fn a_brace_group_survives_the_shell_splitting_it() {
        // The shell hands us ["C-{x", "c}"], not ["C-{x c}"].
        assert_eq!(parse(&args("C-{x c}"), false).unwrap(), vec![Step::Key("ctrl+x".into()), Step::Key("ctrl+c".into())]);
        assert_eq!(
            parse(&args("emacs enter C-{x c}"), false).unwrap(),
            vec![Step::Text("emacs".into()), Step::Key("return".into()), Step::Key("ctrl+x".into()), Step::Key("ctrl+c".into())]
        );
    }

    #[test]
    fn broken_specs_say_what_is_wrong() {
        for (spec, want) in [
            ("C-{x c", "unclosed"),
            ("C-{}", "empty"),
            ("C-{a {b}}", "nested"),
            ("C-", "needs a key"),
            ("ctrl+", "must end in a key"),
            ("C-{x c}trailing", "trailing"),
            ("C-notakey", "not a known key"),
        ] {
            let e = parse_key(spec).unwrap_err();
            assert!(e.why.contains(want), "{spec}: expected {want:?}, got {:?}", e.why);
            assert_eq!(e.token, spec, "the message names the token");
        }
    }

    #[test]
    fn the_emitted_script_is_what_dotool_reads() {
        let steps = parse(&args("warp enter C-{x c}"), false).unwrap();
        assert_eq!(to_dotool(&steps), "type warp\nkey return\nkey ctrl+x\nkey ctrl+c\n");
    }
}
