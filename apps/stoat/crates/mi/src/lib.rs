//! GDB/MI wire format. Pure: bytes in, records out; commands in, bytes out.
//!
//! The parser is deliberately tolerant. gdb's MI output is not as regular as the
//! grammar in the manual suggests:
//!   * inferior stdout lands in the same stream when no separate tty is set,
//!   * MI2 emits bare tuples in result position for multi-location breakpoints
//!     (`bkpt={...},{...}`), fixed only in MI3,
//!   * `ui_file::printchar` octal-escapes bytes 0x80..0x9F but passes >= 0xA0 raw,
//!     so one UTF-8 character can arrive half-escaped (an em dash is
//!     `\xE2` raw + `\200\224`). Unescaping has to go through bytes, not chars.
//! Anything that does not parse becomes `Record::Unparsed` rather than an error;
//! a debugger frontend that dies on an odd line is worse than one that shows it.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Const(String),
    Tuple(Vec<(String, Value)>),
    /// List items keep their name when gdb gives one (`stack=[frame={..},frame={..}]`);
    /// the name is empty for plain value lists.
    List(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Const(s) => Some(s),
            _ => None,
        }
    }

    /// First entry with this key, for tuples and named lists.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Tuple(v) | Value::List(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            Value::Const(_) => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// Parses decimal or `0x` hex, which is how gdb prints addresses.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        parse_u64(self.get_str(key)?)
    }

    /// Child values regardless of naming. Empty for `Const`.
    pub fn items(&self) -> impl Iterator<Item = &Value> {
        let slice: &[(String, Value)] = match self {
            Value::Tuple(v) | Value::List(v) => v,
            Value::Const(_) => &[],
        };
        slice.iter().map(|(_, v)| v)
    }
}

pub fn parse_u64(s: &str) -> Option<u64> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncKind {
    /// `*` — target state changes (running, stopped).
    Exec,
    /// `+` — progress of slow operations.
    Status,
    /// `=` — everything else (threads, libraries, breakpoints changed).
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// `~` — what the CLI would have printed.
    Console,
    /// `@` — inferior output, only for some remote targets.
    Target,
    /// `&` — gdb's own log, including echo of CLI commands and error text.
    Log,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Result { token: Option<u64>, class: String, results: Value },
    Async { kind: AsyncKind, token: Option<u64>, class: String, results: Value },
    Stream { kind: StreamKind, text: String },
    /// The `(gdb)` terminator. Carries no information beyond "gdb is idle on the
    /// MI channel", and with mi-async on it does not even mean the target stopped.
    Prompt,
    /// Not MI. Usually inferior output sharing gdb's stdout.
    Unparsed(String),
}

/// Parse one line as read from gdb's stdout. Takes bytes, not `&str`: because of
/// the half-escaping described above, a line is not valid UTF-8 until after
/// c-string unescaping, and a lossy conversion up front would destroy characters.
pub fn parse_line(line: &[u8]) -> Record {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    let line = &line[..end];
    if line.trim_ascii_end() == b"(gdb)" {
        return Record::Prompt;
    }
    let mut p = Parser { s: line, i: 0 };
    p.record().unwrap_or_else(|| Record::Unparsed(String::from_utf8_lossy(line).into_owned()))
}

pub fn parse_str(line: &str) -> Record {
    parse_line(line.as_bytes())
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn record(&mut self) -> Option<Record> {
        let start = self.i;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.i += 1;
        }
        let token = std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok();
        let sigil = self.peek()?;
        self.i += 1;
        let stream = |k| Some(k);
        let stream_kind = match sigil {
            b'~' => stream(StreamKind::Console),
            b'@' => stream(StreamKind::Target),
            b'&' => stream(StreamKind::Log),
            _ => None,
        };
        if let Some(kind) = stream_kind {
            let text = self.cstring()?;
            return (self.i == self.s.len()).then_some(Record::Stream { kind, text });
        }
        let kind = match sigil {
            b'^' => None,
            b'*' => Some(AsyncKind::Exec),
            b'+' => Some(AsyncKind::Status),
            b'=' => Some(AsyncKind::Notify),
            _ => return None,
        };
        let class = self.ident()?;
        let mut results = Vec::new();
        while self.eat(b',') {
            results.push(self.result()?);
        }
        if self.i != self.s.len() {
            return None;
        }
        let results = Value::Tuple(results);
        Some(match kind {
            None => Record::Result { token, class, results },
            Some(kind) => Record::Async { kind, token, class, results },
        })
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self.peek().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    /// `name=value`, or a bare value (MI2 multi-location breakpoints) with an empty name.
    fn result(&mut self) -> Option<(String, Value)> {
        if matches!(self.peek()?, b'{' | b'[' | b'"') {
            return Some((String::new(), self.value()?));
        }
        let name = self.ident()?;
        if !self.eat(b'=') {
            return None;
        }
        Some((name, self.value()?))
    }

    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            b'"' => self.cstring().map(Value::Const),
            b'{' => self.seq(b'}').map(Value::Tuple),
            b'[' => self.seq(b']').map(Value::List),
            _ => None,
        }
    }

    fn seq(&mut self, close: u8) -> Option<Vec<(String, Value)>> {
        self.i += 1;
        let mut out = Vec::new();
        if self.eat(close) {
            return Some(out);
        }
        loop {
            out.push(self.result()?);
            if self.eat(close) {
                return Some(out);
            }
            if !self.eat(b',') {
                return None;
            }
        }
    }

    fn cstring(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out = Vec::new();
        loop {
            let b = self.peek()?;
            self.i += 1;
            match b {
                b'"' => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'e' => out.push(0x1b),
                        b'a' => out.push(0x07),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'v' => out.push(0x0b),
                        b'0'..=b'7' => {
                            let mut n = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        n = n * 8 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(n as u8);
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }
}

/// An MI command without its token. The token belongs to whoever correlates
/// replies, which is the transport, not the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    op: String,
    parts: Vec<String>,
}

impl Command {
    /// `op` without the leading dash: `Command::new("stack-list-frames")`.
    pub fn new(op: &str) -> Self {
        Command { op: op.trim_start_matches('-').to_string(), parts: Vec::new() }
    }

    /// An option or keyword, written verbatim (`--thread`, `-f`, `--simple-values`).
    pub fn opt(mut self, o: &str) -> Self {
        self.parts.push(o.to_string());
        self
    }

    /// A parameter, quoted only if it needs to be.
    pub fn arg(mut self, a: impl fmt::Display) -> Self {
        let a = a.to_string();
        let plain = !a.is_empty()
            && !a.starts_with('-')
            && a.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\');
        self.parts.push(if plain { a } else { quote(&a) });
        self
    }

    /// A parameter that is always quoted; for whole CLI command lines.
    pub fn quoted(mut self, a: &str) -> Self {
        self.parts.push(quote(a));
        self
    }

    pub fn op(&self) -> &str {
        &self.op
    }

    pub fn encode(&self, token: u64) -> String {
        let mut s = format!("{token}-{}", self.op);
        for p in &self.parts {
            s.push(' ');
            s.push_str(p);
        }
        s.push('\n');
        s
    }
}

pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
