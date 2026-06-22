//! v0.1.861: self-contained JSON for the ProtonMail bridge.
//!
//! The project deliberately carries **no serde dependency** — the LLM
//! providers (`llm_provider.rs`, `chat.rs`) hand-roll their JSON so that
//! the crate keeps a small, audit-able surface and a clean "drop a
//! feature, keep building" story. The ProtonMail API, however, returns
//! responses that are far more deeply nested than a Claude completion
//! (a user carries an array of keys, each address carries its own key
//! array, messages nest header maps), so the `find("\"content\"")`
//! substring approach used for the flat LLM responses would be brittle
//! here.
//!
//! So this module provides the middle path: a tiny, allocation-light
//! recursive-descent parser into a `Json` value, plus a small builder
//! for request bodies. ~300 lines, zero deps, no macros — the same
//! spirit as the rest of `src/`, just enough structure to address the
//! Proton response tree by path instead of by `str::find`.
//!
//! It is intentionally *not* a general-purpose JSON library: it targets
//! UTF-8 API payloads, rejects the pathological inputs an HTTPS API
//! won't send, and keeps numbers as `f64`/`i64` without bignum support.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A parsed JSON value. `Object` keeps insertion-independent ordering via
/// `BTreeMap` so equality and debug output are stable across runs (handy
/// for the golden-file tests the project favours).
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// All numbers land here. Use [`Json::as_i64`] for the integer IDs the
    /// Proton API uses (timestamps, code, password mode).
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// Parse a UTF-8 JSON document. Returns the value and nothing else —
    /// trailing garbage after the top-level value is an error, which
    /// catches truncated/concatenated API responses early.
    pub fn parse(input: &str) -> Result<Json, String> {
        let mut p = Parser {
            b: input.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("trailing data at byte {}", p.i));
        }
        Ok(v)
    }

    // ---- ergonomic accessors (the "address by path" part) -------------

    /// Object field lookup. Returns `None` on non-objects so chains like
    /// `v.get("Auth").and_then(|a| a.get("UID"))` stay terse.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }

    /// Array index lookup, `None` on non-arrays / out of range.
    pub fn at(&self, idx: usize) -> Option<&Json> {
        match self {
            Json::Arr(a) => a.get(idx),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Integer view of a number. Proton sends IDs and enums as JSON
    /// numbers; this truncates toward zero, matching how a Go `int`
    /// decode behaves on those fields.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Convenience: read a string field, owned. `None` if absent or not a
    /// string. Used heavily when lifting API objects into typed structs.
    pub fn str_field(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Json::as_str).map(str::to_string)
    }

    /// Convenience: read an integer field. `None` if absent or not a num.
    pub fn int_field(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Json::as_i64)
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                _ => break,
            }
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        if self.i >= self.b.len() {
            return Err("unexpected end of input".into());
        }
        match self.b[self.i] {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Json::Str(self.string()?)),
            b't' | b'f' => self.boolean(),
            b'n' => self.null(),
            b'-' | b'0'..=b'9' => self.number(),
            c => Err(format!("unexpected byte {:?} at {}", c as char, self.i)),
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.i < self.b.len() && self.b[self.i] == byte {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at {}", byte as char, self.i))
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut m = BTreeMap::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b'}' {
            self.i += 1;
            return Ok(Json::Obj(m));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(b':')?;
            let val = self.value()?;
            m.insert(key, val);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.i)),
            }
        }
        Ok(Json::Obj(m))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut a = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b']' {
            self.i += 1;
            return Ok(Json::Arr(a));
        }
        loop {
            let val = self.value()?;
            a.push(val);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                Some(b']') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.i)),
            }
        }
        Ok(Json::Arr(a))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            self.i += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or("eof in escape")?;
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{0008}'),
                        b'f' => s.push('\u{000C}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            // Surrogate pair handling: a high surrogate must
                            // be followed by \uXXXX low surrogate.
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.b.get(self.i) == Some(&b'\\')
                                    && self.b.get(self.i + 1) == Some(&b'u')
                                {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    let c = 0x10000
                                        + ((cp - 0xD800) << 10)
                                        + (lo - 0xDC00);
                                    s.push(
                                        char::from_u32(c).ok_or("bad surrogate pair")?,
                                    );
                                } else {
                                    return Err("lone high surrogate".into());
                                }
                            } else {
                                s.push(char::from_u32(cp).ok_or("bad codepoint")?);
                            }
                        }
                        other => return Err(format!("bad escape \\{}", other as char)),
                    }
                }
                // Raw control bytes are technically invalid in JSON strings;
                // an HTTPS API won't send them, so we accept multi-byte
                // UTF-8 continuation bytes here transparently by copying.
                _ => {
                    // Re-decode the UTF-8 sequence starting at c.
                    let start = self.i - 1;
                    let len = utf8_len(c);
                    let end = (start + len).min(self.b.len());
                    if let Ok(part) = std::str::from_utf8(&self.b[start..end]) {
                        s.push_str(part);
                    }
                    self.i = end;
                }
            }
        }
        Err("unterminated string".into())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.b.len() {
            return Err("short \\u escape".into());
        }
        let slice = &self.b[self.i..self.i + 4];
        self.i += 4;
        let txt = std::str::from_utf8(slice).map_err(|_| "bad \\u hex")?;
        u32::from_str_radix(txt, 16).map_err(|_| "bad \\u hex".into())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.b.len()
            && matches!(self.b[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        {
            self.i += 1;
        }
        let txt = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad number")?;
        txt.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number {:?}", txt))
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.b[self.i..].starts_with(b"true") {
            self.i += 4;
            Ok(Json::Bool(true))
        } else if self.b[self.i..].starts_with(b"false") {
            self.i += 5;
            Ok(Json::Bool(false))
        } else {
            Err(format!("bad literal at {}", self.i))
        }
    }

    fn null(&mut self) -> Result<Json, String> {
        if self.b[self.i..].starts_with(b"null") {
            self.i += 4;
            Ok(Json::Null)
        } else {
            Err(format!("bad literal at {}", self.i))
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else if first >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

// ---------------------------------------------------------------------
// Builder side — composing request bodies without serde.
//
// Mirrors the `json_str` helper in llm_provider.rs but generalised so the
// auth/refresh/import requests can be built declaratively.
// ---------------------------------------------------------------------

/// Append a properly-escaped JSON string literal (including quotes) to
/// `out`. Public so other mail_* modules can build ad-hoc fragments the
/// same way the LLM providers do.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A minimal object builder for request payloads. Insertion order is
/// preserved in the emitted JSON, which keeps request bodies stable for
/// the recorded-fixture tests.
#[derive(Default)]
pub struct Obj {
    parts: Vec<(String, String)>,
}

impl Obj {
    pub fn new() -> Self {
        Obj { parts: Vec::new() }
    }

    pub fn str(mut self, key: &str, val: &str) -> Self {
        let mut v = String::new();
        write_str(&mut v, val);
        self.parts.push((key.to_string(), v));
        self
    }

    pub fn int(mut self, key: &str, val: i64) -> Self {
        self.parts.push((key.to_string(), val.to_string()));
        self
    }

    pub fn bool(mut self, key: &str, val: bool) -> Self {
        self.parts.push((key.to_string(), val.to_string()));
        self
    }

    /// Insert a raw, already-serialised JSON fragment (e.g. a nested
    /// object or array produced elsewhere).
    pub fn raw(mut self, key: &str, json_fragment: &str) -> Self {
        self.parts.push((key.to_string(), json_fragment.to_string()));
        self
    }

    pub fn build(&self) -> String {
        let mut out = String::from("{");
        for (i, (k, v)) in self.parts.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_str(&mut out, k);
            out.push(':');
            out.push_str(v);
        }
        out.push('}');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nested() {
        let v = Json::parse(r#"{"Auth":{"UID":"abc","Code":1000},"K":[1,2,3]}"#).unwrap();
        assert_eq!(
            v.get("Auth").and_then(|a| a.str_field("UID")).as_deref(),
            Some("abc")
        );
        assert_eq!(v.get("Auth").and_then(|a| a.int_field("Code")), Some(1000));
        assert_eq!(v.get("K").and_then(|k| k.at(2)).and_then(Json::as_i64), Some(3));
    }

    #[test]
    fn escapes_round_trip() {
        let s = "line1\ntab\tquote\"slash\\done";
        let mut buf = String::new();
        write_str(&mut buf, s);
        let parsed = Json::parse(&buf).unwrap();
        assert_eq!(parsed.as_str(), Some(s));
    }

    #[test]
    fn rejects_trailing() {
        assert!(Json::parse(r#"{"a":1} junk"#).is_err());
    }

    #[test]
    fn builds_object() {
        let body = Obj::new()
            .str("Username", "alice")
            .int("Version", 4)
            .build();
        assert_eq!(body, r#"{"Username":"alice","Version":4}"#);
    }
}
