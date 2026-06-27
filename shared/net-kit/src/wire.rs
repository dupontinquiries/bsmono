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

// ── tagged-record organization of encode/decode ────────────────────────────────
//
// Every message in a line protocol here is a TYPE TAG plus its fields, serialized
// as one escaped tab line ("TAG\tfield\tfield…"). `Record` captures that shape so
// apps stop hand-rolling a `format!`/`split` match per message; the `Wire` trait
// ties an app's message enum to it, getting line encode/decode for free. (This is
// the structure the logscope `Msg` codec writes out by hand.)

/// A wire record: a type tag plus its (raw, unescaped) string fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub tag: String,
    pub fields: Vec<String>,
}

impl Record {
    /// Build from a tag and borrowed fields (numbers via `&x.to_string()`).
    pub fn new(tag: &str, fields: &[&str]) -> Self {
        Self { tag: tag.to_string(), fields: fields.iter().map(|s| s.to_string()).collect() }
    }

    /// Serialize to one escaped tab line: the tag then each field, all `esc`-ed.
    pub fn to_line(&self) -> String {
        let mut parts = Vec::with_capacity(self.fields.len() + 1);
        parts.push(esc(&self.tag));
        parts.extend(self.fields.iter().map(|f| esc(f)));
        parts.join("\t")
    }

    /// Parse a tab line into (tag, fields), unescaped. `None` for an empty line.
    pub fn parse(line: &str) -> Option<Record> {
        if line.trim().is_empty() {
            return None;
        }
        let mut it = decode_fields(line).into_iter();
        let tag = it.next()?;
        Some(Record { tag, fields: it.collect() })
    }
}

/// Map a message type to/from a wire [`Record`], getting line `encode`/`decode`
/// for free. Implement the two record methods; `Record` handles the escaping and
/// the tag/field framing. (A message with an embedded comma-sub-list field builds
/// that field with [`esc`] directly — see `encode_fields`' note.)
pub trait Wire: Sized {
    fn to_record(&self) -> Record;
    fn from_record(rec: &Record) -> Option<Self>;
    fn encode(&self) -> String {
        self.to_record().to_line()
    }
    fn decode(line: &str) -> Option<Self> {
        Self::from_record(&Record::parse(line)?)
    }
}

#[cfg(test)]
mod codec_tests {
    use super::*;

    // a tiny message enum that opts into the wire organization
    #[derive(Debug, PartialEq, Eq)]
    enum Msg {
        Hello { who: String },
        Assign { task: u64, payload: String },
    }

    impl Wire for Msg {
        fn to_record(&self) -> Record {
            match self {
                Msg::Hello { who } => Record::new("HELLO", &[who.as_str()]),
                Msg::Assign { task, payload } => {
                    let t = task.to_string();
                    Record::new("ASSIGN", &[t.as_str(), payload.as_str()])
                }
            }
        }
        fn from_record(r: &Record) -> Option<Self> {
            match (r.tag.as_str(), r.fields.as_slice()) {
                ("HELLO", [who]) => Some(Msg::Hello { who: who.clone() }),
                ("ASSIGN", [task, payload]) => {
                    Some(Msg::Assign { task: task.parse().ok()?, payload: payload.clone() })
                }
                _ => None,
            }
        }
    }

    #[test]
    fn record_roundtrips_with_escaped_fields() {
        let r = Record::new("MSG", &["a\tb", "c,d", "100%"]);
        let line = r.to_line();
        assert_eq!(line.split('\t').count(), 4, "tag + 3 fields, embedded tab escaped");
        assert_eq!(Record::parse(&line), Some(r));
        assert!(Record::parse("   ").is_none());
    }

    #[test]
    fn wire_trait_encode_decode_roundtrip() {
        for m in [
            Msg::Hello { who: "a\tb,c".into() },
            Msg::Assign { task: 42, payload: "x\ny".into() },
        ] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
        assert_eq!(Msg::decode("NOPE\tx"), None, "unknown tag → None");
    }
}
