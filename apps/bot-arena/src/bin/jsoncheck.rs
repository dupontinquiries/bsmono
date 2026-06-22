//! jsoncheck — validate JSON and (by default) pretty-print it. Pure Rust, no deps.
//!
//!   jsoncheck <file>        # validate + pretty-print to stdout
//!   jsoncheck --check <file> # validate only (exit non-zero if invalid)

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check_only = args.iter().any(|a| a == "--check");
    let file = args.iter().find(|a| !a.starts_with('-'));
    let Some(file) = file else { eprintln!("usage: jsoncheck [--check] <file>"); std::process::exit(2); };
    let src = match std::fs::read_to_string(Path::new(file)) {
        Ok(s) => s, Err(e) => { eprintln!("cannot read {file}: {e}"); std::process::exit(1); }
    };
    match parse(&src) {
        Ok(v) => { if check_only { println!("valid"); } else { println!("{}", pretty(&v, 0)); } }
        Err(e) => { eprintln!("invalid JSON: {e}"); std::process::exit(1); }
    }
}

#[derive(Debug, PartialEq)]
enum J { Null, Bool(bool), Num(f64), Str(String), Arr(Vec<J>), Obj(Vec<(String, J)>) }

struct P { c: Vec<char>, i: usize }

fn parse(s: &str) -> Result<J, String> {
    let mut p = P { c: s.chars().collect(), i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.c.len() { return Err(format!("trailing data at char {}", p.i)); }
    Ok(v)
}

impl P {
    fn ws(&mut self) { while self.i < self.c.len() && self.c[self.i].is_whitespace() { self.i += 1; } }
    fn peek(&self) -> Option<char> { self.c.get(self.i).copied() }
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
            if self.peek() != Some(ch) { return Err(format!("invalid literal at char {}", self.i)); }
            self.i += 1;
        }
        Ok(v)
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            self.i += 1;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let e = self.peek().ok_or("bad escape")?; self.i += 1;
                    out.push(match e { 'n' => '\n', 't' => '\t', 'r' => '\r', '"' => '"', '\\' => '\\', '/' => '/', _ => e });
                }
                _ => out.push(ch),
            }
        }
        Err("unterminated string".into())
    }
    fn number(&mut self) -> Result<J, String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || "+-.eE".contains(c) { self.i += 1; } else { break; }
        }
        let s: String = self.c[start..self.i].iter().collect();
        s.parse::<f64>().map(J::Num).map_err(|_| format!("bad number '{s}'"))
    }
    fn array(&mut self) -> Result<J, String> {
        self.i += 1; let mut v = Vec::new(); self.ws();
        if self.peek() == Some(']') { self.i += 1; return Ok(J::Arr(v)); }
        loop {
            v.push(self.value()?); self.ws();
            match self.peek() {
                Some(',') => { self.i += 1; }
                Some(']') => { self.i += 1; return Ok(J::Arr(v)); }
                _ => return Err(format!("expected ',' or ']' at char {}", self.i)),
            }
        }
    }
    fn object(&mut self) -> Result<J, String> {
        self.i += 1; let mut v = Vec::new(); self.ws();
        if self.peek() == Some('}') { self.i += 1; return Ok(J::Obj(v)); }
        loop {
            self.ws();
            if self.peek() != Some('"') { return Err(format!("expected key string at char {}", self.i)); }
            let k = self.string()?; self.ws();
            if self.peek() != Some(':') { return Err(format!("expected ':' at char {}", self.i)); }
            self.i += 1;
            v.push((k, self.value()?)); self.ws();
            match self.peek() {
                Some(',') => { self.i += 1; }
                Some('}') => { self.i += 1; return Ok(J::Obj(v)); }
                _ => return Err(format!("expected ',' or '}}' at char {}", self.i)),
            }
        }
    }
}

fn pretty(v: &J, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    let pad1 = "  ".repeat(depth + 1);
    match v {
        J::Null => "null".into(),
        J::Bool(b) => b.to_string(),
        J::Num(n) => if n.fract() == 0.0 { format!("{}", *n as i64) } else { n.to_string() },
        J::Str(s) => format!("{s:?}"),
        J::Arr(a) if a.is_empty() => "[]".into(),
        J::Arr(a) => {
            let items: Vec<String> = a.iter().map(|x| format!("{pad1}{}", pretty(x, depth + 1))).collect();
            format!("[\n{}\n{pad}]", items.join(",\n"))
        }
        J::Obj(o) if o.is_empty() => "{}".into(),
        J::Obj(o) => {
            let items: Vec<String> = o.iter().map(|(k, x)| format!("{pad1}{k:?}: {}", pretty(x, depth + 1))).collect();
            format!("{{\n{}\n{pad}}}", items.join(",\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, J};
    #[test]
    fn valid_and_invalid() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"]}"#).unwrap();
        assert!(matches!(v, J::Obj(_)));
        assert!(parse(r#"{"a":1,}"#).is_err());   // trailing comma
        assert!(parse(r#"[1 2]"#).is_err());        // missing comma
        assert!(parse(r#""unterminated"#).is_err());
        assert!(parse("  42  ").is_ok());
    }
}
