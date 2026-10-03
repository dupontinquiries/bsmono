//! US-layout characters to HID boot-keyboard reports.

pub const SHIFT: u8 = 0x02;

pub const ENTER: u8 = 0x28;
pub const ESC: u8 = 0x29;
pub const BACKSPACE: u8 = 0x2A;
pub const TAB: u8 = 0x2B;
pub const SPACE: u8 = 0x2C;
pub const RIGHT: u8 = 0x4F;
pub const LEFT: u8 = 0x50;
pub const DOWN: u8 = 0x51;
pub const UP: u8 = 0x52;

/// (modifier byte, key code) for a printable ASCII character; None for anything else.
pub fn key_for(c: char) -> Option<(u8, u8)> {
    Some(match c {
        'a'..='z' => (0, 0x04 + (c as u8 - b'a')),
        'A'..='Z' => (SHIFT, 0x04 + (c as u8 - b'A')),
        '1'..='9' => (0, 0x1E + (c as u8 - b'1')),
        '0' => (0, 0x27),
        '\n' => (0, ENTER),
        '\t' => (0, TAB),
        ' ' => (0, SPACE),
        '-' => (0, 0x2D),
        '=' => (0, 0x2E),
        '[' => (0, 0x2F),
        ']' => (0, 0x30),
        '\\' => (0, 0x31),
        ';' => (0, 0x33),
        '\'' => (0, 0x34),
        '`' => (0, 0x35),
        ',' => (0, 0x36),
        '.' => (0, 0x37),
        '/' => (0, 0x38),
        '!' => (SHIFT, 0x1E),
        '@' => (SHIFT, 0x1F),
        '#' => (SHIFT, 0x20),
        '$' => (SHIFT, 0x21),
        '%' => (SHIFT, 0x22),
        '^' => (SHIFT, 0x23),
        '&' => (SHIFT, 0x24),
        '*' => (SHIFT, 0x25),
        '(' => (SHIFT, 0x26),
        ')' => (SHIFT, 0x27),
        '_' => (SHIFT, 0x2D),
        '+' => (SHIFT, 0x2E),
        '{' => (SHIFT, 0x2F),
        '}' => (SHIFT, 0x30),
        '|' => (SHIFT, 0x31),
        ':' => (SHIFT, 0x33),
        '"' => (SHIFT, 0x34),
        '~' => (SHIFT, 0x35),
        '<' => (SHIFT, 0x36),
        '>' => (SHIFT, 0x37),
        '?' => (SHIFT, 0x38),
        _ => return None,
    })
}

/// One report: modifiers, reserved, up to six keys.
pub fn report(modifier: u8, key: u8) -> [u8; 8] {
    [modifier, 0, key, 0, 0, 0, 0, 0]
}

pub const RELEASE: [u8; 8] = [0; 8];

/// The reports that type `text`: a press and a release per character (the release between two equal
/// characters is what makes "ll" two keystrokes). Characters with no key are skipped, and returned.
pub fn type_reports(text: &str) -> (Vec<[u8; 8]>, Vec<char>) {
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for c in text.chars() {
        match key_for(c) {
            Some((m, k)) => {
                out.push(report(m, k));
                out.push(RELEASE);
            }
            None => skipped.push(c),
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_shifted_symbols_map_to_the_usb_usage_table() {
        assert_eq!(key_for('a'), Some((0, 0x04)));
        assert_eq!(key_for('z'), Some((0, 0x1D)));
        assert_eq!(key_for('A'), Some((SHIFT, 0x04)));
        assert_eq!(key_for('1'), Some((0, 0x1E)));
        assert_eq!(key_for('9'), Some((0, 0x26)));
        assert_eq!(key_for('0'), Some((0, 0x27)), "zero follows nine in the table");
        assert_eq!(key_for('!'), Some((SHIFT, 0x1E)));
        assert_eq!(key_for('?'), Some((SHIFT, 0x38)));
        assert_eq!(key_for('é'), None);
    }

    #[test]
    fn every_printable_ascii_character_has_a_key_and_none_collide() {
        let mut seen = std::collections::HashMap::new();
        let mut n = 0;
        for b in 0x20u8..0x7F {
            let c = b as char;
            let k = key_for(c).unwrap_or_else(|| panic!("no key for {c:?}"));
            assert!(seen.insert(k, c).is_none(), "{c:?} collides with {:?}", seen[&k]);
            n += 1;
        }
        assert_eq!(n, 95, "the scan covered all printable ASCII");
    }

    #[test]
    fn a_repeated_character_is_two_keystrokes_not_one_held_key() {
        let (r, skipped) = type_reports("ll");
        assert!(skipped.is_empty());
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], r[2]);
        assert_eq!(r[1], RELEASE);
        assert_eq!(r[3], RELEASE);
    }

    #[test]
    fn what_cannot_be_typed_is_reported_not_silently_dropped() {
        let (r, skipped) = type_reports("a€b");
        assert_eq!(skipped, vec!['€']);
        assert_eq!(r.len(), 4);
    }
}
