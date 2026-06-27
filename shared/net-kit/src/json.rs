//! json.rs — dependency-free JSON helpers for flat API replies (the ollama
//! client, the HF-hub metadata fetch): escape a string for a request body, and
//! pull a top-level string field out of a response WITHOUT a full JSON parser or
//! serde. Enough for flat `{"k":"v", …}` shapes; not a general JSON parser. Std
//! only. Lifted from the logscope ollama client.

/// JSON-escape a string for embedding in a request body.
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Extract a top-level JSON string field's (unescaped) value. Scans the object
/// reading whole string tokens, so the key is never matched inside another
/// field's value; returns the first `"key": "..."` it finds. Not a full parser —
/// enough for a flat reply shape.
pub fn find_string_field(json: &str, key: &str) -> Option<String> {
    let c: Vec<char> = json.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            let (s, next) = read_json_string(&c, i)?;
            let mut j = next;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && c[j] == ':' {
                // this string was an object key
                j += 1;
                while j < c.len() && c[j].is_whitespace() {
                    j += 1;
                }
                if s == key {
                    return if j < c.len() && c[j] == '"' {
                        read_json_string(&c, j).map(|(v, _)| v)
                    } else {
                        None // value isn't a string
                    };
                }
                i = j; // skip to the value; it'll be scanned next
            } else {
                i = next; // string was a value/element
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Read a JSON string literal starting at the opening quote `c[i]`; returns
/// (unescaped, index-after-close). Handles the standard escapes incl. `\uXXXX`.
pub fn read_json_string(c: &[char], mut i: usize) -> Option<(String, usize)> {
    if c.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    let mut out = String::new();
    while i < c.len() {
        match c[i] {
            '"' => return Some((out, i + 1)),
            '\\' => {
                i += 1;
                match c.get(i)? {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{C}'),
                    'u' => {
                        let h: String = c.get(i + 1..i + 5)?.iter().collect();
                        let cp = u32::from_str_radix(&h, 16).ok()?;
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        i += 4;
                    }
                    _ => return None,
                }
                i += 1;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_covers_specials() {
        assert_eq!(escape("a\"b\\c\nd\te"), "a\\\"b\\\\c\\nd\\te");
        assert_eq!(escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn extracts_field_and_unescapes() {
        // "model" string precedes "response"; the extractor must skip it and unescape.
        let body = r#"{"model":"qwen2","response":"hello\nworld \"q\"","done":true}"#;
        assert_eq!(find_string_field(body, "response").as_deref(), Some("hello\nworld \"q\""));
    }

    #[test]
    fn not_fooled_by_key_inside_a_value() {
        // the literal text `response":` appears INSIDE another field's value
        let body = r#"{"note":"see response: nothing","response":"real"}"#;
        assert_eq!(find_string_field(body, "response").as_deref(), Some("real"));
    }

    #[test]
    fn missing_field_is_none() {
        assert_eq!(find_string_field(r#"{"a":"b"}"#, "z"), None);
    }

    #[test]
    fn decodes_unicode_escape() {
        let c: Vec<char> = r#""caf\u00e9""#.chars().collect();
        assert_eq!(read_json_string(&c, 0).unwrap().0, "café");
    }
}
