//! doc.rs — lifting structure and *dimensioned statements* out of prose. This is what makes
//! "design this to fit `enclosure_spec.md`" resolvable: a spec sentence like "the board is
//! 120 mm x 80 mm" becomes a number with a unit and the line it came from, and the `fit` verb
//! (§17.2) can then present it as a checklist item measured against the actual model.
//!
//! Deliberately a small hand-written scanner rather than a regex dependency: the grammar is tiny,
//! and keeping this crate std-only means a corpus scan has no build cost worth mentioning.

use std::fs;
use std::path::Path;

use crate::{Dimension, DocFacts};

/// Units we recognise, longest first so `mm` is not mistaken for `m`.
///
/// The inch and foot marks (`"` and `'`) are deliberately **absent**. They were here, and running
/// the `fit` verb over the real corpus showed what that costs: prose like `"===1" meaning` parsed
/// as one inch, because a closing quotation mark after a digit is indistinguishable from an inch
/// mark without understanding the sentence. In a tree full of developer documentation, quotation
/// marks outnumber inch marks by orders of magnitude, so the mark is dropped and `in` / `inch`
/// carry the unit instead. A spec that writes 5" simply will not be picked up, which is the
/// direction to err: a missed requirement is visible, a fabricated one is not.
/// Recognised units, longest first so `inch` is matched before `in` and `mm` before `m`. The flag
/// is `tight`: whether the unit must abut the number with no space.
///
/// `in` is tight-only, and that too came from running `fit` over the real corpus. "in" is an
/// English preposition, so `level 1 in ~0.5` and `real git 2.43 in ` both parsed as inches. No
/// amount of local scanning can tell a unit from a preposition — only the sentence can — so the
/// abbreviation is accepted only when bound to its number (`5in`), while the spelled-out `inch`
/// and `inches` are unambiguous and may be spaced. A spec writing `5 in` is therefore missed,
/// which is the right way to be wrong: a missing requirement is visible in a checklist, an
/// invented one silently becomes a constraint nobody wrote.
const UNITS: [(&str, bool); 8] = [
    ("inches", false),
    ("inch", false),
    ("thou", false),
    ("deg", false),
    ("mm", false),
    ("cm", false),
    ("in", true),
    ("m", false),
];

/// Collapse spellings to one canonical unit, so a checklist does not report `inch` and `in` as
/// two different things.
fn canonical(u: &str) -> &'static str {
    match u {
        "inches" | "inch" | "in" => "in",
        "mm" => "mm",
        "cm" => "cm",
        "m" => "m",
        "thou" => "thou",
        _ => "deg",
    }
}

pub fn facts(path: &Path) -> Option<DocFacts> {
    // Cap the read: a corpus can contain a very large log, and nothing past the first stretch of a
    // document is useful as a design reference.
    let text = fs::read_to_string(path).ok()?;
    Some(parse(&text[..text.len().min(256 * 1024)]))
}

pub fn parse(text: &str) -> DocFacts {
    let mut f = DocFacts::default();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            if f.title.is_none() {
                f.title = Some(rest.trim().to_string());
            }
            f.headings.push(rest.trim().to_string());
        } else if t.starts_with("##") {
            f.headings.push(t.trim_start_matches('#').trim().to_string());
        }
        scan_dimensions(t, &mut f.dimensions);
    }
    f
}

/// Find `<number><optional space><unit>` runs in a line.
fn scan_dimensions(line: &str, out: &mut Vec<Dimension>) {
    let b: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if !(b[i].is_ascii_digit() || (b[i] == '.' && i + 1 < b.len() && b[i + 1].is_ascii_digit())) {
            i += 1;
            continue;
        }
        // A number may not start mid-token (e.g. the "3" of "M3" is a thread callout, not a length).
        if i > 0 && (b[i - 1].is_alphabetic() || b[i - 1] == '_') {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
            i += 1;
        }
        let num: String = b[start..i].iter().collect();
        let Ok(value) = num.parse::<f32>() else { continue };
        let spaced = i < b.len() && b[i] == ' ';
        let j = if spaced { i + 1 } else { i };
        let tail: String = b[j..].iter().collect::<String>().to_ascii_lowercase();
        if let Some((unit, _)) = UNITS.iter().find(|(u, tight)| {
            (!*tight || !spaced)
                && tail.starts_with(u)
                && !tail[u.len()..].starts_with(|c: char| c.is_alphanumeric())
        }) {
            out.push(Dimension {
                value,
                unit: canonical(unit).to_string(),
                context: line.trim().to_string(),
            });
            i = j + unit.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_title_headings_and_dimensions() {
        let f = parse("# Enclosure spec\n\nThe board is 120 mm x 80mm.\n## Clearances\nWall >= 2.5mm, M3 screws.\n");
        assert_eq!(f.title.as_deref(), Some("Enclosure spec"));
        assert_eq!(f.headings, vec!["Enclosure spec".to_string(), "Clearances".to_string()]);
        let vals: Vec<f32> = f.dimensions.iter().map(|d| d.value).collect();
        assert_eq!(vals, vec![120.0, 80.0, 2.5], "{:?}", f.dimensions);
        assert!(f.dimensions.iter().all(|d| d.unit == "mm"));
        // Context is kept so a checklist can quote its source.
        assert!(f.dimensions[0].context.contains("board"));
    }

    #[test]
    fn thread_callouts_are_not_lengths() {
        let f = parse("Use M3 and M4 fasteners throughout.");
        assert!(f.dimensions.is_empty(), "{:?}", f.dimensions);
    }

    #[test]
    fn quotation_marks_in_prose_are_not_inches() {
        // Regression: found by running `fit` over the real corpus, where changelog prose full of
        // quoted strings produced a checklist of phantom inch dimensions.
        let f = parse("Tabs stop being \"1, 2, 3\". The `===1` stub keeps \"1\" meaning one.");
        assert!(f.dimensions.is_empty(), "{:?}", f.dimensions);
    }

    #[test]
    fn inches_need_either_tight_binding_or_the_spelled_out_word() {
        let f = parse("The shaft is 2in across and the plate is 0.5 inch thick.");
        assert_eq!(f.dimensions.len(), 2, "{:?}", f.dimensions);
        assert!(f.dimensions.iter().all(|d| d.unit == "in"));
    }

    #[test]
    fn in_as_a_preposition_is_not_a_measurement() {
        // Regression from the real corpus: changelog prose measured in phantom inches.
        let f = parse("Measured ~4.9x smaller at level 1 in ~0.5 seconds, with real git 2.43 in CI.");
        assert!(f.dimensions.is_empty(), "{:?}", f.dimensions);
    }

    #[test]
    fn a_bare_number_is_not_a_dimension() {
        let f = parse("There are 4 mounting points and 12 pins.");
        assert!(f.dimensions.is_empty(), "{:?}", f.dimensions);
    }
}
