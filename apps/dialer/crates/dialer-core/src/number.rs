//! Phone-number hygiene. Deliberately *not* E.164/libphonenumber: without a
//! region database, "smart" formatting is wrong more often than none, and a
//! dialer that mangles what the user typed is worse than one that keeps it.
//! We do exactly two things:
//!
//! * [`normalize`] — what goes to the modem and the history: keep dialable
//!   characters (`0-9 + * #`, and `,`/`;` for post-dial pauses some modems
//!   honour), drop visual separators.
//! * [`display_group`] — what the readout shows: the normalized string in
//!   groups of three/four so long numbers stay scannable, with `+` runs kept.

/// Characters a modem can dial.
pub fn is_dialable(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '+' | '*' | '#' | ',' | ';')
}

/// Strip visual separators, keep dialable characters in order.
pub fn normalize(input: &str) -> String {
    input.chars().filter(|c| is_dialable(*c)).collect()
}

/// A normalized number is worth sending to the modem.
pub fn is_dialworthy(normalized: &str) -> bool {
    normalized.chars().any(|c| c.is_ascii_digit())
}

/// Group digits for the readout: `+15551234567` → `+1 555 123 4567`-ish
/// (last group gets the remainder). Non-digit dialables break groups.
pub fn display_group(normalized: &str) -> String {
    let digits: Vec<char> = normalized.chars().collect();
    let n_digits = digits.iter().filter(|c| c.is_ascii_digit()).count();
    if n_digits <= 4 {
        return normalized.to_string();
    }
    let mut out = String::new();
    let mut run = 0usize; // digits since last space
    let mut seen = 0usize;
    for c in digits {
        if c.is_ascii_digit() {
            // Break before this digit if the current run is full. Group size:
            // 3, except let the final group absorb up to 4 so we never end on
            // a lone digit.
            let remaining = n_digits - seen;
            if run == 3 && remaining > 1 && !(run + remaining <= 4) {
                out.push(' ');
                run = 0;
            }
            out.push(c);
            run += 1;
            seen += 1;
        } else {
            out.push(c);
            run = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_dialables_and_drops_decoration() {
        assert_eq!(normalize("+1 (555) 123-4567"), "+15551234567");
        assert_eq!(normalize("*#06#"), "*#06#");
        assert_eq!(normalize("555 1234,,42"), "5551234,,42");
        assert!(!is_dialworthy(&normalize("call me")));
        assert!(is_dialworthy("112"));
    }

    #[test]
    fn short_numbers_are_left_alone() {
        assert_eq!(display_group("112"), "112");
        assert_eq!(display_group("1234"), "1234");
    }

    #[test]
    fn long_numbers_group_without_a_trailing_orphan() {
        assert_eq!(display_group("5551234"), "555 1234");
        // Region-blind grouping: threes with a 2–4 digit tail, never a lone digit.
        assert_eq!(display_group("+15551234567"), "+155 512 345 67");
    }
}
