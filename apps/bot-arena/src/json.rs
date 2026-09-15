//! json — a dependency-free JSON value and parser.
//!
//! Promoted out of `src/bin/jsoncheck.rs` so the lib can read JSON too. This
//! crate deliberately carries no serde (see Cargo.toml: the dep set is kept
//! small enough to build on distro rustc 1.75), and `chatlog` needs to parse
//! multi-megabyte conversation exports, so the choice was promote-or-add-serde
//! and promoting keeps that constraint intact.
//!
//! ## What changed in the move, and why it is not a refactor
//!
//! `\uXXXX` ESCAPES WERE NOT DECODED. The original `string()` had a catch-all
//! `_ => e` on the escape match, which turns `\u2192` into the two-plus-four
//! characters `u2192` — silently, with no parse error. jsoncheck only ever
//! validated and pretty-printed hand-written config, where that never showed;
//! the conversation exports contain 33k+ of them in a 40-file sample (arrows,
//! ellipses, section signs, em dashes), so importing the bug would have
//! corrupted every corpus line carrying one.
//!
//! SURROGATE PAIRS come with it. A `\ud83d\ude80`-style pair encodes one
//! astral-plane char (emoji, which these logs are full of) as two escapes, and
//! decoding each half independently yields two unpaired surrogates — which are
//! not valid `char`s in Rust at all. They are decoded as a pair, and a lone or
//! malformed surrogate becomes U+FFFD rather than failing the whole parse: a
//! 5 MB export with one bad escape should lose one glyph, not the document.
//!
//! ## Cost
//!
//! The parser materialises the input as `Vec<char>` (4 bytes/char), so a 5 MB
//! export peaks around 20 MB before the tree is built. That is fine for a
//! preprocessing stage run once per corpus, and it is why `chatlog` streams
//! one file at a time rather than loading a directory at once.

/// A JSON value. `Obj` keeps insertion order (a `Vec`, not a map) because the
/// export's key order is occasionally meaningful when eyeballing a dump, and
/// nothing here needs sub-linear key lookup on objects this small.
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    /// Field of an object, or `None` for any other kind. Chainable.
    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string behind this value, or `None` if it is not a string.
    ///
    /// Deliberately NOT a stringify: a caller asking for `.as_str()` on a
    /// number wants to know the field was the wrong type, not to receive
    /// `"42"` and carry on.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            J::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            J::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Elements of an array. An empty slice for anything else, because every
    /// caller here loops over the result and `for _ in []` is the correct
    /// behaviour for "this field was absent or the wrong shape".
    pub fn arr(&self) -> &[J] {
        match self {
            J::Arr(a) => a.as_slice(),
            _ => &[],
        }
    }

    /// A string field in one call: `v.str_field("name")`.
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }
}

/// Parse a whole document. Trailing non-whitespace is an error.
pub fn parse(s: &str) -> Result<J, String> {
    let mut p = P { c: s.chars().collect(), i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.c.len() {
        return Err(format!("trailing data at char {}", p.i));
    }
    Ok(v)
}

struct P {
    c: Vec<char>,
    i: usize,
}

impl P {
    fn ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn value(&mut self) -> Result<J, String> {
        self.ws();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(J::Str(self.string()?)),
            Some('t') => self.lit("true", J::Bool(true)),
            Some('f') => self.lit("false", J::Bool(false)),
            Some('n') => self.lit("null", J::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected '{c}' at char {}", self.i)),
            None => Err("unexpected end".into()),
        }
    }

    fn lit(&mut self, word: &str, v: J) -> Result<J, String> {
        for ch in word.chars() {
            if self.peek() != Some(ch) {
                return Err(format!("invalid literal at char {}", self.i));
            }
            self.i += 1;
        }
        Ok(v)
    }

    /// Read four hex digits as a raw UTF-16 code unit. `None` when they are not
    /// four hex digits — the caller substitutes U+FFFD rather than aborting.
    fn hex4(&mut self) -> Option<u32> {
        let mut n: u32 = 0;
        for _ in 0..4 {
            let c = self.peek()?;
            let d = c.to_digit(16)?;
            n = n * 16 + d;
            self.i += 1;
        }
        Some(n)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            self.i += 1;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let e = self.peek().ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'u' => out.push(self.unicode_escape()),
                        // Not a JSON escape at all. The original passed the
                        // character through, which is the lenient reading and
                        // is kept: these exports are machine-generated and a
                        // stray escape is not worth losing a 5 MB file over.
                        other => out.push(other),
                    }
                }
                _ => out.push(ch),
            }
        }
        Err("unterminated string".into())
    }

    /// Decode the body of a `\u` escape, consuming a trailing `\uDC00`-range
    /// low surrogate when this one is a high surrogate.
    ///
    /// Returns U+FFFD for an unpaired or malformed surrogate. That is the
    /// standard substitution and it keeps one bad escape from costing the
    /// whole document — which matters because a single export is thousands of
    /// turns and there is no partial-parse recovery above this.
    fn unicode_escape(&mut self) -> char {
        const REPLACEMENT: char = '\u{FFFD}';
        let Some(hi) = self.hex4() else { return REPLACEMENT };
        // Not a surrogate: a plain BMP code point.
        if !(0xD800..0xE000).contains(&hi) {
            return char::from_u32(hi).unwrap_or(REPLACEMENT);
        }
        // A low surrogate with no high one before it is unpaired.
        if hi >= 0xDC00 {
            return REPLACEMENT;
        }
        // High surrogate: a `\uXXXX` low surrogate must follow. Peek without
        // consuming, so a high surrogate followed by ordinary text leaves that
        // text for the string loop instead of eating two characters of it.
        let save = self.i;
        if self.peek() == Some('\\') {
            self.i += 1;
            if self.peek() == Some('u') {
                self.i += 1;
                if let Some(lo) = self.hex4() {
                    if (0xDC00..0xE000).contains(&lo) {
                        let c = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                        return char::from_u32(c).unwrap_or(REPLACEMENT);
                    }
                }
            }
        }
        self.i = save;
        REPLACEMENT
    }

    fn number(&mut self) -> Result<J, String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || "+-.eE".contains(c) {
                self.i += 1;
            } else {
                break;
            }
        }
        let s: String = self.c[start..self.i].iter().collect();
        s.parse::<f64>().map(J::Num).map_err(|_| format!("bad number '{s}'"))
    }

    fn array(&mut self) -> Result<J, String> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(J::Arr(v));
        }
        loop {
            v.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                _ => return Err(format!("expected ',' or ']' at char {}", self.i)),
            }
        }
    }

    fn object(&mut self) -> Result<J, String> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(J::Obj(v));
        }
        loop {
            self.ws();
            if self.peek() != Some('"') {
                return Err(format!("expected key string at char {}", self.i));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(':') {
                return Err(format!("expected ':' at char {}", self.i));
            }
            self.i += 1;
            v.push((k, self.value()?));
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                _ => return Err(format!("expected ',' or '}}' at char {}", self.i)),
            }
        }
    }
}

/// Render a value back to indented JSON. jsoncheck's pretty-printer, kept with
/// the parser so the two cannot drift.
pub fn pretty(v: &J, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    let pad1 = "  ".repeat(depth + 1);
    match v {
        J::Null => "null".into(),
        J::Bool(b) => b.to_string(),
        J::Num(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        J::Str(s) => format!("{s:?}"),
        J::Arr(a) if a.is_empty() => "[]".into(),
        J::Arr(a) => {
            let items: Vec<String> =
                a.iter().map(|x| format!("{pad1}{}", pretty(x, depth + 1))).collect();
            format!("[\n{}\n{pad}]", items.join(",\n"))
        }
        J::Obj(o) if o.is_empty() => "{}".into(),
        J::Obj(o) => {
            let items: Vec<String> = o
                .iter()
                .map(|(k, x)| format!("{pad1}{k:?}: {}", pretty(x, depth + 1)))
                .collect();
            format!("{{\n{}\n{pad}}}", items.join(",\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_and_invalid() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"]}"#).unwrap();
        assert!(matches!(v, J::Obj(_)));
        assert!(parse(r#"{"a":1,}"#).is_err()); // trailing comma
        assert!(parse(r#"[1 2]"#).is_err()); // missing comma
        assert!(parse(r#""unterminated"#).is_err());
        assert!(parse("  42  ").is_ok());
    }

    /// The bug this module was promoted with, and the reason the promotion was
    /// not a straight copy: a catch-all escape arm turned `\u2192` into the
    /// literal `u2192`. 33k+ of these appear in a 40-file export sample.
    #[test]
    fn unicode_escapes_decode_rather_than_leaking_the_u() {
        let v = parse(r#"{"s":"a\u2192b"}"#).unwrap();
        assert_eq!(v.str_field("s"), Some("a→b"));
        // The exact characters seen in the exports.
        let v = parse(r#""\u2026\u2014\u00a7\u22ef""#).unwrap();
        assert_eq!(v.as_str(), Some("…—§⋯"));
    }

    /// Astral-plane characters arrive as a UTF-16 pair. Decoding the halves
    /// independently yields unpaired surrogates, which are not valid Rust
    /// `char`s — so this has to be handled as a pair or not at all.
    #[test]
    fn surrogate_pairs_become_one_char() {
        let v = parse(r#""\ud83d\ude80""#).unwrap();
        assert_eq!(v.as_str(), Some("🚀"));
        assert_eq!(v.as_str().unwrap().chars().count(), 1);
    }

    /// One malformed escape costs one glyph, not the document. A 5 MB export
    /// with a single bad pair must still parse.
    #[test]
    fn a_lone_surrogate_is_replaced_not_fatal() {
        // High surrogate with no low one following.
        let v = parse(r#""x\ud83dy""#).unwrap();
        assert_eq!(v.as_str(), Some("x\u{FFFD}y"));
        // Lone low surrogate.
        let v = parse(r#""\udc00""#).unwrap();
        assert_eq!(v.as_str(), Some("\u{FFFD}"));
        // Short hex run: not four digits.
        let v = parse(r#""\u12""#).unwrap();
        assert!(v.as_str().unwrap().starts_with('\u{FFFD}'));
    }

    /// A high surrogate followed by ordinary text must not swallow that text
    /// while looking for its pair.
    #[test]
    fn a_failed_pair_lookahead_does_not_eat_the_next_characters() {
        let v = parse(r#""\ud83d\n tail""#).unwrap();
        assert_eq!(v.as_str(), Some("\u{FFFD}\n tail"));
    }

    #[test]
    fn accessors_are_type_checked_not_stringifying() {
        let v = parse(r#"{"n":42,"s":"x","a":[1,2],"o":{"k":"v"}}"#).unwrap();
        assert_eq!(v.str_field("n"), None, "a number is not a string");
        assert_eq!(v.str_field("s"), Some("x"));
        assert_eq!(v.get("a").unwrap().arr().len(), 2);
        assert_eq!(v.get("o").and_then(|o| o.str_field("k")), Some("v"));
        // Absent field and wrong-kind field both yield an empty slice.
        assert!(v.get("missing").is_none());
        assert!(v.get("s").unwrap().arr().is_empty());
    }
}
