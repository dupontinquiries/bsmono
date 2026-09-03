//! IDS — where a line's permanent name comes from.
//!
//! A unit id becomes a WAV filename, a key in a manifest, and a reference from
//! a shipped asset. It is **permanent**: [`Fault::OrphanTake`](crate::Fault)
//! exists because a rename after recording orphans every take that answered to
//! the old name. So the question of where ids come from is not a convenience
//! question.
//!
//! ## Three sources, and why this one
//!
//! **From position** — `content_rs_line_412`, or an index into a table. Fails
//! immediately: inserting a line renumbers every line after it, which renames
//! ids that are supposed to be permanent. A script edit would orphan half the
//! bank.
//!
//! **Authored by hand** — stable and meaningful, and the right answer for the
//! two dozen lines anyone actually cares about. It does not scale to a bank of
//! blips and carrier expansions, where the id is going to be a mechanical
//! transform of the text no matter who types it.
//!
//! **From content**, which is what this does: the speaker plus the opening
//! words. Stable under reordering, because it does not know where the line
//! sits. Unstable under rewriting the opening words — and that is the correct
//! behaviour rather than a flaw, because rewriting a line means the recording
//! is wrong and a re-record is due. The id changing is the signal.
//!
//! The failure mode it accepts: fixing a typo in the first four words renames
//! the unit and orphans its takes. That is loud (`faults()` reports it) rather
//! than silent, and it is rarer than reordering, which is the one this is
//! chosen to survive.
//!
//! ## Filename safety is not a nicety here
//!
//! Ids become files on filesystems that are case-insensitive (Windows, macOS
//! by default), so `Billy_Ack` and `billy_ack` are ONE file and the second
//! take silently overwrites the first. Everything here is lowercased for that
//! reason, and `Bank::faults` checks it again at the collection level because
//! two different texts can slug to the same thing.

/// Longest id this will produce, before any collision suffix. Long enough to
/// stay readable in a directory listing, short enough to survive path limits
/// once a session folder and an extension are added.
pub const MAX_LEN: usize = 48;

/// How many words of the line go into the id. Four is enough to be
/// recognisable and few enough that editing later words does not rename the
/// unit.
pub const WORDS: usize = 4;

/// Reduce to lowercase ASCII alphanumerics and single underscores.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            out.extend(c.to_lowercase());
            gap = false;
        } else {
            // Non-ASCII is dropped rather than transliterated: a guess at what
            // "ü" should become differs by language, and a wrong guess is
            // permanent. The words around it still identify the line.
            gap = true;
        }
    }
    out
}

/// The id for a line: speaker, then its opening words.
///
/// Returns an empty string only if BOTH speaker and text reduce to nothing,
/// which a caller should treat as a fault rather than a name.
pub fn derive(speaker: &str, text: &str) -> String {
    let who = slugify(speaker);
    let what: Vec<String> =
        slugify(text).split('_').filter(|w| !w.is_empty()).take(WORDS).map(String::from).collect();
    let mut id = match (who.is_empty(), what.is_empty()) {
        (true, true) => return String::new(),
        (true, false) => what.join("_"),
        (false, true) => who,
        (false, false) => format!("{who}_{}", what.join("_")),
    };
    if id.len() > MAX_LEN {
        id.truncate(MAX_LEN);
        // never end on a partial word or a trailing separator
        while id.ends_with('_') {
            id.pop();
        }
        if let Some(cut) = id.rfind('_') {
            if id.len() == MAX_LEN {
                id.truncate(cut);
            }
        }
    }
    id
}

/// `derive`, with a numeric suffix if that id is already taken.
///
/// Two different lines can legitimately slug to the same thing — a blip
/// described as "(short acknowledgement)" twice, or two carrier reads whose
/// opening words match. The suffix is assigned in the order units are added,
/// so it is stable as long as the script's order is.
pub fn derive_unique<'a, I>(speaker: &str, text: &str, taken: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let base = derive(speaker, text);
    let existing: Vec<String> = taken.into_iter().map(|s| s.to_lowercase()).collect();
    if base.is_empty() || !existing.iter().any(|t| *t == base) {
        return base;
    }
    for n in 2u32.. {
        let cand = format!("{base}_{n}");
        if !existing.iter().any(|t| *t == cand) {
            return cand;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bank, Fault, Kind, Unit};

    #[test]
    fn an_id_is_readable_in_a_directory_listing() {
        // WORDS is 4, so four words of the line follow the speaker.
        assert_eq!(derive("Billy", "You have one shot."), "billy_you_have_one_shot");
        assert_eq!(
            derive("Keelwright", "It believes in consequence."),
            "keelwright_it_believes_in_consequence"
        );
    }

    #[test]
    fn reordering_the_script_does_not_rename_anything() {
        // THE PROPERTY THIS DESIGN IS CHOSEN FOR. A position-derived id
        // renames every line after an insertion, and ids are permanent.
        let lines = [("Billy", "You have one shot."), ("Billy", "I know."), ("Jib", "Hold on.")];
        let forward: Vec<String> = lines.iter().map(|(s, t)| derive(s, t)).collect();
        let mut shuffled = lines;
        shuffled.reverse();
        let backward: Vec<String> = shuffled.iter().map(|(s, t)| derive(s, t)).collect();
        for id in &forward {
            assert!(backward.contains(id), "{id} changed when the script was reordered");
        }
    }

    #[test]
    fn editing_a_late_word_keeps_the_id_and_editing_an_early_one_does_not() {
        // Deliberate. Rewriting the opening of a line means the recording is
        // wrong, so the id changing is the signal that a re-record is due —
        // and faults() reports the orphaned takes rather than hiding them.
        let a = derive("Billy", "You have one shot at this");
        let b = derive("Billy", "You have one shot at everything");
        assert_eq!(a, b, "a change beyond the first four words renamed the unit");
        let c = derive("Billy", "You get one shot at this");
        assert_ne!(a, c, "a change to the opening words should rename the unit");
    }

    #[test]
    fn punctuation_case_and_accents_cannot_produce_an_unsafe_name() {
        for (s, t) in [
            ("BILLY", "You/have\\one*shot?"),
            ("Billy ", "  you   have  one  shot  "),
            ("Meridian", "Café — déjà vu"),
            ("Jib", "50% chance!"),
        ] {
            let id = derive(s, t);
            assert!(!id.is_empty(), "{s}/{t} produced nothing");
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{id} is not filename-safe"
            );
            assert!(!id.starts_with('_') && !id.ends_with('_'), "{id} has a loose separator");
            assert!(!id.contains("__"), "{id} has a doubled separator");
        }
    }

    #[test]
    fn case_only_differences_collapse_rather_than_colliding_on_disk() {
        // Windows and macOS treat these as one file, so they must be one id
        // here and be caught as a duplicate rather than silently overwriting.
        assert_eq!(derive("Billy", "One shot"), derive("BILLY", "ONE SHOT"));
    }

    #[test]
    fn a_long_line_is_cut_at_a_word_boundary() {
        let id = derive(
            "Keelwright",
            "Orbits that turn without concern for those who live upon them",
        );
        assert!(id.len() <= MAX_LEN, "{} chars: {id}", id.len());
        assert!(!id.ends_with('_'));
        // and it is still recognisable as the line it came from
        assert!(id.starts_with("keelwright_orbits"));
    }

    #[test]
    fn two_lines_that_slug_the_same_get_stable_suffixes() {
        let taken: Vec<String> = vec![];
        let a = derive_unique("Billy", "(short acknowledgement)", taken.iter().map(|s| s.as_str()));
        let held = vec![a.clone()];
        let b = derive_unique("Billy", "(short acknowledgement)", held.iter().map(|s| s.as_str()));
        assert_eq!(a, "billy_short_acknowledgement");
        assert_eq!(b, "billy_short_acknowledgement_2");
        let held2 = vec![a.clone(), b.clone()];
        let c = derive_unique("Billy", "(short acknowledgement)", held2.iter().map(|s| s.as_str()));
        assert_eq!(c, "billy_short_acknowledgement_3");
    }

    #[test]
    fn a_derived_bank_has_no_id_faults() {
        // The end of the argument: ids produced this way pass the collection
        // level checks that exist because filenames collide.
        let lines = [
            ("Billy", "You have one shot.", Kind::Phrase),
            ("Billy", "(short acknowledgement)", Kind::Blip),
            ("BILLY", "You/have/one/shot!", Kind::Phrase), // slugs the same
            ("Keelwright", "It believes in consequence.", Kind::Phrase),
        ];
        let mut b = Bank::default();
        let mut ids: Vec<String> = Vec::new();
        for (s, t, k) in lines {
            let id = derive_unique(s, t, ids.iter().map(|x| x.as_str()));
            ids.push(id.clone());
            b.add(Unit::new(&id, s, t, k).wanting(1));
        }
        let id_faults: Vec<&Fault> = b
            .faults()
            .iter()
            .filter(|f| matches!(f, Fault::DuplicateId(_) | Fault::UnsafeId(_)))
            .count()
            .eq(&0)
            .then(Vec::new)
            .unwrap_or_else(|| panic!("derived ids produced id faults: {:?}", b.faults()));
        assert!(id_faults.is_empty());
    }

    #[test]
    fn nothing_at_all_yields_an_empty_id_rather_than_a_bad_one() {
        // A caller must treat this as a fault, not a name. Returning "unknown"
        // would put several unrelated lines in one file.
        assert_eq!(derive("", "!!!"), "");
        assert_eq!(derive("", ""), "");
        // and a speaker alone is still a usable name
        assert_eq!(derive("Billy", "???"), "billy");
    }
}
