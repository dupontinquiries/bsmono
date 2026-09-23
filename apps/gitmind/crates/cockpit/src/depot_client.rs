//! gitmind as a depot client: pair once, then sign every write, so neither
//! the CLI nor gitmind-egui ever asks the operator to fetch an admin token.
//! The protocol is `shared/depot-auth`; this module is the client's state
//! and transport.
//!
//! Files (under `$XDG_CONFIG_HOME/gitmind/`, all 0600):
//!
//! ```text
//! depot.conf         server=HOST:PORT   ssh=USER@HOST   root=/var/lib/depot
//! depot-client.key   name=<client name>  seed=<ed25519 seed hex>   (the secret)
//! depot-pair         id=<request>  nonce=<depot's nonce>          (while pending)
//! ```
//!
//! The pairing loop, each step visible to the UI as a [`PairState`]:
//! [`Client::start_pairing`] (POST /api/pair, compute the code locally) →
//! approve with the code by one of: [`Client::approve_via_ssh`] (runs
//! `depot pair approve` on the depot host through gitmind's ssh; ssh access
//! is the authority), [`Client::approve_with`] (an admin token, or signed by
//! this client if it already holds `approve`) → [`Client::poll`] until the
//! request reads approved.

use depot_auth::Identity;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_SCOPES: &str = "ingest,packs";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conf {
    /// host:port of the depot's HTTP server.
    pub server: String,
    /// user@host for approving over ssh; empty = not offered.
    pub ssh: String,
    /// The depot root on that host; empty = depot's default.
    pub root: String,
}

impl Conf {
    pub fn parse(text: &str) -> Conf {
        let get = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}="))).unwrap_or("").trim().to_string();
        Conf { server: normalize_server(&get("server")), ssh: get("ssh"), root: get("root") }
    }
    pub fn to_text(&self) -> String {
        format!("server={}\nssh={}\nroot={}\n", self.server, self.ssh, self.root)
    }
}

/// Accept `http://depot.lan:8000/` as well as `depot.lan:8000`.
pub fn normalize_server(s: &str) -> String {
    let s = s.trim().trim_start_matches("http://").trim_end_matches('/');
    if s.is_empty() || s.contains(':') {
        s.to_string()
    } else {
        format!("{s}:8000")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    /// No depot.conf server line.
    NoServer,
    /// No key yet, or a key the depot does not know.
    Unpaired,
    /// Waiting for approval; `code` is what the approver must see.
    Pending { id: String, code: String },
    Paired { name: String },
    Denied,
    /// The request expired or vanished; pair again.
    Lost,
}

// ── pure: request wire, response parse, commands ──

/// One HTTP/1.1 request as bytes; `Connection: close`, one per connection.
pub fn wire(method: &str, path_and_query: &str, host: &str, auth: Option<&str>, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut head = format!("{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    if !content_type.is_empty() {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(a) = auth {
        head.push_str(&format!("Authorization: {a}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

pub fn parse_response(raw: &[u8]) -> (u16, String) {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, body.to_string())
}

pub fn pair_body(name: &str, public_hex: &str, scopes: &str) -> String {
    format!("name={name}\npublic={public_hex}\nscopes={scopes}\n")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What gitmind runs on the depot host to approve. Id and code are
/// validated first (hex / 6 digits), so only the root needs quoting.
pub fn approve_ssh_command(root: &str, id: &str, code: &str) -> Result<String, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("bad request id '{id}'"));
    }
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("bad code '{code}'"));
    }
    let root = if root.is_empty() { String::new() } else { format!(" --root {}", shell_quote(root)) };
    Ok(format!("depot{root} pair approve {id} --code {code}"))
}

/// A default client name: gitmind + this machine's hostname, file-safe.
pub fn default_name() -> String {
    let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
    let host: String = host.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let n = if host.is_empty() { "gitmind".to_string() } else { format!("gitmind-{host}") };
    n.chars().take(48).collect()
}

fn json_field(body: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get(key).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()))
}

fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    // /dev/urandom is always there on Linux; a nonce needs uniqueness, and
    // the clock fallback still gives that per process.
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b.iter_mut().enumerate().for_each(|(i, x)| *x = (t >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(37));
    }
    depot_auth::hex(&b)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn write_private(p: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

// ── IO: one round trip ──

pub fn http(host: &str, request: &[u8]) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(host).map_err(|e| format!("{host}: {e}"))?;
    let t = Some(Duration::from_secs(20));
    let _ = s.set_read_timeout(t);
    let _ = s.set_write_timeout(t);
    s.write_all(request).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    Ok(parse_response(&raw))
}

// ── the client ──

pub struct Client {
    dir: PathBuf,
}

impl Client {
    pub fn new(config_dir: &Path) -> Client {
        Client { dir: config_dir.to_path_buf() }
    }

    /// `$XDG_CONFIG_HOME/gitmind`, else `~/.config/gitmind`.
    pub fn default_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("gitmind"))
    }

    pub fn conf(&self) -> Conf {
        Conf::parse(&std::fs::read_to_string(self.dir.join("depot.conf")).unwrap_or_default())
    }

    pub fn save_conf(&self, c: &Conf) -> Result<(), String> {
        write_private(&self.dir.join("depot.conf"), &c.to_text())
    }

    pub fn identity(&self) -> Option<Identity> {
        let t = std::fs::read_to_string(self.dir.join("depot-client.key")).ok()?;
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(str::trim);
        Identity::from_seed_hex(get("name")?, get("seed")?)
    }

    fn pending(&self) -> Option<(String, String)> {
        let t = std::fs::read_to_string(self.dir.join("depot-pair")).ok()?;
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|s| s.trim().to_string());
        Some((get("id")?, get("nonce")?))
    }

    /// Step 1. A fresh key each time pairing starts (an unpaired key has no
    /// value, and an old one may be what the depot revoked).
    pub fn start_pairing(&self, name: &str, scopes: &str) -> Result<PairState, String> {
        let c = self.conf();
        if c.server.is_empty() {
            return Ok(PairState::NoServer);
        }
        if !depot_auth::valid_name(name) {
            return Err(format!("client name '{name}': letters, digits, - _ . only"));
        }
        let id = Identity::generate(name);
        let body = pair_body(name, &id.public_hex(), scopes);
        let (st, resp) = http(&c.server, &wire("POST", "/api/pair", &c.server, None, "text/plain", body.as_bytes()))?;
        if st != 200 {
            return Err(format!("depot refused the pairing request ({st}): {}", resp.trim()));
        }
        let rid = json_field(&resp, "id").ok_or("depot's reply has no id")?;
        let nonce = json_field(&resp, "nonce").ok_or("depot's reply has no nonce")?;
        write_private(&self.dir.join("depot-client.key"), &format!("name={name}\nseed={}\n", id.seed_hex()))?;
        write_private(&self.dir.join("depot-pair"), &format!("id={rid}\nnonce={nonce}\n"))?;
        Ok(PairState::Pending { code: depot_auth::pair_code(&id.public(), &nonce), id: rid })
    }

    /// Where pairing stands, asking the depot when a request is pending.
    pub fn poll(&self) -> PairState {
        let c = self.conf();
        if c.server.is_empty() {
            return PairState::NoServer;
        }
        let Some(me) = self.identity() else { return PairState::Unpaired };
        let Some((rid, nonce)) = self.pending() else {
            // No pending request: paired if the depot lists us.
            return match http(&c.server, &wire("GET", "/api/clients", &c.server, None, "", b"")) {
                Ok((200, body)) if lists_client(&body, &me.name) => PairState::Paired { name: me.name },
                _ => PairState::Unpaired,
            };
        };
        let code = depot_auth::pair_code(&me.public(), &nonce);
        match http(&c.server, &wire("GET", &format!("/api/pair/{rid}"), &c.server, None, "", b"")) {
            Ok((200, body)) => match json_field(&body, "status").as_deref() {
                Some("approved") => {
                    let _ = std::fs::remove_file(self.dir.join("depot-pair"));
                    PairState::Paired { name: me.name }
                }
                Some("pending") => PairState::Pending { id: rid, code },
                Some("denied") => {
                    let _ = std::fs::remove_file(self.dir.join("depot-pair"));
                    PairState::Denied
                }
                _ => {
                    let _ = std::fs::remove_file(self.dir.join("depot-pair"));
                    PairState::Lost
                }
            },
            // Depot unreachable: report what we know rather than guess.
            _ => PairState::Pending { id: rid, code },
        }
    }

    /// Step 2a: approve through gitmind's ssh to the depot host. The code
    /// passed is the one computed HERE, so depot refuses if it received a
    /// different key.
    pub fn approve_via_ssh(&self) -> Result<String, String> {
        let c = self.conf();
        let (user, host) = c.ssh.split_once('@').ok_or("depot.conf has no ssh=USER@HOST")?;
        let PairState::Pending { id, code } = self.poll() else { return Err("no pending pairing request".into()) };
        let cmd = approve_ssh_command(&c.root, &id, &code)?;
        crate::deploy::remote_capture(user, host, &cmd).map(|o| o.trim().to_string())
    }

    /// Step 2b: approve over HTTP with an admin token (typed into the GUI
    /// once, never stored) or, with `token` empty, signed by another
    /// paired client's identity that holds `approve`.
    pub fn approve_with(&self, token: &str, approver: Option<&Identity>) -> Result<String, String> {
        let c = self.conf();
        let PairState::Pending { id, code } = self.poll() else { return Err("no pending pairing request".into()) };
        let pq = format!("/api/pair/{id}/approve?code={code}");
        let auth = match (token.trim(), approver) {
            ("", Some(a)) => a.authorization("POST", &pq, now(), &random_hex(16), 0),
            ("", None) => return Err("give an admin token or an approving client".into()),
            (t, _) => format!("Bearer {t}"),
        };
        let (st, body) = http(&c.server, &wire("POST", &pq, &c.server, Some(&auth), "", b""))?;
        if st == 200 {
            Ok(body.trim().to_string())
        } else {
            Err(format!("depot refused ({st}): {}", body.trim()))
        }
    }

    /// Any write, signed with this client's key.
    pub fn signed(&self, method: &str, path_and_query: &str, content_type: &str, body: &[u8]) -> Result<(u16, String), String> {
        let c = self.conf();
        let me = self.identity().ok_or("not paired: no depot-client.key")?;
        let auth = me.authorization(method, path_and_query, now(), &random_hex(16), body.len() as u64);
        http(&c.server, &wire(method, path_and_query, &c.server, Some(&auth), content_type, body))
    }

    /// An ingest (PUT), signed, then checked: the body is not in the
    /// signature, so the client compares the sha256 depot reports storing
    /// with what it sent. Returns depot's reply.
    pub fn put_checked(&self, path: &str, body: &[u8]) -> Result<String, String> {
        let (st, reply) = self.signed("PUT", path, "application/octet-stream", body)?;
        if st != 200 {
            return Err(format!("depot refused ({st}): {}", reply.trim()));
        }
        let hash = json_field(&reply, "hash").ok_or_else(|| format!("depot's reply names no hash: {}", reply.trim()))?;
        if !depot_auth::client_checks_stored_hash(body, &hash) {
            return Err(format!("depot stored {hash}, which is not the sha256 of what was sent"));
        }
        Ok(reply.trim().to_string())
    }

    /// Forget this client locally (the depot side: `depot pair revoke`).
    pub fn forget(&self) {
        let _ = std::fs::remove_file(self.dir.join("depot-client.key"));
        let _ = std::fs::remove_file(self.dir.join("depot-pair"));
    }
}

/// Whether depot's `/api/clients` reply names this client. Parsed, not
/// substring-matched: serde_json writes `"name": "x"` or `"name":"x"`
/// depending on the serializer, and a substring match would also accept a
/// client whose name merely contains ours.
pub fn lists_client(body: &str, name: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("clients").and_then(|c| c.as_array()).map(|a| a.iter().any(|c| c.get("name").and_then(|n| n.as_str()) == Some(name))))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_and_commands() {
        let raw = wire("PUT", "/api/apk/fleet?x=1", "h:1", Some("Depot-Sig client=a"), "application/octet-stream", b"\x00ab");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("PUT /api/apk/fleet?x=1 HTTP/1.1\r\nHost: h:1\r\n"));
        assert!(text.contains("Content-Length: 3\r\n") && text.contains("Authorization: Depot-Sig client=a\r\n"));
        assert!(raw.ends_with(b"\r\n\r\n\x00ab"));
        assert_eq!(parse_response(b"HTTP/1.1 422 Unprocessable\r\nX: y\r\n\r\n{\"e\":1}"), (422, "{\"e\":1}".into()));
        assert_eq!(approve_ssh_command("", "a1b2", "012345").unwrap(), "depot pair approve a1b2 --code 012345");
        assert_eq!(approve_ssh_command("/srv/my depot", "a1", "999999").unwrap(), "depot --root '/srv/my depot' pair approve a1 --code 999999");
        assert!(approve_ssh_command("", "a1;rm", "012345").is_err());
        assert!(approve_ssh_command("", "a1", "12345").is_err());
        assert_eq!(normalize_server("http://depot.lan:8000/"), "depot.lan:8000");
        assert_eq!(normalize_server("depot.lan"), "depot.lan:8000");
        let c = Conf { server: "d:8000".into(), ssh: "admin@d".into(), root: "/var/lib/depot".into() };
        assert_eq!(Conf::parse(&c.to_text()), c);
        assert!(depot_auth::valid_name(&default_name()));
    }

    #[test]
    fn the_client_list_is_parsed_not_substring_matched() {
        assert!(lists_client(r#"{"clients":[{"name":"gitmind-a","scopes":"ingest"}]}"#, "gitmind-a"));
        assert!(lists_client(r#"{"clients": [{"name": "gitmind-a", "scopes": "ingest"}]}"#, "gitmind-a"), "spaces after colons");
        assert!(!lists_client(r#"{"clients":[{"name":"gitmind-ab"}]}"#, "gitmind-a"), "a longer name is not ours");
        assert!(!lists_client("not json", "gitmind-a"));
    }
}
