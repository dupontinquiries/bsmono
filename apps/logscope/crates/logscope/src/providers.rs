//! providers.rs — shared Ollama provider registry
//!
//! Manages a persistent list of known Ollama hosts discovered on any network,
//! stored in ~/.local/share/logscope/providers.db (SQLite).
//!
//! Schema
//! ──────
//!   hosts (id, url, label, network_prefix, last_seen_ms, last_ok_ms, fail_count)
//!
//! "network_prefix" is the first 3 octets of the machine's local IP at discovery
//! time (e.g. "192.168.1") so we can quickly filter to hosts reachable on the
//! current network without probing unreachable subnets.
//!
//! Public surface used by both chat_tab and peek_view:
//!   ProviderRegistry::open() → Result<ProviderRegistry>
//!   registry.known_hosts() → Vec<KnownHost>
//!   registry.record_host(url, label, network_prefix)
//!   registry.mark_ok(url)
//!   registry.mark_fail(url)
//!   registry.hosts_for_network(prefix) → Vec<KnownHost>
//!
//! The shared model list (ModelInfo + blending) also lives here so both tabs
//! use identical logic.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json;

use crate::model_selector::ModelInfo;

// ── KnownHost ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownHost {
    pub id:             i64,
    pub url:            String,   // "http://192.168.1.5:11434"
    pub label:          String,   // user-visible name, e.g. "MacBook-Pro"
    pub network_prefix: String,   // "192.168.1"
    pub last_seen_ms:   i64,
    pub last_ok_ms:     i64,
    pub fail_count:     i64,
}

impl KnownHost {
    /// Display name: label if set, otherwise hostname extracted from URL.
    #[allow(dead_code)]
    pub fn display_name(&self) -> String {
        if !self.label.is_empty() { return self.label.clone(); }
        self.url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .split(':').next()
            .unwrap_or(&self.url)
            .to_string()
    }

    /// True if this host has been reachable recently (< 24 h since last_ok).
    pub fn is_recent(&self) -> bool {
        let now = now_ms();
        now - self.last_ok_ms < 24 * 3600 * 1000
    }
}

// ── ProviderRegistry ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ProviderRegistry {
    conn: Arc<Mutex<Connection>>,
}

impl ProviderRegistry {
    pub fn open() -> Result<Self, RegistryError> {
        let path = db_path();
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let conn = Connection::open(&path)?;
        let reg  = Self { conn: Arc::new(Mutex::new(conn)) };
        reg.init_schema()?;
        Ok(reg)
    }

    fn init_schema(&self) -> Result<(), RegistryError> {
        self.conn.lock().unwrap().execute_batch("
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS hosts (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                url            TEXT NOT NULL UNIQUE,
                label          TEXT NOT NULL DEFAULT '',
                network_prefix TEXT NOT NULL DEFAULT '',
                last_seen_ms   INTEGER NOT NULL DEFAULT 0,
                last_ok_ms     INTEGER NOT NULL DEFAULT 0,
                fail_count     INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_hosts_network ON hosts(network_prefix);
        ")?;
        Ok(())
    }

    /// Insert or update a host entry.  Sets last_seen and resets fail_count.
    pub fn record_host(&self, url: &str, label: &str, network_prefix: &str)
        -> Result<(), RegistryError>
    {
        let now = now_ms();
        self.conn.lock().unwrap().execute(
            "INSERT INTO hosts (url, label, network_prefix, last_seen_ms, last_ok_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(url) DO UPDATE SET
               label          = CASE WHEN excluded.label != '' THEN excluded.label ELSE label END,
               network_prefix = excluded.network_prefix,
               last_seen_ms   = excluded.last_seen_ms,
               fail_count     = 0",
            params![url, label, network_prefix, now],
        )?;
        Ok(())
    }

    pub fn mark_ok(&self, url: &str) -> Result<(), RegistryError> {
        let now = now_ms();
        self.conn.lock().unwrap().execute(
            "UPDATE hosts SET last_ok_ms=?1, fail_count=0 WHERE url=?2",
            params![now, url],
        )?;
        Ok(())
    }

    pub fn mark_fail(&self, url: &str) -> Result<(), RegistryError> {
        self.conn.lock().unwrap().execute(
            "UPDATE hosts SET fail_count=fail_count+1 WHERE url=?1",
            params![url],
        )?;
        Ok(())
    }

    /// All known hosts, newest first.
    pub fn known_hosts(&self) -> Vec<KnownHost> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id,url,label,network_prefix,last_seen_ms,last_ok_ms,fail_count
             FROM hosts ORDER BY last_ok_ms DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([], |row| Ok(KnownHost {
            id:             row.get(0)?,
            url:            row.get(1)?,
            label:          row.get(2)?,
            network_prefix: row.get(3)?,
            last_seen_ms:   row.get(4)?,
            last_ok_ms:     row.get(5)?,
            fail_count:     row.get(6)?,
        })).ok()
          .map(|rows| rows.flatten().collect())
          .unwrap_or_default()
    }

    /// Hosts that share the given network prefix (current LAN).
    #[allow(dead_code)]
    pub fn hosts_for_network(&self, prefix: &str) -> Vec<KnownHost> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id,url,label,network_prefix,last_seen_ms,last_ok_ms,fail_count
             FROM hosts WHERE network_prefix=?1 ORDER BY last_ok_ms DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([prefix], |row| Ok(KnownHost {
            id:             row.get(0)?,
            url:            row.get(1)?,
            label:          row.get(2)?,
            network_prefix: row.get(3)?,
            last_seen_ms:   row.get(4)?,
            last_ok_ms:     row.get(5)?,
            fail_count:     row.get(6)?,
        })).ok()
          .map(|rows| rows.flatten().collect())
          .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn delete_host(&self, url: &str) -> Result<(), RegistryError> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM hosts WHERE url=?1", params![url])?;
        Ok(())
    }
}

// ── shared model building ─────────────────────────────────────────────────────
//
// Both peek_view and chat_tab call build_provider_model_list() to get a unified
// Vec<ModelInfo> from local + all known hosts.

/// Query one host and return its models.  On success records the host in the
/// registry; on failure marks a fail.
pub fn probe_host(
    registry: &ProviderRegistry,
    url:      &str,
) -> Vec<ModelInfo> {
    match fetch_tags(url) {
        Ok(models) => {
            let prefix = network_prefix_of(url);
            let label  = hostname_of(url);
            let _ = registry.record_host(url, &label, &prefix);
            let _ = registry.mark_ok(url);
            let is_local = is_localhost(url);
            models.into_iter().map(|(name, size, size_vram, quant)| {
                let host = if is_local { String::new() } else { url.to_owned() };
                ModelInfo::from_ollama_entry(&name, size, size_vram, &quant, &host)
            }).collect()
        }
        Err(_) => {
            let _ = registry.mark_fail(url);
            vec![]
        }
    }
}

/// Inline /api/tags fetch — does not depend on crate::ui so providers.rs
/// can be used from both the binary and the library crate root.
fn fetch_tags(host: &str) -> Result<Vec<(String, u64, u64, String)>, String> {
    let url = format!("{host}/api/tags");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let arr = body["models"].as_array()
        .ok_or_else(|| "no models array".to_string())?;
    Ok(arr.iter().filter_map(|m| {
        let name  = m["name"].as_str()?.to_owned();
        let size  = m["size"].as_u64().unwrap_or(0);
        let vram  = m["size_vram"].as_u64().unwrap_or(0);
        let quant = m["details"]["quantization_level"].as_str().unwrap_or("").to_owned();
        Some((name, size, vram, quant))
    }).collect())
}

/// Return the current machine's local-facing subnet prefix, e.g. "192.168.1".
pub fn local_network_prefix() -> String {
    if let Ok(fib) = std::fs::read_to_string("/proc/net/fib_trie") {
        let mut last_ip: Option<std::net::Ipv4Addr> = None;
        for line in fib.lines() {
            let t = line.trim();
            if t.contains("LOCAL") {
                if let Some(ip) = last_ip.take() {
                    if !ip.is_loopback() && is_private_v4(ip) {
                        let o = ip.octets();
                        return format!("{}.{}.{}", o[0], o[1], o[2]);
                    }
                }
            }
            if let Ok(ip) = t.split_whitespace().next()
                .unwrap_or("").parse::<std::net::Ipv4Addr>()
            {
                if !ip.is_loopback() && is_private_v4(ip) { last_ip = Some(ip); }
            }
        }
    }
    String::new()
}

fn network_prefix_of(url: &str) -> String {
    let host = url.trim_start_matches("http://").trim_start_matches("https://")
        .split(':').next().unwrap_or("");
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        return format!("{}.{}.{}", o[0], o[1], o[2]);
    }
    String::new()
}

fn hostname_of(url: &str) -> String {
    url.trim_start_matches("http://").trim_start_matches("https://")
        .split(':').next().unwrap_or("").to_string()
}

fn is_localhost(url: &str) -> bool {
    let h = url.trim_start_matches("http://").trim_start_matches("https://")
        .split(':').next().unwrap_or("");
    matches!(h, "localhost" | "127.0.0.1" | "::1")
}

fn is_private_v4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0]==10 || (o[0]==172&&(16..=31).contains(&o[1])) || (o[0]==192&&o[1]==168)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logscope")
        .join("providers.db")
}


// ── LAN discovery ─────────────────────────────────────────────────────────────

/// Scan the local /24 subnet(s) for Ollama instances in parallel.
/// Sends `(host_url, Vec<ModelInfo>)` batches as each host responds.
/// Results arrive within ~1-3 seconds regardless of subnet size.
pub fn scan_lan_parallel(
    tx: &crossbeam_channel::Sender<(String, Vec<ModelInfo>)>,
) {
    use std::net::{Ipv4Addr, TcpStream};
    use std::time::Duration;

    let subnets = local_subnets_v4();
    if subnets.is_empty() { return; }

    let addrs: Vec<u32> = subnets.into_iter().flat_map(|base| {
        let net = u32::from(base) & 0xFFFFFF00;
        (1u32..=254).map(move |i| net | i)
    }).collect();

    // Use a dedicated thread pool for the TCP probes — NOT rayon's global pool.
    // Rayon's pool is for CPU-bound work; blocking 254 threads on 120ms TCP
    // connect_timeout would starve it. A separate 32-thread pool keeps the
    // probes parallel without touching the rest of the app.
    let found: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(32)
        .build()
        .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap());

    pool.install(|| {
        use rayon::prelude::*;
        addrs.par_iter().for_each(|&ip_u32| {
            let ip = Ipv4Addr::from(ip_u32);
            if let Ok(addr) = format!("{ip}:11434").parse() {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok() {
                    found.lock().unwrap().push(format!("http://{ip}:11434"));
                }
            }
        });
    });

    // Fetch models and record to DB sequentially (single SQLite connection)
    if let Ok(reg) = ProviderRegistry::open() {
        let prefix = local_network_prefix();
        for url in found.into_inner().unwrap() {
            let _ = reg.record_host(&url, "", &prefix);
            let batch = probe_host(&reg, &url);
            if !batch.is_empty() { let _ = tx.send((url, batch)); }
        }
    }
}

fn local_subnets_v4() -> Vec<std::net::Ipv4Addr> {
    let mut out = Vec::new();
    if let Ok(fib) = std::fs::read_to_string("/proc/net/fib_trie") {
        let mut last_ip: Option<std::net::Ipv4Addr> = None;
        for line in fib.lines() {
            let t = line.trim();
            if t.contains("LOCAL") {
                if let Some(ip) = last_ip.take() {
                    if !ip.is_loopback() && is_private_v4(ip) { out.push(ip); }
                }
            }
            if let Ok(ip) = t.split_whitespace().next()
                .unwrap_or("").parse::<std::net::Ipv4Addr>() {
                if !ip.is_loopback() && is_private_v4(ip) { last_ip = Some(ip); }
            }
        }
    }
    if out.is_empty() {
        // Fallback: try common /24 subnets
        for s in ["192.168.1.1","192.168.0.1","192.168.2.1","10.0.0.1","10.0.1.1"] {
            if let Ok(ip) = s.parse() { out.push(ip); }
        }
    }
    out
}

// ── errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum RegistryError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}
impl From<rusqlite::Error> for RegistryError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<std::io::Error>  for RegistryError { fn from(e: std::io::Error)  -> Self { Self::Io(e) } }
impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Sqlite(e) => write!(f, "{e}"), Self::Io(e) => write!(f, "{e}") }
    }
}

// ── ModelEntry — unified model list entry ─────────────────────────────────────
//
// A ModelEntry represents one row in the model dropdown.  It captures all the
// information needed to both display a rich two-line entry and to dispatch the
// actual inference request to the right host.
//
// Three kinds of entry:
//   Auto   — model available on ≥2 hosts; picks best at dispatch time
//   Local  — model available only on localhost
//   Remote — model available only on a specific LAN host

#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// The bare model name as Ollama knows it, e.g. "llama3.2:1b"
    pub model:    String,
    /// True iff this is the @auto virtual row (multi-host)
    pub is_auto:  bool,
    /// Pinned host for non-auto entries (empty = localhost)
    pub pin_host: String,
    /// Pre-computed subtitle for the popup list (e.g. "local, goliath.local, 2 others")
    pub subtitle: String,
    /// Kind badge shown in the popup: "auto", "local", or "LAN"
    pub kind:     String,
}

impl ModelEntry {
    /// The key stored in the StringList.
    /// Auto:    "llama3.2:1b@auto"
    /// Local:   "llama3.2:1b"
    /// Remote:  "llama3.2:1b@http://192.168.2.215:11434"
    pub fn store_key(&self) -> String {
        if self.is_auto {
            format!("{}@auto", self.model)
        } else if self.pin_host.is_empty() {
            self.model.clone()
        } else {
            format!("{}@{}", self.model, self.pin_host)
        }
    }

    /// (display_name, subtitle, kind_badge) for the popup row.
    #[allow(dead_code)]
    pub fn display(&self) -> (&str, &str, &str) {
        (&self.model, &self.subtitle, &self.kind)
    }
}

/// Build a merged, sorted list of ModelEntries from all probed hosts.
///
/// Sort order:
///   1. Currently loaded in VRAM (size_vram > 0 on any host)
///   2. Recency list position (recently used models first)
///   3. Parameter count descending (largest first)
///
/// For each unique base model name:
///   - If it appears on ≥2 hosts: emit one @auto row then one row per host
///   - If it appears on exactly 1 host: emit only that one row
pub fn build_model_entries(
    host_batches: &[(String, Vec<ModelInfo>)],  // (host_url, models)
    recency:      &[String],
) -> Vec<ModelEntry> {
    use std::collections::HashMap;

    // Group ModelInfo by base model name
    let mut by_name: HashMap<String, Vec<(String, ModelInfo)>> = HashMap::new();
    for (host_url, models) in host_batches {
        for m in models {
            by_name.entry(m.name.clone())
                .or_default()
                .push((host_url.clone(), m.clone()));
        }
    }

    // Score for sorting: (loaded_tier, recency_pos, params_desc)
    let score = |name: &str, variants: &[(String, ModelInfo)]| {
        let loaded = variants.iter().any(|(_, m)| m.size_vram > 0);
        let rec    = recency.iter().position(|r| r == name).unwrap_or(usize::MAX);
        let params = variants.iter().map(|(_, m)| m.params_m).max().unwrap_or(0);
        (if loaded { 0u8 } else { 1u8 }, rec, u64::MAX - params)
    };

    let mut names: Vec<String> = by_name.keys().cloned().collect();
    names.sort_by_key(|n| score(n, &by_name[n]));

    let mut entries: Vec<ModelEntry> = Vec::new();

    for name in names {
        let variants = by_name.remove(&name).unwrap();
        let multi = variants.len() > 1;

        // Build per-host subtitle fragments
        let host_labels: Vec<String> = variants.iter().map(|(url, _)| {
            if url.is_empty() || url == "http://localhost:11434" || url == "http://127.0.0.1:11434" {
                "local".to_string()
            } else {
                url.trim_start_matches("http://")
                   .trim_start_matches("https://")
                   .split(':').next()
                   .unwrap_or(url)
                   .to_string()
            }
        }).collect();

        let subtitle_for_auto = match host_labels.len() {
            0 => String::new(),
            1 => host_labels[0].clone(),
            2 => format!("{}, {}", host_labels[0], host_labels[1]),
            3 => format!("{}, {}, {}", host_labels[0], host_labels[1], host_labels[2]),
            n => format!("{}, {}, {} others", host_labels[0], host_labels[1], n - 2),
        };


        if multi {
            // @auto row
            entries.push(ModelEntry {
                model:    name.clone(),
                is_auto:  true,
                pin_host: String::new(),
                subtitle: subtitle_for_auto,
                kind:     "auto".to_string(),
            });
        }

        // Per-host pinned rows
        for (host_url, _info) in &variants {
            let is_local = host_url.is_empty()
                || host_url == "http://localhost:11434"
                || host_url == "http://127.0.0.1:11434";
            let subtitle = if is_local {
                String::new()
            } else {
                host_url.trim_start_matches("http://")
                    .trim_start_matches("https://")
                    .split(':').next()
                    .unwrap_or(host_url)
                    .to_string()
            };
            entries.push(ModelEntry {
                model:    name.clone(),
                is_auto:  false,
                pin_host: if is_local { String::new() } else { host_url.clone() },
                subtitle,
                kind:     if is_local { "local".to_string() } else { "LAN".to_string() },
            });
        }
    }

    entries
}

/// Probe all known hosts (local + registry) in order, sending each result
/// batch as it arrives so callers can update the UI incrementally.
pub fn probe_all_known(
    tx: &crossbeam_channel::Sender<(String, Vec<ModelInfo>)>,
) {
    const LOCAL: &str = "http://localhost:11434";
    if let Ok(reg) = ProviderRegistry::open() {
        // Local first (fastest, always available)
        let local_models = probe_host(&reg, LOCAL);
        if !local_models.is_empty() {
            let _ = tx.send((String::new(), local_models)); // empty = localhost
        }
        // Known LAN hosts
        for kh in reg.known_hosts() {
            if kh.url == LOCAL { continue; }
            if !kh.is_recent() {
                        continue;
            }
                let models = probe_host(&reg, &kh.url);
            if !models.is_empty() {
                let _ = tx.send((kh.url.clone(), models));
            }
        }
    } else {
    }
}
