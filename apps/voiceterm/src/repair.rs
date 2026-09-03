//! repair: fixing what was just dictated, without throwing it away.
//!
//! `Ctl::ScratchThat` deletes the whole last unit. That is the right
//! tool when the recognizer produced nonsense, and the wrong tool for
//! the common case — whisper heard the words but got the SHAPE wrong:
//! `read to string` instead of `read_to_string`, `path` instead of
//! `Path`, a homophone in the middle of an otherwise fine line. Saying
//! the whole line again to fix one token is how a voice interface loses
//! to a keyboard.
//!
//! So: a small set of verbs that operate on the last inserted unit.
//!
//! ```text
//!   "make that snake case"    recase the whole unit
//!   "capitalize that"         first letter
//!   "uppercase that"          all of it
//!   "change foo to bar"       one word, in place
//!   "spell it r e a d"        replace with literal letters
//!   "spell it romeo echo alpha delta"   NATO, for when letters mishear
//!   "I said read to string"   replace the unit outright
//!   "join that"               close the space before the unit
//! ```
//!
//! ## Why these are whisper-only
//!
//! Most carry an ARGUMENT, and the acoustic fast path is a
//! fixed-phrase recognizer — it can match "capitalize that" but has
//! nothing to match "change foo to bar" against. Rather than split the
//! set across two tiers and have the fast path silently handle half of
//! it, the whole family goes through whisper. The latency is
//! acceptable here for a reason the control verbs do not share: a
//! repair happens after you have already noticed a mistake, so you are
//! reading, not mid-flow.
//!
//! ## Scope
//!
//! ## Scope, and its sharp edge
//!
//! A repair applies to the LAST DICTATED UNIT — one utterance's worth of
//! text — not to the last word. That is right for `Change` (which names
//! its target) and for `Spell`/`Literal` (which replace outright), but
//! it makes `Recase` blunt: after dictating "let raw equals read to
//! string" as one utterance, "make that snake case" yields
//! `let_raw_equals_read_to_string`, not `let raw equals read_to_string`.
//!
//! The practical workaround is to dictate an identifier as its own
//! utterance, which is how it reads aloud anyway. The real fix is a word
//! count on the verb ("snake case the last three words"), deliberately
//! not built here: the parse is easy and the DEFAULT is not, and getting
//! the default wrong is worse than a verb that is honestly blunt.
//! `Change` covers the precise case in the meantime.
//!
//! Every repair is a pure `&str -> String` over the last unit. It never
//! sees the document. That keeps it testable and, more importantly,
//! keeps its blast radius to text the user just produced and is
//! currently looking at.
//!
//! PURE: std only.

use crate::dictate::{apply_casing, casing, Casing};

/// A repair to apply to the last dictated unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// Re-case the whole unit as an identifier.
    Recase(Casing),
    /// Capitalize the first letter, leave the rest.
    Capitalize,
    Uppercase,
    Lowercase,
    /// Replace one word within the unit.
    Change { from: String, to: String },
    /// Replace the unit with these literal characters.
    Spell(String),
    /// Replace the unit outright with what the user just said.
    Literal(String),
    /// Close the whitespace between this unit and what precedes it.
    Join,
}

/// Parse a repair verb. Returns None for anything that is not one —
/// callers treat that as ordinary content.
pub fn parse(phrase: &str) -> Option<Repair> {
    let p = normalize(phrase);
    let p = p.as_str();

    match p {
        "capitalize that" | "capitalise that" | "capitalize" | "cap that" => {
            return Some(Repair::Capitalize)
        }
        "uppercase that" | "upper case that" | "all caps" | "shout that" => {
            return Some(Repair::Uppercase)
        }
        "lowercase that" | "lower case that" => return Some(Repair::Lowercase),
        "join that" | "join" | "no space" | "close that up" => return Some(Repair::Join),
        _ => {}
    }

    // "make that snake case" / "make it camel case" / "snake case that"
    for lead in ["make that ", "make it ", "turn that into ", "that should be "] {
        if let Some(rest) = p.strip_prefix(lead) {
            if let Some(c) = casing(rest.trim()) {
                return Some(Repair::Recase(c));
            }
        }
    }
    if let Some(rest) = p.strip_suffix(" that") {
        if let Some(c) = casing(rest.trim()) {
            return Some(Repair::Recase(c));
        }
    }

    // "spell it r e a d" / "spell that romeo echo alpha delta"
    for lead in ["spell it ", "spell that ", "spelled ", "spelling "] {
        if let Some(rest) = p.strip_prefix(lead) {
            let s = spell_out(rest);
            if !s.is_empty() {
                return Some(Repair::Spell(s));
            }
            return None;
        }
    }

    // "change foo to bar" / "replace foo with bar"
    for (lead, sep) in [
        ("change ", " to "),
        ("replace ", " with "),
        ("swap ", " for "),
    ] {
        if let Some(rest) = p.strip_prefix(lead) {
            if let Some(i) = rest.find(sep) {
                let from = rest[..i].trim();
                let to = rest[i + sep.len()..].trim();
                if !from.is_empty() && !to.is_empty() {
                    return Some(Repair::Change { from: from.into(), to: to.into() });
                }
            }
        }
    }

    // "I said <x>" / "no I said <x>" — the outright retry.
    for lead in ["no i said ", "i said ", "i meant ", "no i meant "] {
        if let Some(rest) = p.strip_prefix(lead) {
            let r = rest.trim();
            if !r.is_empty() {
                return Some(Repair::Literal(r.into()));
            }
        }
    }

    None
}

/// Apply a repair to the last unit. Returns the replacement text, or
/// None if the repair does not apply (e.g. `Change` whose `from` is not
/// present) — the caller should then tell the user nothing happened
/// rather than silently mangling the line.
///
/// `Join` returns the unit unchanged; the whitespace it removes belongs
/// to the buffer, not the unit, so the sink handles it. It is here so
/// the vocabulary stays in one place.
pub fn apply(unit: &str, r: &Repair) -> Option<String> {
    let u = unit.trim();
    if u.is_empty() {
        return None;
    }
    Some(match r {
        Repair::Recase(c) => {
            // Split on the separators an identifier might already carry,
            // so re-casing an ALREADY-cased unit works: you can say
            // "make that camel case" after "make that snake case"
            // without the underscores becoming part of a word.
            let words: Vec<&str> = u
                .split(|ch: char| ch == '_' || ch == '-' || ch.is_whitespace())
                .filter(|w| !w.is_empty())
                .collect();
            if words.is_empty() {
                return None;
            }
            let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
            let refs: Vec<&str> = lower.iter().map(|s| s.as_str()).collect();
            apply_casing(*c, &refs)
        }
        Repair::Capitalize => {
            let mut cs = u.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => return None,
            }
        }
        Repair::Uppercase => u.to_uppercase(),
        Repair::Lowercase => u.to_lowercase(),
        Repair::Change { from, to } => {
            let out = replace_word(u, from, to)?;
            out
        }
        Repair::Spell(s) => s.clone(),
        Repair::Literal(s) => s.clone(),
        Repair::Join => u.to_string(),
    })
}

/// Replace `from` as a WHOLE word. Substring replacement would turn
/// "change path to route" against `parse_path_buf` into
/// `parse_route_buf`, which is not what anyone means; word boundaries
/// here include identifier separators so `read_to_string` can have its
/// `to` changed.
fn replace_word(unit: &str, from: &str, to: &str) -> Option<String> {
    let is_sep = |c: char| c == '_' || c == '-' || c.is_whitespace() || c == '.' || c == ':';
    let mut out = String::with_capacity(unit.len());
    let mut hit = false;
    let mut start = 0usize;
    let bytes: Vec<(usize, char)> = unit.char_indices().collect();
    let mut i = 0;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let sep = at_end || is_sep(bytes[i].1);
        if sep {
            let end = if at_end { unit.len() } else { bytes[i].0 };
            let word = &unit[start..end];
            if !hit && word.eq_ignore_ascii_case(from) {
                out.push_str(to);
                hit = true;
            } else {
                out.push_str(word);
            }
            if !at_end {
                out.push(bytes[i].1);
                start = bytes[i].0 + bytes[i].1.len_utf8();
            }
        }
        i += 1;
    }
    if hit {
        Some(out)
    } else {
        None
    }
}

/// Turn spoken letters into a string. Accepts bare letters ("r e a d",
/// which whisper often renders "R. E. A. D."), NATO words, and a few
/// common mishearings of single letters.
///
/// NATO matters more than it looks: single letters are the hardest thing
/// for any recognizer — b/d/e/g/p/t/v/z are near-identical over a phone
/// mic — and spelling is exactly the moment the user has already given
/// up on being understood.
pub fn spell_out(s: &str) -> String {
    let mut out = String::new();
    for tok in normalize(s).split_whitespace() {
        let t = tok.trim_matches(|c: char| c == '.' || c == ',');
        if t.is_empty() {
            continue;
        }
        if let Some(c) = nato(t) {
            out.push(c);
        } else if t.chars().count() == 1 {
            let c = t.chars().next().unwrap();
            if c.is_ascii_alphanumeric() {
                out.push(c);
            }
        } else if t == "underscore" {
            out.push('_');
        } else if t == "dash" || t == "hyphen" {
            out.push('-');
        } else if t == "dot" || t == "point" {
            out.push('.');
        }
        // Anything else is dropped: a word that is neither a letter nor
        // a NATO code in the middle of a spelling is almost always a
        // misrecognition, and guessing at it defeats the purpose.
    }
    out
}

fn nato(w: &str) -> Option<char> {
    Some(match w {
        "alpha" | "alfa" => 'a',
        "bravo" => 'b',
        "charlie" => 'c',
        "delta" => 'd',
        "echo" => 'e',
        "foxtrot" => 'f',
        "golf" => 'g',
        "hotel" => 'h',
        "india" => 'i',
        "juliet" | "juliett" => 'j',
        "kilo" => 'k',
        "lima" => 'l',
        "mike" => 'm',
        "november" => 'n',
        "oscar" => 'o',
        "papa" => 'p',
        "quebec" => 'q',
        "romeo" => 'r',
        "sierra" => 's',
        "tango" => 't',
        "uniform" => 'u',
        "victor" => 'v',
        "whiskey" | "whisky" => 'w',
        "xray" | "x-ray" => 'x',
        "yankee" => 'y',
        "zulu" => 'z',
        _ => return None,
    })
}

fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, ',' | '!' | '?' | ';' | ':'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recase_phrasings() {
        assert_eq!(parse("make that snake case"), Some(Repair::Recase(Casing::Snake)));
        assert_eq!(parse("make it camel case"), Some(Repair::Recase(Casing::Camel)));
        assert_eq!(parse("pascal case that"), Some(Repair::Recase(Casing::Pascal)));
        assert_eq!(parse("that should be kebab case"), Some(Repair::Recase(Casing::Kebab)));
    }

    #[test]
    fn recase_works_on_an_already_cased_unit() {
        // the second correction must not treat separators as letters
        let once = apply("read to string", &Repair::Recase(Casing::Snake)).unwrap();
        assert_eq!(once, "read_to_string");
        let twice = apply(&once, &Repair::Recase(Casing::Pascal)).unwrap();
        assert_eq!(twice, "ReadToString");
        let thrice = apply(&twice, &Repair::Recase(Casing::Snake)).unwrap();
        assert_eq!(thrice, "readtostring", "Pascal lost the word boundaries; see the note");
    }

    #[test]
    fn simple_case_verbs() {
        assert_eq!(apply("path", &Repair::Capitalize).as_deref(), Some("Path"));
        assert_eq!(apply("max_size", &Repair::Uppercase).as_deref(), Some("MAX_SIZE"));
        assert_eq!(apply("MAX_SIZE", &Repair::Lowercase).as_deref(), Some("max_size"));
        assert_eq!(parse("capitalize that"), Some(Repair::Capitalize));
        assert_eq!(parse("all caps"), Some(Repair::Uppercase));
    }

    #[test]
    fn change_replaces_a_whole_word_only() {
        let r = Repair::Change { from: "to".into(), to: "into".into() };
        assert_eq!(apply("read_to_string", &r).as_deref(), Some("read_into_string"));
        // must not fire inside another word
        let r2 = Repair::Change { from: "path".into(), to: "route".into() };
        assert_eq!(apply("parse_path_buf", &r2).as_deref(), Some("parse_route_buf"));
        assert_eq!(apply("pathological", &r2), None, "substring must not match");
    }

    #[test]
    fn change_reports_failure_instead_of_mangling() {
        let r = Repair::Change { from: "nope".into(), to: "x".into() };
        assert_eq!(apply("read_to_string", &r), None);
    }

    #[test]
    fn change_only_replaces_the_first_occurrence() {
        let r = Repair::Change { from: "a".into(), to: "b".into() };
        assert_eq!(apply("a_and_a", &r).as_deref(), Some("b_and_a"));
    }

    #[test]
    fn change_phrasings() {
        assert_eq!(
            parse("change foo to bar"),
            Some(Repair::Change { from: "foo".into(), to: "bar".into() })
        );
        assert_eq!(
            parse("replace path with route"),
            Some(Repair::Change { from: "path".into(), to: "route".into() })
        );
        assert_eq!(parse("change foo"), None, "no target, no repair");
    }

    #[test]
    fn spelling_accepts_bare_letters_with_whisper_punctuation() {
        // whisper renders spelled letters as "R. E. A. D."
        assert_eq!(parse("spell it R. E. A. D."), Some(Repair::Spell("read".into())));
        assert_eq!(parse("spell that h x"), Some(Repair::Spell("hx".into())));
    }

    #[test]
    fn spelling_accepts_nato() {
        assert_eq!(
            parse("spell it romeo echo alpha delta"),
            Some(Repair::Spell("read".into()))
        );
        // mixed is fine — people switch to NATO only for the hard letters
        assert_eq!(parse("spell it p a papa h"), Some(Repair::Spell("paph".into())));
    }

    #[test]
    fn spelling_drops_words_it_cannot_read() {
        // a stray misrecognized word must not become letters
        assert_eq!(spell_out("a something b"), "ab");
        assert_eq!(spell_out("underscore t"), "_t");
    }

    #[test]
    fn spell_with_no_content_is_not_a_repair() {
        assert_eq!(parse("spell it"), None);
        assert_eq!(parse("spell it umm"), None);
    }

    #[test]
    fn outright_retry() {
        assert_eq!(parse("i said read to string"), Some(Repair::Literal("read to string".into())));
        assert_eq!(parse("no I meant Path"), Some(Repair::Literal("path".into())));
        assert_eq!(apply("anything", &Repair::Literal("x".into())).as_deref(), Some("x"));
    }

    #[test]
    fn join_leaves_the_unit_alone() {
        assert_eq!(parse("join that"), Some(Repair::Join));
        assert_eq!(apply("bar", &Repair::Join).as_deref(), Some("bar"));
    }

    #[test]
    fn ordinary_content_is_not_a_repair() {
        for s in [
            "let x equals five",
            "return the path",
            "change directory",     // no " to " — a shell command, not a repair
            "spelling matters",     // "spelling " lead with unreadable rest
            "capitalize the first letter of each word",
        ] {
            assert_eq!(parse(s), None, "{s} must pass through as content");
        }
    }

    #[test]
    fn empty_unit_is_never_repaired() {
        assert_eq!(apply("", &Repair::Capitalize), None);
        assert_eq!(apply("   ", &Repair::Uppercase), None);
    }
}
