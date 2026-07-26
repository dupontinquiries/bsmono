//! Minimal JSON reader — just enough for glTF, with no dependencies.
//!
//! Deliberately not a general parser: it does objects, arrays, strings, numbers,
//! bools and null, which is the whole of the glTF schema. Writing it by hand keeps
//! the crate dependency-free, which is the reason the rest of this port compiles
//! anywhere.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(HashMap<String, J>),
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        match self { J::Obj(m) => m.get(k), _ => None }
    }
    pub fn idx(&self, i: usize) -> Option<&J> {
        match self { J::Arr(v) => v.get(i), _ => None }
    }
    pub fn arr(&self) -> &[J] {
        match self { J::Arr(v) => v, _ => &[] }
    }
    pub fn u(&self, k: &str) -> Option<usize> {
        match self.get(k) { Some(J::Num(n)) => Some(*n as usize), _ => None }
    }
    pub fn s(&self, k: &str) -> Option<&str> {
        match self.get(k) { Some(J::Str(s)) => Some(s), _ => None }
    }
    pub fn f(&self, k: &str) -> Option<f64> {
        match self.get(k) { Some(J::Num(n)) => Some(*n), _ => None }
    }
}

pub fn parse(s: &[u8]) -> Option<J> {
    let mut i = 0usize;
    let v = val(s, &mut i)?;
    Some(v)
}

fn ws(s: &[u8], i: &mut usize) {
    while *i < s.len() && matches!(s[*i], b' ' | b'\t' | b'\n' | b'\r') { *i += 1; }
}

fn val(s: &[u8], i: &mut usize) -> Option<J> {
    ws(s, i);
    if *i >= s.len() { return None; }
    match s[*i] {
        b'{' => {
            *i += 1;
            let mut m = HashMap::new();
            loop {
                ws(s, i);
                if *i < s.len() && s[*i] == b'}' { *i += 1; break; }
                let k = match val(s, i)? { J::Str(k) => k, _ => return None };
                ws(s, i);
                if *i >= s.len() || s[*i] != b':' { return None; }
                *i += 1;
                let v = val(s, i)?;
                m.insert(k, v);
                ws(s, i);
                if *i < s.len() && s[*i] == b',' { *i += 1; }
            }
            Some(J::Obj(m))
        }
        b'[' => {
            *i += 1;
            let mut a = Vec::new();
            loop {
                ws(s, i);
                if *i < s.len() && s[*i] == b']' { *i += 1; break; }
                a.push(val(s, i)?);
                ws(s, i);
                if *i < s.len() && s[*i] == b',' { *i += 1; }
            }
            Some(J::Arr(a))
        }
        b'"' => {
            *i += 1;
            let mut out = String::new();
            while *i < s.len() {
                match s[*i] {
                    b'"' => { *i += 1; return Some(J::Str(out)); }
                    b'\\' => {
                        *i += 1;
                        if *i >= s.len() { return None; }
                        match s[*i] {
                            b'n' => out.push('\n'), b't' => out.push('\t'),
                            b'r' => out.push('\r'), b'b' => out.push('\u{8}'),
                            b'f' => out.push('\u{c}'),
                            b'u' => {
                                let h = std::str::from_utf8(s.get(*i+1..*i+5)?).ok()?;
                                let c = u32::from_str_radix(h, 16).ok()?;
                                out.push(char::from_u32(c).unwrap_or('?'));
                                *i += 4;
                            }
                            c => out.push(c as char),
                        }
                        *i += 1;
                    }
                    c => {
                        // collect a UTF-8 run rather than casting bytes to chars
                        let start = *i;
                        let mut j = *i;
                        while j < s.len() && s[j] != b'"' && s[j] != b'\\' { j += 1; }
                        out.push_str(&String::from_utf8_lossy(&s[start..j]));
                        *i = j;
                        let _ = c;
                    }
                }
            }
            None
        }
        b't' => { *i += 4; Some(J::Bool(true)) }
        b'f' => { *i += 5; Some(J::Bool(false)) }
        b'n' => { *i += 4; Some(J::Null) }
        _ => {
            let start = *i;
            while *i < s.len() && matches!(s[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            std::str::from_utf8(&s[start..*i]).ok()?.parse().ok().map(J::Num)
        }
    }
}

/// Serialise back out. Needed because the raid generator rewrites maps.json in
/// place, and clobbering a hand-edited content file with a lossy round-trip would
/// be worse than not writing it at all -- so parse and emit have to agree.
pub fn write(v: &J, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let pad2 = " ".repeat(indent + 1);
    match v {
        J::Null => "null".into(),
        J::Bool(b) => b.to_string(),
        J::Num(n) => {
            if (n.fract()).abs() < 1e-9 && n.abs() < 1e15 { format!("{}", *n as i64) }
            else { format!("{}", n) }
        }
        J::Str(s) => format!("\"{}\"", esc(s)),
        J::Arr(a) => {
            if a.is_empty() { return "[]".into(); }
            let inner: Vec<String> = a.iter().map(|x| format!("{}{}", pad2, write(x, indent + 1))).collect();
            format!("[\n{}\n{}]", inner.join(",\n"), pad)
        }
        J::Obj(m) => {
            if m.is_empty() { return "{}".into(); }
            // stable order: json objects are unordered but a diffable file is not
            let mut ks: Vec<&String> = m.keys().collect();
            ks.sort();
            let inner: Vec<String> = ks.iter()
                .map(|k| format!("{}\"{}\": {}", pad2, esc(k), write(&m[*k], indent + 1)))
                .collect();
            format!("{{\n{}\n{}}}", inner.join(",\n"), pad)
        }
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Standard base64, for glTF data: URIs.
pub fn b64(s: &str) -> Vec<u8> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (i, &c) in T.iter().enumerate() { rev[c as usize] = i as u8; }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        let v = rev[c as usize];
        if v == 255 { continue; }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
