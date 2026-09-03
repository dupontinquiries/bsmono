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

/// What `curl http://<addr>/` returns: instructions, in plain text.
///
/// # The bug this fixes
///
/// The share UI advertises a bare `host:port` and the operator does the
/// obvious thing — `curl -O http://192.168.1.20:9876` from the phone. That
/// failed, and the way it failed gave nothing to go on: this server answers
/// ONLY `/card`, so `/` returned "unknown path" with a 404, and `curl -O` on a
/// URL with no filename component reports "Remote filename has no length"
/// before it even shows the body.
///
/// So an operator following the address the app gave them got an error about
/// curl's arguments for a server that was working correctly and had simply not
/// been asked for the one path it serves.
///
/// The address is a discovery endpoint, not a download. Rather than pretend
/// otherwise by serving a file, `/` now explains what this is and gives the
/// two commands that do work — which is what the operator was looking for.
pub fn bootstrap_text(addr: &str, card_name: &str) -> String {
    format!(
        "gitmind share — {card_name}\n\
         \n\
         This is a DISCOVERY endpoint, not a file download. There is nothing\n\
         here to `curl -O`; the repo travels over ssh, not over this port.\n\
         \n\
         The card, as JSON:\n\
         \x20 curl http://{addr}{CARD_PATH}\n\
         \n\
         Add this peer from another device:\n\
         \x20 gin peer add http://{addr}\n\
         \n\
         The card names the ssh:// provider to fetch from. That machine needs\n\
         to accept your key already — this port does not carry credentials.\n"
    )
}

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
pub fn serve_loop(listener: TcpListener, card_json: String, name: String, adv: Arc<Advertiser>) -> Result<(), String> {
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
                    } else if req.path == "/" {
                        // 200, not a redirect or a 404. An operator who typed
                        // the advertised address deserves an answer, and a
                        // 404 here is what sent them looking for a mistake
                        // they had not made.
                        http::ServerResponse::text(200, "OK", bootstrap_text(&adv.addr(), &name))
                    } else {
                        // Still 404 for anything else, but say what IS served —
                        // "unknown path" alone gives a reader nothing to try
                        // next.
                        http::ServerResponse::text(
                            404, "Not Found",
                            format!("unknown path — this server serves / and {CARD_PATH}\n"))
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

/// How usable an address is for a peer on the same LAN.
///
/// Ordered worst to best so `max_by_key` picks the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddrKind {
    /// `127.0.0.0/8`. Cannot serve another device, ever.
    Loopback,
    /// `169.254.0.0/16` — no DHCP happened. Reachable only if the peer also
    /// self-assigned, which is rare and not worth advertising over anything
    /// else.
    LinkLocal,
    /// `100.64.0.0/10`. Tailscale's range and carrier-grade NAT. Routable for
    /// the VPN's members and invisible to a phone on the same wifi, which is
    /// the case that makes the default-route trick pick the wrong answer.
    Cgnat,
    /// Anything else — a public address, or something unrecognised.
    Other,
    /// RFC1918: `10/8`, `172.16/12`, `192.168/16`. What a home LAN uses, and
    /// what the phone is almost certainly on.
    PrivateLan,
}

/// Classify a dotted-quad IPv4 address.
pub fn addr_kind(ip: &str) -> AddrKind {
    let o: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
    if o.len() != 4 {
        return AddrKind::Other;
    }
    match (o[0], o[1]) {
        (127, _) => AddrKind::Loopback,
        (169, 254) => AddrKind::LinkLocal,
        (10, _) => AddrKind::PrivateLan,
        (192, 168) => AddrKind::PrivateLan,
        (172, b) if (16..=31).contains(&b) => AddrKind::PrivateLan,
        (100, b) if (64..=127).contains(&b) => AddrKind::Cgnat,
        _ => AddrKind::Other,
    }
}

/// Choose the address to advertise, given every one this host has.
///
/// # Why the default-route trick is not enough on its own
///
/// [`local_ip`] connects a UDP socket to 8.8.8.8 and reads back the local
/// address, which returns whichever interface holds the DEFAULT ROUTE. That is
/// the LAN interface most of the time and the wrong answer twice:
///
/// * **A VPN holds the route.** Tailscale or wireguard puts a `100.64/10`
///   address there, and the phone on the same wifi cannot reach it. The share
///   advertises an address that is perfectly valid and completely unreachable.
/// * **There is no default route at all.** A desktop and a phone on a LAN with
///   no internet — or a captive portal before sign-in — makes `connect` fail,
///   `local_ip` return `None`, and the caller fall back to `127.0.0.1`. That
///   is advertised as if it were fine, and it can never work from another
///   device.
///
/// So: prefer a private-LAN address over everything, and NEVER return loopback
/// when any other address exists. Returns `None` rather than a loopback-only
/// answer, so the caller can say "no usable address" instead of printing one
/// that cannot work.
pub fn pick_share_ip(candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .filter(|ip| addr_kind(ip) != AddrKind::Loopback)
        // `max_by_key` keeps the LAST maximum; iterating in order and taking
        // the last means a later interface wins a tie, which is arbitrary but
        // stable — the enumeration order is the OS's and does not change
        // between calls.
        .max_by_key(|ip| addr_kind(ip))
        .cloned()
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
    // Transport seam (see maintenance/SHARING.md): providers are scheme-prefixed
    // URLs. ssh:// is wired today; bt:// (Bluetooth OBEX) and dht:// (opendht /
    // snapclone device IDs) are reserved for when sharekit lands.
    if provider.starts_with("bt://") {
        return Err("bt:// providers aren't wired yet — use the Bluetooth send panel to push a bundle for now".into());
    }
    if provider.starts_with("dht://") {
        return Err("dht:// providers aren't wired yet (coming with sharekit + opendht device IDs)".into());
    }
    if !provider.starts_with("ssh://") {
        return Err(format!("unsupported provider scheme: '{provider}' (expected ssh://)"));
    }
    let safe_name = name.replace(['/', '\\'], "_");
    let target = format!("{}/{}", dest.trim_end_matches('/'), safe_name);
    // Same non-interactive ssh the deploy paths use, so the clone can't hang on a
    // host-key/password prompt (a new peer's key is auto-trusted into known_hosts).
    let status = Command::new("git")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new",
        )
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
    #[test]
    fn the_bootstrap_page_answers_the_question_the_operator_asked() {
        // They ran `curl -O <addr>` from the phone and got nothing usable. The
        // page has to say what this is, and give the commands that DO work —
        // an error page that only says "no" leaves them where they started.
        let t = bootstrap_text("192.168.1.20:9876", "monorepo");
        assert!(t.contains("monorepo"), "the page does not say which repo");
        // Line-bounded, not a substring: `.contains("…/card")` is satisfied by
        // "…/cards", so a page printing a path the server does not serve
        // passed. Found by breaking exactly that.
        assert!(t.lines().any(|l| l.trim() == "curl http://192.168.1.20:9876/card"),
                "no fetchable card URL on a line of its own: {t}");
        assert!(t.contains("gin peer add"), "no command to actually add the peer");
        // And it says plainly there is nothing to download, because that is the
        // belief that sent them here.
        assert!(t.to_lowercase().contains("not a file download"),
                "the page does not correct the assumption: {t}");
    }

    #[test]
    fn the_card_path_in_the_page_matches_the_one_the_server_serves() {
        // Two places naming one path is how a page confidently prints a URL
        // that 404s.
        let t = bootstrap_text("h:1", "r");
        let want = format!("curl http://h:1{CARD_PATH}");
        assert!(t.lines().any(|l| l.trim() == want),
                "the page's card URL is not exactly {want}: {t}");
    }

    #[test]
    fn an_unknown_path_says_what_is_served() {
        // The old reply was "unknown path", which gives a reader nothing to try
        // next. This is the string the 404 arm builds.
        let msg = format!("unknown path — this server serves / and {CARD_PATH}\n");
        assert!(msg.contains("/card"));
        assert!(msg.contains("serves /"));
    }

    #[test]
    fn a_private_lan_address_beats_everything_else() {
        // The reported failure: the phone could not reach the advertised
        // address. A VPN holding the default route is one way that happens.
        let c: Vec<String> = ["100.101.5.7", "192.168.1.20", "127.0.0.1"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_share_ip(&c).as_deref(), Some("192.168.1.20"));

        // All three RFC1918 blocks, and 172.32 is NOT one of them.
        assert_eq!(addr_kind("10.0.0.5"), AddrKind::PrivateLan);
        assert_eq!(addr_kind("172.16.0.1"), AddrKind::PrivateLan);
        assert_eq!(addr_kind("172.31.255.254"), AddrKind::PrivateLan);
        assert_eq!(addr_kind("172.32.0.1"), AddrKind::Other);
        assert_eq!(addr_kind("172.15.0.1"), AddrKind::Other);
    }

    #[test]
    fn loopback_is_never_advertised() {
        // `local_ip()` returning None makes the caller fall back to
        // 127.0.0.1, which is then shown as if it were a usable address. It
        // can never serve another device.
        let only_loop: Vec<String> = vec!["127.0.0.1".into()];
        assert_eq!(pick_share_ip(&only_loop), None,
                   "loopback was offered as a share address");
        let with_lan: Vec<String> = vec!["127.0.0.1".into(), "10.1.2.3".into()];
        assert_eq!(pick_share_ip(&with_lan).as_deref(), Some("10.1.2.3"));
        assert_eq!(pick_share_ip(&[]), None);
    }

    #[test]
    fn a_tailscale_address_loses_to_a_lan_one_and_wins_over_nothing() {
        // Both directions. It IS reachable for VPN members, so it is better
        // than refusing to serve; it is just not what a phone on the wifi
        // wants.
        assert_eq!(addr_kind("100.64.0.1"), AddrKind::Cgnat);
        assert_eq!(addr_kind("100.127.255.255"), AddrKind::Cgnat);
        // 100.128 is outside the /10 and is a normal public address.
        assert_eq!(addr_kind("100.128.0.1"), AddrKind::Other);
        let vpn_only: Vec<String> = vec!["100.101.5.7".into(), "127.0.0.1".into()];
        assert_eq!(pick_share_ip(&vpn_only).as_deref(), Some("100.101.5.7"));
    }

    #[test]
    fn a_link_local_address_is_a_last_resort_but_not_refused() {
        // 169.254 means no DHCP happened. Worth offering over nothing, and
        // ranked below anything routable.
        assert_eq!(addr_kind("169.254.3.4"), AddrKind::LinkLocal);
        let ll: Vec<String> = vec!["169.254.3.4".into()];
        assert_eq!(pick_share_ip(&ll).as_deref(), Some("169.254.3.4"));
        let both: Vec<String> = vec!["169.254.3.4".into(), "192.168.0.9".into()];
        assert_eq!(pick_share_ip(&both).as_deref(), Some("192.168.0.9"));
    }

    #[test]
    fn a_malformed_address_does_not_panic_or_win() {
        for junk in ["", "not.an.ip", "1.2.3", "1.2.3.4.5", "999.1.1.1"] {
            let _ = addr_kind(junk);
        }
        // `Other` outranks Cgnat, so junk must not beat a real LAN address.
        let c: Vec<String> = vec!["not.an.ip".into(), "192.168.1.5".into()];
        assert_eq!(pick_share_ip(&c).as_deref(), Some("192.168.1.5"));
    }

}
