//! config — everything the shim needs, from one JSON file (`~/.config/logscope/shim.json`) with env
//! overrides for the things you change per session. Written out on first run so the file exists to edit.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Extra llama-server arguments, e.g. ["--spec-type","draft-mtp","--spec-draft-n-max","3"]
    pub args: Vec<String>,
    #[serde(default = "default_ctx")] pub ctx: u32,
    #[serde(default = "default_np")] pub np: u32,
}
fn default_ctx() -> u32 { 65536 }
fn default_np() -> u32 { 2 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// The NVMe library: every GGUF you own, read-only from the shim's point of view.
    pub library_dir: PathBuf,
    /// Internal storage the server actually loads from; rotated under `budget_gb`.
    pub cache_dir: PathBuf,
    pub budget_gb: f64,
    pub llama_server: String,
    /// Where llama-server listens; 0.0.0.0 so the Ultra can use it as a box, too.
    pub server_host: String,
    pub server_port: u16,
    /// The shim's own HTTP port for directives and status.
    pub port: u16,
    /// Bearer token both ends know. Empty = no auth (LAN only, your call).
    pub token: String,
    /// The name this box signs its status lines with.
    pub name: String,
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
    /// Optional: forward status lines to another HTTP sink (a dashboard, the Ultra's shim).
    #[serde(default)]
    pub status_http: Option<String>,
    /// Optional: the DHT status channel secret; when set (and built with `dht`), lines go to the phone.
    #[serde(default)]
    pub status_channel: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert("plain".into(), Profile { args: vec![], ctx: 65536, np: 2 });
        profiles.insert("mtp".into(), Profile { args: ["--spec-type", "draft-mtp", "--spec-draft-n-max", "3"].iter().map(|s| s.to_string()).collect(), ctx: 65536, np: 2 });
        profiles.insert("ngram".into(), Profile { args: ["--spec-type", "ngram-simple", "--spec-draft-n-max", "16"].iter().map(|s| s.to_string()).collect(), ctx: 65536, np: 2 });
        Config {
            library_dir: PathBuf::from("/Volumes/Library/models"), cache_dir: home.join("models-cache"), budget_gb: 120.0,
            llama_server: "llama-server".into(), server_host: "0.0.0.0".into(), server_port: 8080, port: 8790,
            token: String::new(), name: "mini".into(), profiles, status_http: None, status_channel: None,
        }
    }
}

pub fn path() -> PathBuf {
    std::env::var("LOGSCOPE_SHIM_CONFIG").map(PathBuf::from).unwrap_or_else(|_| logscope_hive_config_dir().join("shim.json"))
}

fn logscope_hive_config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")).join("logscope")
}

pub fn load_or_create() -> Result<Config, String> {
    let p = path();
    let mut c = match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?,
        Err(_) => { let c = Config::default(); if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); } let _ = std::fs::write(&p, serde_json::to_string_pretty(&c).unwrap_or_default()); c }
    };
    if let Ok(v) = std::env::var("LOGSCOPE_SHIM_LIBRARY") { c.library_dir = PathBuf::from(v); }
    if let Ok(v) = std::env::var("LOGSCOPE_SHIM_CACHE") { c.cache_dir = PathBuf::from(v); }
    if let Ok(v) = std::env::var("LOGSCOPE_SHIM_TOKEN") { c.token = v; }
    if let Ok(v) = std::env::var("LOGSCOPE_SHIM_PORT") { if let Ok(n) = v.parse() { c.port = n; } }
    if let Ok(v) = std::env::var("LOGSCOPE_SHIM_LLAMA") { c.llama_server = v; }
    if let Ok(v) = std::env::var("LOGSCOPE_STATUS_CHANNEL") { c.status_channel = Some(v); }
    Ok(c)
}
