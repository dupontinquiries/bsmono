//! second_layer — more keys on the standard letter layouts, one long-press away.
//!
//! The standard layouts had a second layer on the TOP row only (digits on a
//! long-press of q..p); the home and bottom rows carried nothing but a few
//! accents, so the symbols a phone keyboard keeps under the letters (@ # $ _
//! & - + ( ) * " ' : ; ! ?) meant a trip to the ?123 page.
//!
//! The layer is by POSITION, not by letter (Gboard's rule): the key in QWERTY's
//! `s` column gives `#` whatever letter sits there, so the symbols stay under
//! the same fingers in QWERTY, Dvorak or any other standard layout. Position
//! is the key's column among its row's character keys (single visible
//! characters: `' , .` count, `⇧` and `⌫` do not), counted over the rows that
//! hold letters. Only single-LETTER keys get a symbol; a punctuation key keeps
//! its own behaviour. The symbol goes FIRST in the long-press list (what a hold
//! shows first, and what a hint can draw); existing variants -- accents -- stay
//! after it, and nothing is listed twice.
//!
//! Pure functions over labels, so every key representation (this crate's
//! `Layout`, the egui frontend's own) applies the same table.

/// The layer, per letter row: top, home, bottom.
pub const LAYER: [&[&str]; 3] = [
    &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
    &["@", "#", "$", "_", "&", "-", "+", "(", ")"],
    &["*", "\"", "'", ":", ";", "!", "?"],
];

/// One alphabetic character.
pub fn is_letter(label: &str) -> bool {
    let mut c = label.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if ch.is_alphabetic())
}

/// One visible character (a column in the layer): letters and punctuation keys.
fn is_char_key(label: &str) -> bool {
    let mut c = label.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if !ch.is_whitespace() && (ch.is_alphanumeric() || ch.is_ascii_punctuation()))
}

/// The symbol each key gets, row by row (None: no second-layer symbol).
pub fn plan(rows: &[Vec<&str>]) -> Vec<Vec<Option<&'static str>>> {
    let mut letter_row = 0;
    rows.iter()
        .map(|labels| {
            if !labels.iter().any(|l| is_letter(l)) {
                return vec![None; labels.len()];
            }
            let table = LAYER.get(letter_row).copied();
            letter_row += 1;
            let mut col = 0;
            labels.iter()
                .map(|l| {
                    if !is_char_key(l) {
                        return None;
                    }
                    let c = col;
                    col += 1;
                    if is_letter(l) { table.and_then(|t| t.get(c).copied()) } else { None }
                })
                .collect()
        })
        .collect()
}

/// `sym` first, then the existing variants without it.
pub fn merged(existing: &[String], sym: &str) -> Vec<String> {
    let mut v = vec![sym.to_string()];
    v.extend(existing.iter().filter(|s| s.as_str() != sym).cloned());
    v
}

impl crate::Layout {
    /// This layout with the second layer in its keys' long-press lists
    /// (`extended`, and `extended_shifted` where the key has one).
    pub fn with_second_layer(mut self) -> Self {
        let labels: Vec<Vec<&str>> = self.rows.iter().map(|r| r.keys.iter().map(|k| k.label.as_str()).collect()).collect();
        let owned: Vec<Vec<Option<&'static str>>> = plan(&labels);
        for (row, syms) in self.rows.iter_mut().zip(owned) {
            for (key, sym) in row.keys.iter_mut().zip(syms) {
                if let Some(s) = sym {
                    key.extended = merged(&key.extended, s);
                    if !key.extended_shifted.is_empty() {
                        key.extended_shifted = merged(&key.extended_shifted, s);
                    }
                }
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwerty_gets_symbols_under_the_home_and_bottom_rows() {
        let rows = vec![
            "q w e r t y u i o p".split(' ').collect::<Vec<_>>(),
            "a s d f g h j k l".split(' ').collect(),
            "⇧ z x c v b n m ⌫".split(' ').collect(),
            vec!["?123", " ", ",", "."],
        ];
        let p = plan(&rows);
        assert_eq!(p[0][0], Some("1"));
        assert_eq!(p[0][9], Some("0"));
        assert_eq!(p[1][1], Some("#"), "s");
        assert_eq!(p[1][8], Some(")"), "l");
        assert_eq!(p[2][0], None, "shift is not a column");
        assert_eq!(p[2][1], Some("*"), "z");
        assert_eq!(p[2][7], Some("?"), "m");
        assert_eq!(p[2][8], None, "backspace");
        assert!(p[3].iter().all(|s| s.is_none()), "the bottom row has no letters");
    }

    #[test]
    fn the_layer_follows_position_so_dvorak_keeps_the_same_fingers() {
        let rows = vec![
            vec!["'", ",", ".", "p", "y", "f", "g", "c", "r", "l"],
            vec!["a", "o", "e", "u", "i", "d", "h", "t", "n", "s"],
        ];
        let p = plan(&rows);
        assert_eq!(p[0][0], None, "a punctuation key keeps its own behaviour");
        assert_eq!(p[0][3], Some("4"), "p sits in QWERTY's r column");
        assert_eq!(p[1][1], Some("#"), "o sits in QWERTY's s column");
        assert_eq!(p[1][9], None, "a tenth home-row key has no symbol in the table");
    }

    #[test]
    fn merging_puts_the_symbol_first_and_never_lists_it_twice() {
        let e = vec!["à".to_string(), "@".to_string(), "á".to_string()];
        assert_eq!(merged(&e, "@"), vec!["@", "à", "á"]);
        assert_eq!(merged(&["1".to_string(), "é".to_string()], "1"), vec!["1", "é"]);
        assert_eq!(merged(&[], "#"), vec!["#"]);
    }

    #[test]
    fn a_page_without_letters_is_untouched() {
        let p = plan(&[vec!["1", "2", "3"], vec!["@", "#", "&"]]);
        assert!(p.iter().flatten().all(|s| s.is_none()));
    }

    #[test]
    fn the_real_qwerty_gets_it_and_keeps_its_accents() {
        let l = crate::Layout::default_qwerty().with_second_layer();
        let key = |label: &str| l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == label).unwrap().clone();
        assert_eq!(key("s").extended, vec!["#"]);
        assert_eq!(key("a").extended.first().map(String::as_str), Some("@"));
        assert!(key("a").extended.contains(&"à".to_string()), "accents kept");
        assert_eq!(key("e").extended.iter().filter(|s| *s == "3").count(), 1, "not doubled");
        assert_eq!(key("m").extended, vec!["?"]);
    }
}
