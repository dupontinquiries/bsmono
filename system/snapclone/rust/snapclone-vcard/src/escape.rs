//! vCard 3.0 value escaping and unescaping.
//!
//! RFC 2426 §5 requires escaping `\`, `,`, `;`, and newlines in property values.

/// Escape a string for use in a vCard property value.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ','  => out.push_str("\\,"),
            ';'  => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}  // strip CR — we output CRLF in the writer
            _    => out.push(ch),
        }
    }
    out
}

/// Unescape a vCard property value.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some(',')  => out.push(','),
                Some(';')  => out.push(';'),
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => { out.push('\\'); out.push(other); }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn roundtrip()       { assert_eq!(unescape(&escape("a,b;c\\d\ne")), "a,b;c\\d\ne"); }
    #[test] fn empty_ok()        { assert_eq!(escape(""), ""); }
    #[test] fn backslash_alone() { assert_eq!(escape("\\"), "\\\\"); }
}
