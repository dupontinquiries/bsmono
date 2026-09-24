//! join_server — the laptop side of [`join`](crate::join), actually served.
//!
//! Until this module the Fleet page copied `curl http://<ip>:8080/join/CODE | sh` to the
//! clipboard, and nothing listened: the command failed on the phone, and `JoinCode`'s single
//! use and expiry were enforced nowhere. This is the listener, for a phone that has a browser
//! and `curl` and nothing else (no sshd, no key, no app).
//!
//! ## The session, as the phone sees it
//!
//! 1. The browser opens `http://<laptop>:<port>/` (every address the laptop has is shown,
//!    the USB-network link included, not just the default route's).
//! 2. **Login**: the 6-digit one-time code shown on the laptop. Three wrong codes close the
//!    session.
//! 3. **Number match**: the phone shows three numbers, the laptop one; the operator taps the
//!    one the laptop shows. One wrong pick closes the session (a guess wins 1 in 3, once).
//! 4. **The command**: the page shows a line that fetches the join script with a single-use
//!    128-bit token and checks its SHA-256 before running it. The laptop shows the same
//!    fingerprint.
//! 5. The script (`join::join_script`: sshd on, fleet user key-only, key installed) reports
//!    hostname, host key and arch back; the laptop records them with the phone's address AS
//!    SEEN ON THE CONNECTION, which is the ssh target to use next.
//!
//! ## What each step buys, plainly
//!
//! This is HTTP on a local network; there is no TLS and no pinned server key.
//! - The **code** stops anyone on the LAN from fetching the script or reaching the matching
//!   step: 3 tries at 10^6, then the session is gone. It plays the role of a login and
//!   password, and a one-time code is stronger than a reusable password (nothing to leak,
//!   nothing stored).
//! - The **number match** confirms the page the phone is looking at belongs to the session
//!   the laptop is showing: the numbers exist only on the laptop's screen and in its session.
//!   It does not stop an attacker who RELAYS the whole exchange to the real laptop.
//! - The **fingerprint** is what catches that relay altering the script: the command checks
//!   the fetched script against a SHA-256 prefix, and the operator compares that prefix with
//!   the one on the laptop screen. Skip the comparison and a relaying attacker on the LAN
//!   can substitute the script. That is the honest limit without TLS.
//!
//! Layers: [`Session`] is the engine (request in, response out, clock and randomness passed
//! in), tested without sockets. [`serve`] is the transport: a std TcpListener, one short
//! request per connection.

use crate::join::{JoinCode, JoinReport};
use std::sync::{Arc, Mutex};

/// Wrong codes before the session closes.
pub const MAX_LOGIN_FAILURES: u32 = 3;
/// Hex digits of the script's SHA-256 shown on both screens (48 bits: finding a different
/// script with the same prefix is out of reach inside the code's 10-minute life).
pub const FINGERPRINT_HEX: usize = 12;
/// The largest request body accepted (a form post is tiny).
pub const MAX_BODY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for the code.
    Login,
    /// Code accepted; the phone is choosing among `options`.
    Matching { options: [u16; 3], correct: u16 },
    /// Matched; the script may be fetched with `token`.
    Confirmed { token: String },
    /// The script reported back.
    Joined { report: JoinReport, peer: String },
    /// Over, with the reason (shown on both sides).
    Closed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Response {
    fn html(status: u16, title: &str, inner: &str) -> Response {
        Response { status, content_type: "text/html; charset=utf-8", body: page(title, inner) }
    }
    fn text(status: u16, body: impl Into<String>) -> Response {
        Response { status, content_type: "text/plain; charset=utf-8", body: body.into() }
    }
}

pub struct Session {
    pub code: JoinCode,
    /// `http://<address>:<port>` as the phone reaches it; the script's callback uses it.
    base_url: String,
    fleet_user: String,
    pubkey: String,
    pub stage: Stage,
    login_failures: u32,
    rand: Box<dyn FnMut() -> u64 + Send>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn page(title: &str, inner: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>{t}</title><style>body{{font:18px system-ui,sans-serif;margin:1.2em;max-width:34em}}\
         input,button{{font:inherit;padding:.6em;margin:.3em 0}}button.n{{font-size:2em;min-width:3.2em;margin:.3em}}\
         textarea{{width:100%;font:14px monospace;height:7em}}.w{{color:#a40}}</style></head>\
         <body><h2>gitmind: join this phone</h2>{inner}</body></html>",
        t = esc(title)
    )
}

/// A form field from an `application/x-www-form-urlencoded` body (digits and hex here, so
/// only `+` and `%XX` need decoding).
pub fn form_field(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| {
            let v = v.replace('+', " ");
            let mut out = Vec::new();
            let b = v.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' && i + 2 < b.len() {
                    if let Ok(n) = u8::from_str_radix(&v[i + 1..i + 3], 16) { out.push(n); i += 3; continue; }
                }
                out.push(b[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    })
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

impl Session {
    /// A new session. `rand` supplies randomness (the transport passes the OS CSPRNG; tests a
    /// fixed sequence).
    pub fn new(base_url: &str, fleet_user: &str, pubkey: &str, now: u64, mut rand: Box<dyn FnMut() -> u64 + Send>) -> Session {
        let code = crate::join::code_from_bytes(&rand().to_le_bytes());
        Session { code: JoinCode::issue(code, now), base_url: base_url.trim_end_matches('/').to_string(), fleet_user: fleet_user.into(), pubkey: pubkey.into(), stage: Stage::Login, login_failures: 0, rand }
    }

    /// The number the laptop shows while the phone is choosing.
    pub fn laptop_number(&self) -> Option<u16> {
        match self.stage { Stage::Matching { correct, .. } => Some(correct), _ => None }
    }

    /// The exact script the token unlocks.
    pub fn script(&self) -> Option<String> {
        match &self.stage {
            Stage::Confirmed { token } => Some(crate::join::join_script(&self.base_url, token, &self.fleet_user, &self.pubkey)),
            _ => None,
        }
    }

    /// The SHA-256 prefix of that script, shown on the laptop to compare with the phone's.
    pub fn fingerprint(&self) -> Option<String> {
        self.script().map(|s| sha256_hex(&s)[..FINGERPRINT_HEX].to_string())
    }

    /// The line the phone runs: fetch, check the fingerprint, then run.
    pub fn phone_command(&self) -> Option<String> {
        let Stage::Confirmed { token } = &self.stage else { return None };
        let full = sha256_hex(&self.script()?);
        Some(format!(
            "curl -fsS {b}/join/{token} -o /tmp/gitmind-join.sh && echo '{full}  /tmp/gitmind-join.sh' | sha256sum -c - && sh /tmp/gitmind-join.sh",
            b = self.base_url
        ))
    }

    fn close(&mut self, why: &str) -> Response {
        self.stage = Stage::Closed(why.to_string());
        Response::html(403, "closed", &format!("<p class=w>{}</p><p>Start a new join on the laptop.</p>", esc(why)))
    }

    fn token(&mut self) -> String {
        let (a, b) = ((self.rand)(), (self.rand)());
        format!("{a:016x}{b:016x}")
    }

    /// One request. `peer` is the phone's address on this connection.
    pub fn handle(&mut self, method: &str, path: &str, body: &str, peer: &str, now: u64) -> Response {
        if let Stage::Closed(why) = &self.stage {
            return Response::html(410, "closed", &format!("<p class=w>{}</p>", esc(why)));
        }
        // The session's life is the code's: `expires_at`, not `valid_at` -- the latter also goes
        // false once the code is redeemed, which is every stage after login.
        if !matches!(self.stage, Stage::Joined { .. }) && now >= self.code.expires_at {
            return self.close("This join has expired.");
        }
        match (method, path) {
            ("GET", "/") => match &self.stage {
                Stage::Login => Response::html(200, "code", "<form method=post action=/login><p>The code on the laptop:</p>\
                    <input name=code inputmode=numeric autocomplete=off maxlength=6 autofocus> <button>Continue</button></form>"),
                Stage::Matching { options, .. } => Response::html(200, "match", &pick_form(options)),
                Stage::Confirmed { .. } => Response::html(200, "command", &self.command_html()),
                Stage::Joined { .. } => Response::html(200, "joined", "<p>Joined. The laptop has this phone now.</p>"),
                Stage::Closed(_) => unreachable!(),
            },
            ("POST", "/login") => {
                if self.stage != Stage::Login {
                    return Response::html(409, "login", "<p>Already past the code. <a href=/>Continue</a></p>");
                }
                let offered = form_field(body, "code").unwrap_or_default();
                if self.code.redeem(offered.trim(), now) {
                    let correct = 10 + ((self.rand)() % 90) as u16;
                    let mut options = [correct, 0, 0];
                    let mut k = 1;
                    while k < 3 {
                        let n = 10 + ((self.rand)() % 90) as u16;
                        if !options[..k].contains(&n) { options[k] = n; k += 1; }
                    }
                    let shift = ((self.rand)() % 3) as usize;
                    options.rotate_left(shift);
                    self.stage = Stage::Matching { options, correct };
                    Response::html(200, "match", &pick_form(&options))
                } else {
                    self.login_failures += 1;
                    if self.login_failures >= MAX_LOGIN_FAILURES {
                        return self.close("Too many wrong codes.");
                    }
                    Response::html(401, "code", &format!("<p class=w>Wrong code ({} of {} tries left).</p><p><a href=/>Try again</a></p>", MAX_LOGIN_FAILURES - self.login_failures, MAX_LOGIN_FAILURES))
                }
            }
            ("POST", "/pick") => {
                let Stage::Matching { correct, .. } = self.stage else {
                    return Response::html(409, "pick", "<p><a href=/>Continue</a></p>");
                };
                let picked: Option<u16> = form_field(body, "n").and_then(|v| v.trim().parse().ok());
                if picked != Some(correct) {
                    return self.close("That was not the number on the laptop. If it was not on the laptop at all, this page may not be your laptop's.");
                }
                let token = self.token();
                self.stage = Stage::Confirmed { token };
                Response::html(200, "command", &self.command_html())
            }
            ("GET", p) if p.starts_with("/join/") => match &self.stage {
                Stage::Confirmed { token } if p == format!("/join/{token}") => Response::text(200, self.script().unwrap_or_default()),
                _ => Response::text(404, "no such join\n"),
            },
            ("POST", p) if p.starts_with("/joined/") => {
                let Stage::Confirmed { token } = &self.stage else { return Response::text(404, "no such join\n") };
                if p != format!("/joined/{token}") { return Response::text(404, "no such join\n"); }
                let report = JoinReport {
                    hostname: form_field(body, "hostname").unwrap_or_default(),
                    host_key: form_field(body, "host_key").unwrap_or_default(),
                    arch: form_field(body, "arch").unwrap_or_default(),
                };
                self.stage = Stage::Joined { report, peer: peer.to_string() };
                Response::text(200, "ok\n")
            }
            _ => Response::text(404, "not found\n"),
        }
    }

    fn command_html(&self) -> String {
        format!(
            "<p>Open a terminal on this phone and run (long-press to copy):</p><textarea readonly>{c}</textarea>\
             <p>Before running it, check that <b>{f}</b> is what the laptop shows as the script fingerprint. \
             If it differs, do not run it.</p><p>It enables ssh, creates the <code>{u}</code> account (key-only), installs the laptop's key and reports back.</p>",
            c = esc(&self.phone_command().unwrap_or_default()),
            f = esc(&self.fingerprint().unwrap_or_default()),
            u = esc(&self.fleet_user),
        )
    }
}

fn pick_form(options: &[u16; 3]) -> String {
    let buttons: String = options.iter().map(|n| format!("<button class=n name=n value={n}>{n}</button>")).collect();
    format!("<p>Tap the number the laptop shows:</p><form method=post action=/pick>{buttons}</form>")
}

// ── addresses ──

/// IPv4 addresses from `ip -4 -o addr show` (loopback left out), with their interface:
/// the phone may reach the laptop over Wi-Fi OR the USB network link, and the default
/// route (`deploy::local_ipv4`) only names one of them.
pub fn parse_ip_addr(out: &str) -> Vec<(String, String)> {
    out.lines().filter_map(|l| {
        let mut w = l.split_whitespace();
        let _idx = w.next()?;
        let ifname = w.next()?.to_string();
        let fam = w.next()?;
        let cidr = w.next()?;
        let ip = cidr.split('/').next()?.to_string();
        (fam == "inet" && !ip.starts_with("127.")).then_some((ifname, ip))
    }).collect()
}

pub fn local_addresses() -> Vec<(String, String)> {
    std::process::Command::new("ip").args(["-4", "-o", "addr", "show"]).output()
        .map(|o| parse_ip_addr(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

// ── transport ──

fn urandom_u64() -> u64 {
    // The OS CSPRNG via crypto-kit (panics if there is none: tokens must not be guessable).
    let mut b = [0u8; 8];
    crypto_kit::random_bytes(&mut b);
    u64::from_le_bytes(b)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A running join: the session the GUI reads, the port, and a stop flag.
pub struct Running {
    pub session: Arc<Mutex<Session>>,
    pub port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Running {
    pub fn stop(&self) { self.stop.store(true, std::sync::atomic::Ordering::Relaxed); }
}

impl Drop for Running {
    fn drop(&mut self) { self.stop(); }
}

/// Parse one request from `stream`: method, path, body. Headers are read up to a blank
/// line; the body by Content-Length, capped at MAX_BODY.
fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, String, String)> {
    use std::io::{BufRead, BufReader, Read};
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 { break; }
        let h = h.trim_end();
        if h.is_empty() { break; }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") { len = v.trim().parse().unwrap_or(0); }
        }
    }
    if len > MAX_BODY { return None; }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some((method, path, String::from_utf8_lossy(&body).into_owned()))
}

/// Listen on every interface, on `port` (0 = any free port), for one join session.
/// `base_host` is the address the script calls back to (the one the operator's phone
/// uses; the GUI lists them all).
pub fn serve(port: u16, base_host: &str, fleet_user: &str, pubkey: &str) -> Result<Running, String> {
    let listener = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("join server: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let session = Arc::new(Mutex::new(Session::new(&format!("http://{base_host}:{port}"), fleet_user, pubkey, now_secs(), Box::new(urandom_u64))));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, st2) = (session.clone(), stop.clone());
    std::thread::spawn(move || {
        use std::io::Write;
        while !st2.load(std::sync::atomic::Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, addr)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                    let resp = match read_request(&mut stream) {
                        Some((m, p, b)) => s2.lock().map(|mut s| s.handle(&m, &p, &b, &addr.ip().to_string(), now_secs()))
                            .unwrap_or_else(|_| Response::text(500, "busy\n")),
                        None => Response::text(400, "bad request\n"),
                    };
                    let reason = match resp.status { 200 => "OK", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 409 => "Conflict", 410 => "Gone", _ => "Error" };
                    let _ = write!(stream, "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", resp.status, resp.content_type, resp.body.len(), resp.body);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        }
    });
    Ok(Running { session, port, stop })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(v: Vec<u64>) -> Box<dyn FnMut() -> u64 + Send> {
        let mut i = 0;
        Box::new(move || { let x = v[i % v.len()]; i += 1; x })
    }
    fn session() -> Session {
        Session::new("http://10.0.0.2:8099/", "fleet", "ssh-ed25519 AAAAkey me@laptop", 1000, seq(vec![123456, 17, 55, 42, 1, 0xdead, 0xbeef]))
    }
    fn login(s: &mut Session) -> Response {
        let code = s.code.code.clone();
        s.handle("POST", "/login", &format!("code={code}"), "10.0.0.9", 1001)
    }

    #[test]
    fn the_whole_flow_code_match_command_script_report() {
        let mut s = session();
        assert!(s.handle("GET", "/", "", "10.0.0.9", 1000).body.contains("name=code"));
        let r = login(&mut s);
        assert_eq!(r.status, 200);
        let n = s.laptop_number().expect("the laptop now shows a number");
        let Stage::Matching { options, .. } = s.stage.clone() else { panic!() };
        assert!(options.contains(&n) && options.iter().all(|o| (10..100).contains(o)));
        assert_eq!({ let mut o = options.to_vec(); o.sort(); o.dedup(); o.len() }, 3, "three distinct numbers");
        let r = s.handle("POST", "/pick", &format!("n={n}"), "10.0.0.9", 1002);
        assert_eq!(r.status, 200);
        let cmd = s.phone_command().unwrap();
        let fp = s.fingerprint().unwrap();
        assert!(r.body.contains(&fp), "the phone page shows the fingerprint to compare");
        let Stage::Confirmed { token } = s.stage.clone() else { panic!() };
        assert_eq!(token.len(), 32, "128-bit token");
        assert!(cmd.contains(&format!("http://10.0.0.2:8099/join/{token}")) && cmd.contains("sha256sum -c -"));
        // The script the token fetches is exactly what the command's hash names.
        let script = s.handle("GET", &format!("/join/{token}"), "", "10.0.0.9", 1003);
        assert_eq!(script.status, 200);
        assert!(cmd.contains(&sha256_hex(&script.body)), "command pins the served script");
        assert!(sha256_hex(&script.body).starts_with(&fp), "the laptop's fingerprint is this script's");
        assert!(script.body.contains(&format!("http://10.0.0.2:8099/joined/{token}")), "callback carries the token");
        assert!(script.body.contains("ssh-ed25519 AAAAkey me@laptop"));
        // A wrong token gets nothing.
        assert_eq!(s.handle("GET", "/join/00000000000000000000000000000000", "", "x", 1003).status, 404);
        // The report, with the phone's address as seen on the connection.
        let r = s.handle("POST", &format!("/joined/{token}"), "hostname=fp5&host_key=SHA256%3Aabc%2Bdef&arch=aarch64", "172.16.42.1", 1004);
        assert_eq!(r.status, 200);
        let Stage::Joined { report, peer } = &s.stage else { panic!("{:?}", s.stage) };
        assert_eq!((report.hostname.as_str(), report.host_key.as_str(), report.arch.as_str(), peer.as_str()), ("fp5", "SHA256:abc+def", "aarch64", "172.16.42.1"));
        // After joining, the script is no longer served.
        assert_eq!(s.handle("GET", &format!("/join/{token}"), "", "x", 1005).status, 404);
    }

    #[test]
    fn wrong_codes_and_a_wrong_pick_close_the_session() {
        let mut s = session();
        for left in [2, 1] {
            let r = s.handle("POST", "/login", "code=000000", "x", 1001);
            assert_eq!(r.status, 401);
            assert!(r.body.contains(&format!("{left} of 3")), "{}", r.body);
        }
        assert_eq!(s.handle("POST", "/login", "code=000000", "x", 1001).status, 403);
        assert!(matches!(s.stage, Stage::Closed(_)));
        assert_eq!(s.handle("POST", "/login", &format!("code={}", s.code.code.clone()), "x", 1001).status, 410, "the right code is useless once closed");
        // One wrong pick is final.
        let mut t = session();
        login(&mut t);
        let n = t.laptop_number().unwrap();
        let wrong = if n == 99 { 98 } else { n + 1 };
        assert_eq!(t.handle("POST", "/pick", &format!("n={wrong}"), "x", 1002).status, 403);
        assert!(matches!(t.stage, Stage::Closed(_)));
        assert_eq!(t.handle("POST", "/pick", &format!("n={n}"), "x", 1002).status, 410);
    }

    #[test]
    fn nothing_is_reachable_out_of_order_and_the_code_expires() {
        let mut s = session();
        assert_eq!(s.handle("POST", "/pick", "n=17", "x", 1001).status, 409, "no picking before the code");
        assert_eq!(s.handle("GET", "/join/anything", "", "x", 1001).status, 404, "no script before the match");
        assert_eq!(s.handle("POST", "/joined/anything", "", "x", 1001).status, 404);
        // Past login the code is spent, and the session must still be alive until expiry.
        let mut m = session();
        login(&mut m);
        assert_eq!(m.handle("GET", "/", "", "x", 1002).status, 200, "matching stage survives the code being used");
        let mut e = session();
        let late = 1000 + crate::join::CODE_TTL.as_secs() + 1;
        assert_eq!(e.handle("GET", "/", "", "x", late).status, 403);
        assert!(matches!(e.stage, Stage::Closed(ref w) if w.contains("expired")));
    }

    #[test]
    fn pages_escape_and_addresses_parse() {
        assert_eq!(esc("<a href='x'>&"), "&lt;a href=&#39;x&#39;&gt;&amp;");
        assert_eq!(form_field("a=1&code=12%203+4&b=2", "code").as_deref(), Some("12 3 4"));
        assert_eq!(form_field("code=%", "code").as_deref(), Some("%"), "a bare % is kept, not a panic");
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\n2: wlan0    inet 192.168.2.10/24 brd 192.168.2.255 scope global wlan0\n5: usb0    inet 172.16.42.2/24 scope global usb0\n3: wlan0    inet6 fe80::1/64 scope link\n";
        assert_eq!(parse_ip_addr(out), vec![("wlan0".into(), "192.168.2.10".into()), ("usb0".into(), "172.16.42.2".into())]);
    }

    #[test]
    fn served_over_a_real_socket_end_to_end() {
        // The transport, for real: bind, then walk the flow with raw HTTP.
        let run = serve(0, "127.0.0.1", "fleet", "ssh-ed25519 AAAAkey t").unwrap();
        let port = run.port;
        let req = |raw: String| -> String {
            use std::io::{Read, Write};
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            c.read_to_string(&mut out).unwrap();
            out
        };
        let post = |path: &str, body: &str| req(format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        assert!(req("GET / HTTP/1.1\r\nHost: x\r\n\r\n".into()).starts_with("HTTP/1.1 200 OK"));
        let code = run.session.lock().unwrap().code.code.clone();
        assert!(post("/login", &format!("code={code}")).starts_with("HTTP/1.1 200"));
        let n = run.session.lock().unwrap().laptop_number().unwrap();
        assert!(post("/pick", &format!("n={n}")).contains("sha256sum -c"));
        let token = match run.session.lock().unwrap().stage.clone() { Stage::Confirmed { token } => token, s => panic!("{s:?}") };
        let script = req(format!("GET /join/{token} HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(script.contains("gitmind-join-ok"));
        assert!(post(&format!("/joined/{token}"), "hostname=t&host_key=k&arch=x86_64").starts_with("HTTP/1.1 200"));
        assert!(matches!(run.session.lock().unwrap().stage, Stage::Joined { ref peer, .. } if peer == "127.0.0.1"));
        // An oversized body is refused without reaching the session.
        assert!(req(format!("POST /login HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1)).starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn the_phone_command_really_fetches_verifies_and_refuses_a_substitute() {
        // The exact command string the phone runs, with real curl and sha256sum; only the
        // final `sh` is swapped for an echo so nothing is executed here.
        let have = |c: &str| std::process::Command::new(c).arg("--version").output().is_ok();
        if !have("curl") || !have("sha256sum") { eprintln!("no curl/sha256sum; skipping"); return; }
        let run = serve(0, "127.0.0.1", "fleet", "ssh-ed25519 AAAAkey t").unwrap();
        let mut s = run.session.lock().unwrap();
        let code = s.code.code.clone();
        s.handle("POST", "/login", &format!("code={code}"), "x", now_secs());
        let n = s.laptop_number().unwrap();
        s.handle("POST", "/pick", &format!("n={n}"), "x", now_secs());
        let cmd = s.phone_command().unwrap();
        drop(s);
        let dry = cmd.replace("&& sh /tmp/gitmind-join.sh", "&& echo VERIFIED");
        let out = std::process::Command::new("sh").arg("-c").arg(&dry).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("VERIFIED"), "{:?}", out);
        // A script that is not the one the laptop pinned is refused before it runs.
        let full = cmd.split('\'').nth(1).unwrap().split_whitespace().next().unwrap().to_string();
        let tampered = dry.replace(&full, &"0".repeat(64));
        let out = std::process::Command::new("sh").arg("-c").arg(&tampered).output().unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains("VERIFIED"), "a hash mismatch must stop it");
    }
}
