//! vcard — reading `.vcf` files, so the Import button does something.
//!
//! ## The bug
//!
//! `friends.rs` draws a "📥 Import vCard" button and raises
//! `Action::ImportVCard`. `lib.rs` matched it with `=> {}`. No comment,
//! no toast, no log — a button that a person can press, that looks like
//! every other working button, and that does nothing at all.
//!
//! That is worse than a missing button. A missing feature is visible; a
//! present one that silently declines teaches the person that the app is
//! unreliable, and they cannot tell whether the file was rejected, the
//! import found nothing, or nothing happened.
//!
//! ## Ported, not invented
//!
//! `src/core/VCardIO.cpp` already does this on the Qt side, and its
//! field set is what this follows: `FN` for the display name, `N` as the
//! fallback when `FN` has not been seen, `NOTE`, and the custom
//! `X-SNAPCLONE-ID`. Matching it matters because the two halves read
//! each other's exports — a parser that quietly disagreed about which
//! field wins would round-trip a contact into a different one.
//!
//! Parsing only. Choosing a file and storing the result are the app's
//! job; this takes text and returns contacts, which is what makes the
//! awkward inputs testable without a file dialog.

/// One contact read out of a vCard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedContact {
    /// Display name: `FN` when present, otherwise assembled from `N`.
    pub name: String,
    pub note: String,
    /// `X-SNAPCLONE-ID` — present when the card was exported by
    /// snapclone, absent for a card from any other address book.
    pub snapclone_id: Option<String>,
}

impl ImportedContact {
    /// A card with no name is not a contact. Used to drop entries rather
    /// than import blanks that a person then has to find and delete.
    pub fn is_usable(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// Unfold RFC 6350 line continuations.
///
/// A vCard may wrap a long value onto the next line, marked by a leading
/// space or tab. Exports from phones do this constantly for long notes,
/// so a parser that skips it drops the tail of every wrapped value and
/// looks like it is truncating data at random.
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            let last = out.last_mut().expect("checked non-empty");
            last.push_str(&line[1..]);
        } else {
            out.push(line.to_string());
        }
    }
    out
}

/// Split `PROP;PARAM=x:value` into its property name and value.
///
/// Parameters are dropped: `TEL;TYPE=CELL` and `TEL` are the same
/// property here. Keeping them would mean deciding which TYPE wins, and
/// the Qt side does not read them either — diverging on that is how the
/// two halves would disagree about the same file.
fn split_prop(line: &str) -> Option<(String, String)> {
    let colon = line.find(':')?;
    let (head, rest) = line.split_at(colon);
    let name = head.split(';').next()?.trim().to_ascii_uppercase();
    Some((name, rest[1..].trim().to_string()))
}

/// Read every contact in a vCard file.
///
/// Multiple `BEGIN:VCARD` blocks are normal — exporting a whole address
/// book produces one file with many cards, which is the common way this
/// feature gets used.
///
/// Malformed cards are SKIPPED rather than failing the import. A single
/// bad entry in a 200-contact export should not lose the other 199, and
/// the caller is told how many were dropped so the person can be shown a
/// count rather than a silent difference.
pub fn parse(text: &str) -> (Vec<ImportedContact>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let mut cur: Option<ImportedContact> = None;
    let mut saw_fn = false;

    for line in unfold(text) {
        let upper = line.trim().to_ascii_uppercase();
        if upper.starts_with("BEGIN:VCARD") {
            // An unterminated previous card counts as malformed rather
            // than being silently completed: a truncated file should
            // report the loss, not invent a contact from half of one.
            if cur.is_some() {
                skipped += 1;
            }
            cur = Some(ImportedContact::default());
            saw_fn = false;
            continue;
        }
        if upper.starts_with("END:VCARD") {
            match cur.take() {
                Some(c) if c.is_usable() => out.push(c),
                Some(_) => skipped += 1,
                None => {} // stray END with no BEGIN: ignore
            }
            saw_fn = false;
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        let Some((prop, value)) = split_prop(&line) else { continue };
        match prop.as_str() {
            "FN" => {
                c.name = value;
                saw_fn = true;
            }
            "N" if !saw_fn => {
                // `N:family;given;middle;prefix;suffix` — the Qt side
                // uses "given family", so this does too. Reversing them
                // would sort a re-imported address book differently from
                // the one it came from.
                let parts: Vec<&str> = value.split(';').collect();
                let family = parts.first().copied().unwrap_or("").trim();
                let given = parts.get(1).copied().unwrap_or("").trim();
                c.name = match (given.is_empty(), family.is_empty()) {
                    (false, false) => format!("{given} {family}"),
                    (false, true) => given.to_string(),
                    (true, false) => family.to_string(),
                    (true, true) => String::new(),
                };
            }
            "NOTE" => c.note = value,
            "X-SNAPCLONE-ID" => {
                let t = value.trim();
                if !t.is_empty() {
                    c.snapclone_id = Some(t.to_string());
                }
            }
            _ => {}
        }
    }
    // A file that ended mid-card.
    if cur.is_some() {
        skipped += 1;
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_card_reads() {
        let (cs, skipped) = parse(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada Lovelace\r\nNOTE:met at work\r\nEND:VCARD\r\n",
        );
        assert_eq!(skipped, 0);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].name, "Ada Lovelace");
        assert_eq!(cs[0].note, "met at work");
        assert_eq!(cs[0].snapclone_id, None);
    }

    #[test]
    fn a_whole_address_book_reads_as_many_contacts() {
        // Exporting an address book gives one file with many cards, which
        // is the common way this feature is used at all.
        let text = (0..3)
            .map(|i| format!("BEGIN:VCARD\nFN:Person {i}\nEND:VCARD\n"))
            .collect::<String>();
        let (cs, skipped) = parse(&text);
        assert_eq!(cs.len(), 3, "{cs:?}");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn folded_lines_keep_their_tails() {
        // Phone exports wrap long values constantly. A parser that
        // ignores folding drops the tail of every wrapped value and looks
        // like it truncates data at random.
        let (cs, _) = parse(
            "BEGIN:VCARD\r\nNOTE:this note is long\r\n  and continues here\r\nFN:Bob\r\nEND:VCARD\r\n",
        );
        assert_eq!(cs[0].note, "this note is long and continues here");
    }

    #[test]
    fn fn_wins_over_n_whichever_order_they_appear() {
        // FN is the display name; N is only the fallback. Getting this
        // backwards renames every contact on import.
        let a = parse("BEGIN:VCARD\nFN:Ada L\nN:Lovelace;Ada\nEND:VCARD\n").0;
        let b = parse("BEGIN:VCARD\nN:Lovelace;Ada\nFN:Ada L\nEND:VCARD\n").0;
        assert_eq!(a[0].name, "Ada L");
        assert_eq!(b[0].name, "Ada L", "FN after N must still win");
    }

    #[test]
    fn n_alone_becomes_given_then_family() {
        // Matches the Qt side. Reversing them would sort a re-imported
        // address book differently from the one it came from.
        let (cs, _) = parse("BEGIN:VCARD\nN:Lovelace;Ada;;;\nEND:VCARD\n");
        assert_eq!(cs[0].name, "Ada Lovelace");
    }

    #[test]
    fn parameters_are_ignored_not_treated_as_part_of_the_name() {
        let (cs, _) = parse("BEGIN:VCARD\nFN;CHARSET=UTF-8:Grace H\nEND:VCARD\n");
        assert_eq!(cs[0].name, "Grace H");
    }

    #[test]
    fn one_bad_card_does_not_lose_the_others() {
        // A single malformed entry in a 200-contact export must not cost
        // the other 199 — and the count is reported so a person sees "3
        // imported, 1 skipped" rather than a silent difference.
        let (cs, skipped) = parse(
            "BEGIN:VCARD\nFN:Good One\nEND:VCARD\n\
             BEGIN:VCARD\nNOTE:nameless\nEND:VCARD\n\
             BEGIN:VCARD\nFN:Good Two\nEND:VCARD\n",
        );
        assert_eq!(cs.len(), 2);
        assert_eq!(skipped, 1, "the nameless card was counted, not hidden");
    }

    #[test]
    fn a_truncated_file_reports_the_loss() {
        // Half a card must not become a contact: a download that was cut
        // off should say so rather than inventing an entry from what
        // arrived.
        let (cs, skipped) = parse("BEGIN:VCARD\nFN:Half A Card\n");
        assert!(cs.is_empty(), "{cs:?}");
        assert_eq!(skipped, 1);
    }

    #[test]
    fn a_snapclone_export_round_trips_its_id() {
        // The two halves read each other's files; losing the id would
        // turn a known contact into a stranger on re-import.
        let (cs, _) = parse(
            "BEGIN:VCARD\nFN:Peer\nX-SNAPCLONE-ID:abc123\nEND:VCARD\n",
        );
        assert_eq!(cs[0].snapclone_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn junk_is_not_a_contact() {
        let (cs, skipped) = parse("hello\nthis is not a vcard\n");
        assert!(cs.is_empty());
        assert_eq!(skipped, 0, "no BEGIN means no card was attempted");
    }
}
