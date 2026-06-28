//! Network share: advertise this repo over HTTP so peers can find and fetch it,
//! look up a peer's advertisement, and install (clone) it.
//!
//! Uses the pieces gitmind already had: `gitmind_core::announce::ProjectCard` is
//! the content-addressed payload (name + refs + ssh provider), and
//! `net_kit::http` is the transport (`serve` on the advertising side, `roundtrip`
//! on the looking-up side). Card construction and the request/clone command
//! building are unit-tested; the live serve / GET / clone I/O runs on real
//! devices (no network in the build sandbox).

use gitmind_core::announce::ProjectCard;
use net_kit::http;
use std::io::Write;
use std::net::{TcpListener, UdpSocket};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The path a peer GETs to read the advertised card.
pub const CARD_PATH: &str = "/card";

// ───────────────────────── advertise (serve) ─────────────────────────

/// Shared state for the advertising HTTP server. `addr` empty = not serving.
/// `serve_loop` polls `should_stop` so the GUI can turn advertising off without
/// waiting for an incoming connection.
#[derive(Default)]
pub struct Advertiser {
    stop: AtomicBool,
    addr: Mutex<String>,
}

impl Advertiser {
    pub fn is_serving(&self) -> bool {
        !self.addr.lock().unwrap().is_empty()
    }
    pub fn addr(&self) -> String {
        self.addr.lock().unwrap().clone()
    }
    /// Mark serving at `addr` and clear any prior stop request.
    pub fn begin(&self, addr: String) {
        self.stop.store(false, Ordering::Relaxed);
        *self.addr.lock().unwrap() = addr;
    }
    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    /// Request the serve loop stop and mark not-serving.
    pub fn end(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.addr.lock().unwrap().clear();
    }
}

/// Serve `card_json` at GET /card on an already-bound `listener` until the
/// advertiser is asked to stop. Non-blocking accept + a short poll so the stop
/// flag is honored promptly.
pub fn serve_loop(listener: TcpListener, card_json: String, adv: Arc<Advertiser>) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    loop {
        if adv.should_stop() {
            return Ok(());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                if let Some(req) = http::read_request(&mut stream) {
                    let resp = if req.path == CARD_PATH {
                        http::ServerResponse::json(200, "OK", card_json.clone())
                    } else {
                        http::ServerResponse::text(404, "Not Found", "unknown path")
                    };
                    let _ = stream.write_all(&resp.to_wire());
                    let _ = stream.flush();
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

// ───────────────────────── card construction ─────────────────────────

/// Build the card advertising the repo at `root`, fetchable by a peer over ssh
/// as `ssh://user@host<abs-path>`. Refs and the root oid come from the repo's
/// branches (best-effort via git); name is the repo directory.
pub fn card_for_repo(root: &Path, user: &str, host: &str) -> Result<ProjectCard, String> {
    let abs = root.canonicalize().map_err(|e| format!("repo path: {e}"))?;
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let provider = format!("ssh://{user}@{host}{}", abs.to_string_lossy());
    let refs = git_refs(&abs);
    let root_oid = refs.first().map(|(_, o)| o.clone()).unwrap_or_default();
    Ok(ProjectCard {
        name,
        description: String::new(),
        root_oid,
        refs,
        providers: vec![provider],
        created_unix: now_unix(),
    })
}

fn git_refs(repo: &Path) -> Vec<(String, String)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", "--format=%(refname) %(objectname)", "refs/heads"])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            return String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| {
                    let mut it = l.split_whitespace();
                    Some((it.next()?.to_string(), it.next()?.to_string()))
                })
                .collect();
        }
    }
    Vec::new()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// This host's LAN IP (the source address used to reach the outside world).
pub fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// The current user (for the ssh provider url).
pub fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".into())
}

// ───────────────────────── look up + install ─────────────────────────

/// The raw HTTP request that fetches a peer's card. Pure — unit-tested.
pub fn card_get_request(addr: &str) -> String {
    http::Request {
        method: "GET",
        path: CARD_PATH,
        host: addr,
        content_type: "text/plain",
        body: String::new(),
    }
    .to_wire()
}

/// GET a peer's card from `addr` ("host:port") and parse it.
pub fn lookup_peer(addr: &str, timeout: Duration) -> Result<ProjectCard, String> {
    let raw = http::roundtrip(addr, &card_get_request(addr), timeout)
        .map_err(|e| format!("could not reach {addr}: {e}"))?;
    let resp = http::parse_response(&raw);
    if resp.status != 200 {
        return Err(format!("peer at {addr} returned status {}", resp.status));
    }
    ProjectCard::from_bytes(resp.body.as_bytes()).map_err(|e| format!("bad card from {addr}: {e}"))
}

/// `git clone` a card's first ssh:// provider into `dest`/<name>. Pre-checked so
/// only ssh providers are attempted (http/dht providers come later).
pub fn install_card(name: &str, provider: &str, dest: &str) -> Result<String, String> {
    if !provider.starts_with("ssh://") {
        return Err(format!("only ssh:// providers are supported yet (got '{provider}')"));
    }
    let safe_name = name.replace(['/', '\\'], "_");
    let target = format!("{}/{}", dest.trim_end_matches('/'), safe_name);
    let status = Command::new("git")
        .args(["clone", provider, &target])
        .status()
        .map_err(|e| format!("git clone: {e}"))?;
    if status.success() {
        Ok(format!("cloned {name} → {target}"))
    } else {
        Err(format!("git clone exited {:?}", status.code()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn card_has_name_and_ssh_provider() {
        let tmp = std::env::temp_dir().join(format!("gm_net_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let card = card_for_repo(&tmp, "me", "10.0.0.5").unwrap();
        assert!(card.name.starts_with("gm_net_"), "name from dir: {}", card.name);
        assert_eq!(card.providers.len(), 1);
        assert!(card.providers[0].starts_with("ssh://me@10.0.0.5/"), "provider: {}", card.providers[0]);
        // content-addressed id is stable across re-serialization
        assert_eq!(card.id(), ProjectCard::from_bytes(&card.to_bytes()).unwrap().id());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn card_get_request_is_a_get_for_the_card_path() {
        let req = card_get_request("192.168.1.7:9876");
        assert!(req.starts_with("GET /card HTTP/1.1"), "req: {req}");
        assert!(req.contains("Host: 192.168.1.7:9876"));
    }

    #[test]
    fn install_rejects_non_ssh_providers() {
        assert!(install_card("x", "http://host/x", "/tmp").is_err());
        assert!(install_card("x", "dht://abcd", "/tmp").is_err());
    }

    #[test]
    fn advertiser_tracks_serving_state() {
        let a = Advertiser::default();
        assert!(!a.is_serving());
        a.begin("10.0.0.5:9876".into());
        assert!(a.is_serving());
        assert_eq!(a.addr(), "10.0.0.5:9876");
        assert!(!a.should_stop());
        a.end();
        assert!(!a.is_serving());
        assert!(a.should_stop(), "end() requests the loop stop");
    }
}
