//! status — the one line from the boxes at home, read off the DHT. logscope's `status` module on the
//! Ultra or the mini publishes `{"at","source","level","text"}` under a channel that is a secret
//! string both ends know; this resolves it and keeps the newest line. Behind `status-dht` because it
//! pulls the presence node in; without the feature the feed exists and is always empty, so the shell
//! compiles the same either way.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Line { pub at: String, pub source: String, pub level: String, pub text: String }

impl Line {
    /// Tolerant hand parse of the four flat string fields, so this crate needs no serde.
    pub fn parse(json: &[u8]) -> Option<Line> {
        let s = std::str::from_utf8(json).ok()?;
        let field = |k: &str| -> Option<String> {
            let key = format!("\"{k}\":");
            let i = s.find(&key)? + key.len();
            let rest = s[i..].trim_start();
            let rest = rest.strip_prefix('"')?;
            let mut out = String::new(); let mut esc = false;
            for c in rest.chars() { if esc { out.push(c); esc = false; } else if c == '\\' { esc = true; } else if c == '"' { return Some(out); } else { out.push(c); } }
            None
        };
        Some(Line { at: field("at").unwrap_or_default(), source: field("source")?, level: field("level").unwrap_or_default(), text: field("text")? })
    }
    pub fn render(&self) -> String {
        let tag = match self.level.as_str() { "fact" => "·", "inference" => "~", _ => "?" };
        format!("{} {} {}", self.source, tag, self.text)
    }
}

/// Newest line wins by its `at` (the publisher's `unix:<secs>` stamp), so a stale value that
/// arrives late cannot overwrite a fresh one.
fn newer(a: &Line, b: &Line) -> bool {
    let n = |l: &Line| l.at.strip_prefix("unix:").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    n(a) > n(b)
}

pub struct Feed {
    latest: Option<Line>,
    seen_at: Option<Instant>,
    #[cfg(feature = "status-dht")]
    host: Option<snapclone_presence::node_host::NodeHost>,
    channel: String,
    last_ask: Instant,
    pub interval: Duration,
}

impl Feed {
    /// `channel` empty = disabled. The node binds an ephemeral UDP port and bootstraps like snapclone's.
    pub fn new(channel: &str) -> Feed {
        Feed { latest: None, seen_at: None, #[cfg(feature = "status-dht")] host: None, channel: channel.to_string(), last_ask: Instant::now() - Duration::from_secs(60), interval: Duration::from_secs(20) }
    }

    pub fn enabled(&self) -> bool { !self.channel.is_empty() }
    pub fn latest(&self) -> Option<&Line> { self.latest.as_ref() }
    /// How long since the newest line arrived; the shell dims a stale one.
    pub fn age(&self) -> Option<Duration> { self.seen_at.map(|t| t.elapsed()) }

    /// Called once per frame (or per timer tick) by the shell: asks every `interval`, drains answers.
    pub fn tick(&mut self) {
        if !self.enabled() { return; }
        #[cfg(feature = "status-dht")]
        {
            if self.host.is_none() {
                let id = snapclone_presence::presence::key_for_scid(&format!("hatch-{}", std::process::id()));
                self.host = snapclone_presence::node_host::NodeHost::spawn(snapcore::routing::NodeId(id.0), "0.0.0.0:0".parse().unwrap()).ok();
            }
            let Some(h) = &self.host else { return };
            if self.last_ask.elapsed() >= self.interval { h.resolve_raw(&self.channel); self.last_ask = Instant::now(); }
            for ev in h.poll() {
                if let snapclone_presence::node_host::Event::ResolvedRaw { values, .. } = ev {
                    for v in values {
                        if let Some(l) = Line::parse(&v) {
                            if self.latest.as_ref().map(|cur| newer(&l, cur)).unwrap_or(true) { self.latest = Some(l); self.seen_at = Some(Instant::now()); }
                        }
                    }
                }
            }
        }
    }

    /// For the shell's own tests and for the probe: inject a line as if it had been resolved.
    pub fn inject(&mut self, l: Line) { if self.latest.as_ref().map(|cur| newer(&l, cur)).unwrap_or(true) { self.latest = Some(l); self.seen_at = Some(Instant::now()); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_the_wire_form_and_keeps_the_newest() {
        let a = Line::parse(br#"{"at":"unix:100","source":"mini","level":"fact","text":"loaded Ornith (mtp) 71 tok/s"}"#).unwrap();
        assert_eq!(a.render(), "mini · loaded Ornith (mtp) 71 tok/s");
        let b = Line::parse(br#"{"source":"ultra","level":"inference","text":"claude: review \"ok\"","at":"unix:99"}"#).unwrap();
        assert_eq!(b.text, "claude: review \"ok\"");
        let mut f = Feed::new("chan");
        f.inject(a.clone()); f.inject(b);
        assert_eq!(f.latest().unwrap().source, "mini", "the older line must not replace the newer one");
        assert!(Line::parse(b"not json").is_none());
    }
}
