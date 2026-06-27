//! wire.rs — a dependency-free line/datagram codec: percent-escape the bytes
//! significant to the framing (`%`, tab, newline, comma) so arbitrary string
//! fields ride one tab-delimited line — one UDP datagram, one log line. This is
//! the escaping the logscope fleet codec and the voiceterm/unendlich wire formats
//! all hand-rolled. Std only.

/// Percent-escape the framing-significant bytes (`%`, tab, newline, comma) so a
/// field — or a comma-joined sub-list element — can contain any text. Pairs with
/// [`unesc`]. Byte-identical to the logscope enlist codec's `esc`.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => o.push_str("%25"),
            '\t' => o.push_str("%09"),
            '\n' => o.push_str("%0A"),
            ',' => o.push_str("%2C"),
            _ => o.push(c),
        }
    }
    o
}

/// Reverse [`esc`]. A lone `%` not followed by two hex digits is passed through.
pub fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '%' {
            let h: String = it.clone().take(2).collect();
            if h.len() == 2 {
                if let Ok(b) = u8::from_str_radix(&h, 16) {
                    o.push(b as char);
                    it.next();
                    it.next();
                    continue;
                }
            }
        }
        o.push(c);
    }
    o
}

/// Join fields into one tab-delimited line, each field [`esc`]-escaped. Pairs
/// with [`decode_fields`]. The convenience for the flat case (a record-type tag
/// plus simple string fields), which is what most messages are.
///
/// NOTE: a field that is itself a comma-sub-list (e.g. enlist's `models`) can't
/// go through `encode_fields` — esc would escape its delimiter commas. Build such
/// a line manually with [`esc`] per element joined by `,`, exactly as the
/// logscope `Msg` codec does; `encode_fields` is only for all-simple fields.
pub fn encode_fields(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| esc(f))
        .collect::<Vec<_>>()
        .join("\t")
}

/// Split a tab-delimited line into [`unesc`]-decoded fields (trailing CR/LF
/// trimmed first, so a datagram or a read line both parse).
pub fn decode_fields(line: &str) -> Vec<String> {
    line.trim_end_matches(['\r', '\n'])
        .split('\t')
        .map(unesc)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_unesc_roundtrips_specials() {
        for s in ["plain", "a\tb", "a\nb", "a,b", "100%", "a\t,%\nz", ""] {
            assert_eq!(unesc(&esc(s)), s, "roundtrip {s:?}");
        }
    }

    #[test]
    fn fields_roundtrip_through_a_tab_line() {
        let fields = ["MSG", "hello, world", "tab\there", "65%"];
        let line = encode_fields(&fields);
        // the embedded tab and comma were escaped, so the line splits into exactly 4
        assert_eq!(line.split('\t').count(), 4, "embedded tab is escaped, not a delimiter");
        assert_eq!(decode_fields(&line), fields.map(String::from).to_vec());
    }

    #[test]
    fn decode_trims_trailing_crlf() {
        assert_eq!(decode_fields("a\tb\r\n"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn lone_percent_passes_through() {
        // not a valid %XX escape → left as-is by unesc
        assert_eq!(unesc("50% off"), "50% off");
    }
}
