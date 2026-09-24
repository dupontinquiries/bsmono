//! status — one short line about what a box is doing, with the liaison's honesty tag on it, sent
//! to wherever the person is looking: the ledger (always), an HTTP sink (the shim, a dashboard),
//! and the DHT (the phone, via hatch). Sinks are plugged in; the line is the contract.
use serde::{Deserialize, Serialize};

/// The liaison's three-way split, kept as a string on the wire so a phone without the enum can show it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Level { Fact, Inference, Unknown }

impl Level {
    pub fn tag(self) -> &'static str { match self { Level::Fact => "·", Level::Inference => "~", Level::Unknown => "?" } }
    pub fn parse(s: &str) -> Level { match s { "fact" => Level::Fact, "inference" => Level::Inference, _ => Level::Unknown } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusLine {
    pub at: String,
    /// Which box said it: "ultra", "mini", "claude".
    pub source: String,
    pub level: Level,
    pub text: String,
}

impl StatusLine {
    pub fn new(source: &str, level: Level, text: &str) -> StatusLine {
        StatusLine { at: crate::matrix::now(), source: source.into(), level, text: text.chars().take(240).collect() }
    }
    /// The one-line form the phone shows: `ultra · loaded Ornith-1.5-35B (Q4) 71 tok/s`.
    pub fn render(&self) -> String { format!("{} {} {}", self.source, self.level.tag(), self.text) }
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn from_bytes(b: &[u8]) -> Option<StatusLine> { serde_json::from_slice(b).ok() }
}

pub trait Sink: Send {
    fn name(&self) -> &str;
    fn publish(&self, line: &StatusLine) -> Result<(), String>;
}

/// Always-on sink: the ledger, so a status line is also a history.
pub struct LedgerSink;
impl Sink for LedgerSink {
    fn name(&self) -> &str { "ledger" }
    fn publish(&self, line: &StatusLine) -> Result<(), String> { crate::ledger::append(crate::ledger::Entry::Note { text: line.render() }) }
}

/// POST the line as JSON to `{base}/status` — the shim on the mini, or anything that listens.
pub struct HttpSink { pub base: String, pub token: Option<String> }
impl Sink for HttpSink {
    fn name(&self) -> &str { "http" }
    fn publish(&self, line: &StatusLine) -> Result<(), String> {
        let mut r = ureq::post(&format!("{}/status", self.base.trim_end_matches('/'))).timeout(std::time::Duration::from_secs(5));
        if let Some(t) = &self.token { r = r.set("Authorization", &format!("Bearer {t}")); }
        r.send_json(serde_json::to_value(line).map_err(|e| e.to_string())?).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Publish through every sink; the ledger is always first. Failures are collected, not fatal:
/// a phone that is off should not stop the ledger line.
pub fn broadcast(sinks: &[Box<dyn Sink>], line: &StatusLine) -> Vec<(String, String)> {
    let mut errs = Vec::new();
    if let Err(e) = LedgerSink.publish(line) { errs.push(("ledger".into(), e)); }
    for s in sinks { if let Err(e) = s.publish(line) { errs.push((s.name().to_string(), e)); } }
    errs
}

/// The DHT sink: the line goes under the status channel's sc-ID, where hatch on the phone looks.
/// The channel id is a secret string both ends know (`LOGSCOPE_STATUS_CHANNEL`); sha1(secret) is the
/// key, so anyone with the string can read the status and nobody else can find it.
#[cfg(feature = "dht")]
pub mod dht {
    use super::*;
    use snapclone_presence::node_host::{Event, NodeHost};
    use std::net::SocketAddr;

    pub struct DhtSink { host: NodeHost, channel: String }

    impl DhtSink {
        /// Bind a node on `bind` (0.0.0.0:0 for an ephemeral port) and bootstrap against `boot`.
        pub fn new(channel: &str, bind: SocketAddr, boot: Option<(SocketAddr, snapcore::routing::NodeId)>) -> std::io::Result<DhtSink> {
            let id = snapcore::routing::NodeId(snapclone_presence::presence::key_for_scid(&format!("logscope-{}", std::process::id())).0);
            let host = NodeHost::spawn(id, bind)?;
            if let Some((a, i)) = boot { host.bootstrap(a, i); }
            Ok(DhtSink { host, channel: channel.into() })
        }
        /// Ask for the latest lines; answered on `poll` as `ResolvedRaw`.
        pub fn resolve(&self) -> bool { self.host.resolve_raw(&self.channel) }
        pub fn poll(&self) -> Vec<StatusLine> {
            self.host.poll().into_iter().filter_map(|e| match e {
                Event::ResolvedRaw { values, .. } => Some(values.into_iter().filter_map(|v| StatusLine::from_bytes(&v)).collect::<Vec<_>>()),
                _ => None,
            }).flatten().collect()
        }
    }

    impl Sink for DhtSink {
        fn name(&self) -> &str { "dht" }
        fn publish(&self, line: &StatusLine) -> Result<(), String> {
            let vid = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1);
            if self.host.publish_raw(&self.channel, vid, line.to_bytes()) { Ok(()) } else { Err("dht node thread gone".into()) }
        }
    }
}
