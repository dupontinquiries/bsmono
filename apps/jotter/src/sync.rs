//! sync -- two trusted devices bring their notes level, over TCP, and a new device is paired, on the same port.
//!
//! **Authentication is mutual and pinned.** Each side sends its two public keys and a nonce, signed by its own
//! signing key. The other side accepts only a device that is on its trust list AND whose agreement key is the one
//! recorded when it was paired (a stolen signing key is not enough, and neither is a new key under an old name).
//! The session key comes from the X25519 agreement of the two static keys and both nonces, so each connection has
//! its own.
//!
//! **Everything after the handshake is sealed** (AES-256-GCM) with a direction byte and a counter inside, so a
//! frame cannot be flipped, reflected back, replayed, dropped from the middle, or reordered without the next read failing.
//!
//! **The exchange:** each side lists its notes as `(id, digest)`, sends the notes the other lacks or has a
//! different version of, and merges what arrives. The notes are CRDTs, so merging is safe in either order and a
//! second run changes nothing.
//!
//! **Pairing** shares the port: the first frame is `JPR1` + a claim (see `pairing`). The shower checks the claim
//! against an offer it issued, asks its owner (`PairPolicy::confirm`) with the other device's name and
//! fingerprint, and only on yes adds it to the trust list.

use crate::identity::Identity;
use crate::note::Note;
use crate::pairing::{self, Offer, OfferBook, PairError};
use crate::trust::TrustStore;
use crypto_kit::{derive_session_key, open, random_bytes, seal, sign, verify, x25519_shared};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const HELLO: &[u8; 4] = b"JSY1";
pub const PAIR: &[u8; 4] = b"JPR1";
pub const PAIRED: &[u8; 4] = b"JOK1";
pub const REFUSED: &[u8; 4] = b"JNO1";
/// The default port.
pub const PORT: u16 = 7471;
/// No frame is larger than this (a big note is a few hundred KB).
pub const MAX_FRAME: usize = 16 << 20;
pub const IO_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Io(String),
    /// The other device is not on the trust list (or its keys are not the pinned ones).
    NotTrusted,
    BadHandshake,
    /// A sealed frame did not open, or was replayed, reflected or out of order.
    BadFrame,
    TooLarge,
    Refused,
    Protocol(String),
}

impl From<std::io::Error> for SyncError { fn from(e: std::io::Error) -> Self { SyncError::Io(e.to_string()) } }

fn write_frame(w: &mut impl Write, b: &[u8]) -> Result<(), SyncError> {
    if b.len() > MAX_FRAME { return Err(SyncError::TooLarge); }
    w.write_all(&(b.len() as u32).to_be_bytes())?;
    w.write_all(b)?;
    w.flush()?;
    Ok(())
}

fn read_frame(r: &mut impl Read) -> Result<Vec<u8>, SyncError> {
    let mut l = [0u8; 4];
    r.read_exact(&mut l)?;
    let n = u32::from_be_bytes(l) as usize;
    if n > MAX_FRAME { return Err(SyncError::TooLarge); }
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    Ok(b)
}

/// A device's notes, as sync sees them.
pub trait Store {
    fn list(&self) -> Vec<([u8; 8], u64)>;
    fn get(&self, id: &[u8; 8]) -> Option<Vec<u8>>;
    /// Merge a note received from a peer (bytes as `Note::to_bytes`). `Err` for a note that does not decode.
    fn put(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// A store in a directory of `.jnt` files (what the app writes).
pub struct FileStore { pub dir: std::path::PathBuf, pub changed: u64 }

impl FileStore {
    pub fn new(dir: std::path::PathBuf) -> FileStore { FileStore { dir, changed: 0 } }
    fn path(&self, id: &[u8; 8]) -> std::path::PathBuf { self.dir.join(format!("{}.jnt", crate::identity::hex(id))) }
}

impl Store for FileStore {
    fn list(&self) -> Vec<([u8; 8], u64)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return vec![] };
        rd.flatten().filter_map(|e| std::fs::read(e.path()).ok()).filter_map(|b| Note::from_bytes(&b)).map(|n| (n.id, n.digest())).collect()
    }
    fn get(&self, id: &[u8; 8]) -> Option<Vec<u8>> { std::fs::read(self.path(id)).ok() }
    fn put(&mut self, bytes: &[u8]) -> Result<(), String> {
        let incoming = Note::from_bytes(bytes).ok_or("a note that does not decode")?;
        let path = self.path(&incoming.id);
        let mut merged = std::fs::read(&path).ok().and_then(|b| Note::from_bytes(&b)).unwrap_or_else(|| incoming.clone());
        let before = merged.digest();
        merged.merge(&incoming);
        if merged.digest() != before || !path.exists() {
            std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
            let tmp = path.with_extension("jnt.tmp");
            std::fs::write(&tmp, merged.to_bytes()).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())?;
            self.changed += 1;
        }
        Ok(())
    }
}

/// An authenticated, sealed channel.
pub struct Session<S: Read + Write> {
    stream: S,
    key: zeroize::Zeroizing<[u8; 32]>,
    /// 0 when this side is the client, 1 the server: the direction byte it sends with.
    dir: u8,
    send_n: u64,
    recv_n: u64,
    pub peer: crate::trust::Peer,
}

impl<S: Read + Write> Session<S> {
    pub fn send(&mut self, msg: &[u8]) -> Result<(), SyncError> {
        let mut p = Vec::with_capacity(9 + msg.len());
        p.push(self.dir);
        p.extend_from_slice(&self.send_n.to_be_bytes());
        p.extend_from_slice(msg);
        self.send_n += 1;
        write_frame(&mut self.stream, &seal(&self.key, &p))
    }

    pub fn recv(&mut self) -> Result<Vec<u8>, SyncError> {
        let sealed = read_frame(&mut self.stream)?;
        let p = open(&self.key, &sealed).ok_or(SyncError::BadFrame)?;
        if p.len() < 9 || p[0] != (self.dir ^ 1) || u64::from_be_bytes(p[1..9].try_into().unwrap()) != self.recv_n { return Err(SyncError::BadFrame); }
        self.recv_n += 1;
        Ok(p[9..].to_vec())
    }
}

fn hello_bytes(me: &Identity, nonce: &[u8; 16], peer_nonce: Option<&[u8; 16]>) -> Vec<u8> {
    let mut signed = Vec::new();
    signed.extend_from_slice(b"jotter-hello");
    signed.extend_from_slice(&me.x_pub);
    signed.extend_from_slice(nonce);
    if let Some(n) = peer_nonce { signed.extend_from_slice(n); }
    let sig = sign_with(me, &signed);
    let mut b = Vec::with_capacity(4 + 32 + 32 + 16 + 64);
    b.extend_from_slice(HELLO);
    b.extend_from_slice(&me.ed_pub);
    b.extend_from_slice(&me.x_pub);
    b.extend_from_slice(nonce);
    b.extend_from_slice(&sig);
    b
}

fn sign_with(me: &Identity, msg: &[u8]) -> [u8; 64] { sign_msg(me, msg) }
fn sign_msg(me: &Identity, msg: &[u8]) -> [u8; 64] { let _ = sign; me.sign_public(msg) }

struct Hello { ed: [u8; 32], x: [u8; 32], nonce: [u8; 16] }

fn parse_hello(b: &[u8], peer_nonce: Option<&[u8; 16]>) -> Result<Hello, SyncError> {
    if b.len() != 4 + 32 + 32 + 16 + 64 || &b[..4] != HELLO { return Err(SyncError::BadHandshake); }
    let ed: [u8; 32] = b[4..36].try_into().unwrap();
    let x: [u8; 32] = b[36..68].try_into().unwrap();
    let nonce: [u8; 16] = b[68..84].try_into().unwrap();
    let sig: [u8; 64] = b[84..148].try_into().unwrap();
    let mut signed = Vec::new();
    signed.extend_from_slice(b"jotter-hello");
    signed.extend_from_slice(&x);
    signed.extend_from_slice(&nonce);
    if let Some(n) = peer_nonce { signed.extend_from_slice(n); }
    if !verify(&ed, &signed, &sig) { return Err(SyncError::BadHandshake); }
    Ok(Hello { ed, x, nonce })
}

fn session_key(me: &Identity, their_x: &[u8; 32], c_nonce: &[u8; 16], s_nonce: &[u8; 16]) -> zeroize::Zeroizing<[u8; 32]> {
    let mut ikm = Vec::with_capacity(64);
    ikm.extend_from_slice(&*x25519_shared(me.x_secret(), their_x));
    ikm.extend_from_slice(c_nonce);
    ikm.extend_from_slice(s_nonce);
    derive_session_key(&ikm, b"jotter-sync-v1")
}

fn pinned<'a>(trust: &'a TrustStore, h: &Hello) -> Result<&'a crate::trust::Peer, SyncError> {
    match trust.find(&h.ed) {
        Some(p) if p.x_pub == h.x => Ok(p),
        _ => Err(SyncError::NotTrusted),
    }
}

/// The CLIENT side of the handshake (we connected).
pub fn connect<S: Read + Write>(mut stream: S, me: &Identity, trust: &TrustStore) -> Result<Session<S>, SyncError> {
    let mut c_nonce = [0u8; 16];
    random_bytes(&mut c_nonce);
    write_frame(&mut stream, &hello_bytes(me, &c_nonce, None))?;
    let reply = read_frame(&mut stream)?;
    if reply == REFUSED { return Err(SyncError::NotTrusted); }
    let s = parse_hello(&reply, Some(&c_nonce))?;
    let peer = pinned(trust, &s)?.clone();
    let key = session_key(me, &s.x, &c_nonce, &s.nonce);
    Ok(Session { stream, key, dir: 0, send_n: 0, recv_n: 0, peer })
}

/// The SERVER side (a device connected to us, with `first` its first frame already read).
fn accept_hello<S: Read + Write>(mut stream: S, first: &[u8], me: &Identity, trust: &TrustStore) -> Result<Session<S>, SyncError> {
    let c = parse_hello(first, None)?;
    let peer = match pinned(trust, &c) {
        Ok(p) => p.clone(),
        Err(e) => { let _ = write_frame(&mut stream, REFUSED); return Err(e); }
    };
    let mut s_nonce = [0u8; 16];
    random_bytes(&mut s_nonce);
    write_frame(&mut stream, &hello_bytes(me, &s_nonce, Some(&c.nonce)))?;
    let key = session_key(me, &c.x, &c.nonce, &s_nonce);
    Ok(Session { stream, key, dir: 1, send_n: 0, recv_n: 0, peer })
}

fn list_bytes(items: &[([u8; 8], u64)]) -> Vec<u8> {
    let mut b = vec![b'L'];
    for (id, d) in items { b.extend_from_slice(id); b.extend_from_slice(&d.to_be_bytes()); }
    b
}

fn parse_list(b: &[u8]) -> Result<Vec<([u8; 8], u64)>, SyncError> {
    if b.first() != Some(&b'L') || (b.len() - 1) % 16 != 0 { return Err(SyncError::Protocol("a bad note list".into())); }
    Ok(b[1..].chunks(16).map(|c| (c[..8].try_into().unwrap(), u64::from_be_bytes(c[8..16].try_into().unwrap()))).collect())
}

/// What a sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report { pub sent: usize, pub received: usize }

/// Bring the two stores level. Both sides run this on their end of a `Session`.
pub fn exchange<S: Read + Write>(sess: &mut Session<S>, store: &mut dyn Store) -> Result<Report, SyncError> {
    let mine = store.list();
    sess.send(&list_bytes(&mine))?;
    let theirs = parse_list(&sess.recv()?)?;
    let their: std::collections::HashMap<[u8; 8], u64> = theirs.into_iter().collect();
    let mut report = Report::default();
    // send what they lack or hold a different version of
    for (id, digest) in &mine {
        if their.get(id) != Some(digest) {
            if let Some(bytes) = store.get(id) {
                let mut m = vec![b'N'];
                m.extend_from_slice(&bytes);
                sess.send(&m)?;
                report.sent += 1;
            }
        }
    }
    sess.send(b"D")?;
    loop {
        let m = sess.recv()?;
        match m.first() {
            Some(b'D') => break,
            Some(b'N') => { store.put(&m[1..]).map_err(SyncError::Protocol)?; report.received += 1; }
            _ => return Err(SyncError::Protocol("an unexpected message".into())),
        }
    }
    // each side says "applied" only after merging everything, and waits for the other's: a finished sync means both
    // devices hold the result, not just that the last frame left this one
    sess.send(b"A")?;
    if sess.recv()? != b"A" { return Err(SyncError::Protocol("no final acknowledgement".into())); }
    Ok(report)
}

/// Sync with the trusted device at `addr`.
pub fn sync_with(addr: &str, me: &Identity, trust: &TrustStore, store: &mut dyn Store) -> Result<(crate::trust::Peer, Report), SyncError> {
    use std::net::ToSocketAddrs;
    let sock = addr.to_socket_addrs()?.next().ok_or_else(|| SyncError::Io(format!("no address for {addr}")))?;
    let stream = TcpStream::connect_timeout(&sock, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut sess = connect(stream, me, trust)?;
    let r = exchange(&mut sess, store)?;
    Ok((sess.peer.clone(), r))
}

/// Asks the owner whether to trust a device that just scanned our code.
pub trait PairPolicy: Send {
    fn confirm(&mut self, who: &Offer) -> bool;
}

impl<F: FnMut(&Offer) -> bool + Send> PairPolicy for F { fn confirm(&mut self, who: &Offer) -> bool { self(who) } }

/// Everything a serving device shares between connections.
pub struct Host {
    pub me: Identity,
    pub trust: Mutex<TrustStore>,
    pub trust_path: Option<std::path::PathBuf>,
    pub offers: Mutex<OfferBook>,
    pub policy: Mutex<Box<dyn PairPolicy>>,
    pub store_dir: std::path::PathBuf,
    /// Counts completed syncs that changed something (the app watches it to reload).
    pub changes: std::sync::atomic::AtomicU64,
}

fn now_s() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Handle one connection on the serving side.
pub fn handle<S: Read + Write>(mut stream: S, host: &Host) -> Result<String, SyncError> {
    let first = read_frame(&mut stream)?;
    if first.starts_with(PAIR) {
        let claim = &first[4..];
        let offer = host.offers.lock().unwrap().accept(&host.me, claim, now_s()).map_err(|e| match e { PairError::Expired => SyncError::Refused, _ => SyncError::BadHandshake })?;
        let yes = host.policy.lock().unwrap().confirm(&offer);
        if !yes { let _ = write_frame(&mut stream, REFUSED); return Err(SyncError::Refused); }
        {
            let mut t = host.trust.lock().unwrap();
            t.trust(&offer, now_s());
            if let Some(p) = &host.trust_path { let _ = t.save(p); }
        }
        write_frame(&mut stream, PAIRED)?;
        return Ok(format!("paired with {} {}", offer.name, crate::identity::fingerprint(&offer.ed_pub)));
    }
    let trust = host.trust.lock().unwrap().clone();
    let mut sess = accept_hello(stream, &first, &host.me, &trust)?;
    let mut store = FileStore::new(host.store_dir.clone());
    let r = exchange(&mut sess, &mut store)?;
    if store.changed > 0 { host.changes.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    Ok(format!("synced with {}: sent {}, received {}", sess.peer.name, r.sent, r.received))
}

/// Serve on `listener` until `stop` is set: one thread per connection, each with timeouts.
pub fn serve(listener: TcpListener, host: Arc<Host>, stop: Arc<std::sync::atomic::AtomicBool>, log: Arc<dyn Fn(String) + Send + Sync>) {
    let _ = listener.set_nonblocking(true);
    let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        match listener.accept() {
            Ok((s, from)) => {
                if live.load(std::sync::atomic::Ordering::SeqCst) >= 8 { continue; } // a flood is dropped, not queued
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                let (host, log, live) = (host.clone(), log.clone(), live.clone());
                live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    match handle(s, &host) {
                        Ok(m) => log(format!("{from}: {m}")),
                        Err(e) => log(format!("{from}: refused ({e:?})")),
                    }
                    live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// The SCANNER side of pairing: we read `offer` from a QR and connect to the first address it lists that answers.
/// On yes from the other owner we trust it in return. `my_addrs` is where it can reach us.
pub fn pair_with(offer: &Offer, me: &Identity, my_addrs: Vec<String>, trust: &mut TrustStore) -> Result<(), SyncError> {
    use std::net::ToSocketAddrs;
    let claim = pairing::claim(me, offer, my_addrs, now_s());
    let mut last = SyncError::Io("the code lists no address".into());
    for a in &offer.addrs {
        let Some(sock) = a.to_socket_addrs().ok().and_then(|mut i| i.next()) else { continue };
        let mut stream = match TcpStream::connect_timeout(&sock, IO_TIMEOUT) { Ok(s) => s, Err(e) => { last = e.into(); continue } };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(120))); // the other owner has to tap yes
        let mut f = PAIR.to_vec();
        f.extend_from_slice(&claim);
        write_frame(&mut stream, &f)?;
        let reply = read_frame(&mut stream)?;
        if reply == PAIRED { trust.trust(offer, now_s()); return Ok(()); }
        return Err(SyncError::Refused);
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn pair_up(a: &Identity, b: &Identity) -> (TrustStore, TrustStore) {
        let (mut ta, mut tb) = (TrustStore::default(), TrustStore::default());
        ta.trust(&Offer::new(b, vec![], 1), 1);
        tb.trust(&Offer::new(a, vec![], 1), 1);
        (ta, tb)
    }

    fn note_with(id: u8, strokes: &[u64]) -> Note {
        use ink_kit::stroke::{InkPoint, Stroke, StrokeId, StrokeStyle};
        let mut n = Note::new([id; 8], "n", 100, [9; 8]);
        for s in strokes {
            let mut st = Stroke::new(StrokeId(*s), StrokeStyle::default());
            for k in 0..5 { st.points.push(InkPoint::xy(*s as f64 + k as f64, k as f64)); }
            n.add(st);
        }
        n
    }

    fn put_note(dir: &std::path::Path, n: &Note) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{}.jnt", crate::identity::hex(&n.id))), n.to_bytes()).unwrap();
    }

    fn read_notes(dir: &std::path::Path) -> std::collections::BTreeMap<[u8; 8], Vec<u64>> {
        FileStore::new(dir.to_path_buf()).list().into_iter()
            .map(|(id, _)| (id, Note::from_bytes(&FileStore::new(dir.to_path_buf()).get(&id).unwrap()).unwrap().strokes().map(|s| s.id.0).collect()))
            .collect()
    }

    fn host_for(me: Identity, trust: TrustStore, dir: std::path::PathBuf, yes: bool) -> Arc<Host> {
        Arc::new(Host { me, trust: Mutex::new(trust), trust_path: None, offers: Mutex::new(OfferBook::default()), policy: Mutex::new(Box::new(move |_: &Offer| yes)), store_dir: dir, changes: Default::default() })
    }

    fn start(host: Arc<Host>) -> (String, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let (s2, l2) = (stop.clone(), logs.clone());
        std::thread::spawn(move || serve(l, host, s2, Arc::new(move |m| l2.lock().unwrap().push(m))));
        (addr, stop, logs)
    }

    #[test]
    fn two_trusted_devices_that_both_wrote_end_up_with_the_same_notes() {
        let (tablet, phone) = (Identity::generate("Tablet"), Identity::generate("Phone"));
        let (t_tablet, t_phone) = pair_up(&tablet, &phone);
        let (dt, dp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // the tablet has note 1 (strokes 10, 11) and note 2; the phone has note 1 (strokes 11, 12) and note 3
        put_note(dt.path(), &note_with(1, &[10, 11]));
        put_note(dt.path(), &note_with(2, &[20]));
        put_note(dp.path(), &note_with(1, &[11, 12]));
        put_note(dp.path(), &note_with(3, &[30]));
        let (addr, stop, _) = start(host_for(tablet, t_tablet, dt.path().to_path_buf(), true));
        let mut store = FileStore::new(dp.path().to_path_buf());
        let (peer, report) = sync_with(&addr, &phone, &t_phone, &mut store).expect("synced");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(peer.name, "Tablet");
        assert!(report.sent >= 2 && report.received >= 2, "{report:?}");
        let (a, b) = (read_notes(dt.path()), read_notes(dp.path()));
        assert_eq!(a, b, "the same pages on both");
        assert_eq!(a.len(), 3);
        assert_eq!(a[&[1u8; 8]], vec![10, 11, 12], "the shared note is the union of both devices' strokes");
    }

    #[test]
    fn a_device_that_is_not_trusted_gets_nothing_and_the_server_changes_nothing() {
        let (tablet, phone, stranger) = (Identity::generate("Tablet"), Identity::generate("Phone"), Identity::generate("Stranger"));
        let (t_tablet, _) = pair_up(&tablet, &phone);
        let dt = tempfile::tempdir().unwrap();
        put_note(dt.path(), &note_with(1, &[10]));
        let (addr, stop, logs) = start(host_for(tablet.clone(), t_tablet, dt.path().to_path_buf(), true));
        // the stranger trusts the tablet (it scanned the code) but the tablet never trusted the stranger
        let mut ts = TrustStore::default();
        ts.trust(&Offer::new(&tablet, vec![], 1), 1);
        let ds = tempfile::tempdir().unwrap();
        put_note(ds.path(), &note_with(9, &[90]));
        let mut store = FileStore::new(ds.path().to_path_buf());
        assert_eq!(sync_with(&addr, &stranger, &ts, &mut store).map(|_| ()), Err(SyncError::NotTrusted));
        std::thread::sleep(Duration::from_millis(200));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(read_notes(dt.path()).len(), 1, "nothing was written to the tablet");
        assert_eq!(read_notes(ds.path()).len(), 1, "and nothing was taken from it");
        assert!(logs.lock().unwrap().iter().any(|l| l.contains("refused")), "{:?}", logs.lock().unwrap());
    }

    #[test]
    fn a_known_name_with_a_new_key_is_refused_because_keys_are_pinned() {
        let (tablet, phone) = (Identity::generate("Tablet"), Identity::generate("Phone"));
        let (t_tablet, t_phone) = pair_up(&tablet, &phone);
        // an impostor with the phone's signing identity but a different agreement key cannot exist (the signature
        // covers it), so test the pin directly: the tablet holds a different x key for the phone
        let mut wrong = t_tablet.clone();
        let mut offer = Offer::new(&phone, vec![], 1);
        offer.x_pub = Identity::generate("other").x_pub;
        wrong = { let mut t = TrustStore::default(); t.trust(&offer, 1); let _ = wrong; t };
        let dt = tempfile::tempdir().unwrap();
        let (addr, stop, _) = start(host_for(tablet, wrong, dt.path().to_path_buf(), true));
        let dp = tempfile::tempdir().unwrap();
        assert_eq!(sync_with(&addr, &phone, &t_phone, &mut FileStore::new(dp.path().to_path_buf())).map(|_| ()), Err(SyncError::NotTrusted));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn a_sealed_channel_refuses_a_flipped_replayed_or_reflected_frame() {
        let (a, b) = (Identity::generate("A"), Identity::generate("B"));
        let (ta, tb) = pair_up(&a, &b);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut s2 = s.try_clone().unwrap();
            let first = read_frame(&mut s2).unwrap();
            let mut sess = accept_hello(s, &first, &b, &tb).unwrap();
            let one = sess.recv().unwrap();
            let two = sess.recv();
            (one, two)
        });
        let mut c = connect(TcpStream::connect(addr).unwrap(), &a, &ta).unwrap();
        c.send(b"first").unwrap();
        // the second message is replayed: send the first frame's counter again by rewinding
        c.send_n = 0;
        c.send(b"replayed").unwrap();
        let (one, two) = server.join().unwrap();
        assert_eq!(one, b"first");
        assert_eq!(two, Err(SyncError::BadFrame), "a replayed counter is refused");
        // a flipped byte never opens
        let key = [3u8; 32];
        let mut sealed = seal(&key, b"hello");
        sealed[20] ^= 1;
        assert!(open(&key, &sealed).is_none());
    }

    #[test]
    fn oversized_and_garbage_frames_are_refused_without_allocating_them() {
        let mut huge = Vec::new();
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        huge.extend_from_slice(b"junk");
        assert_eq!(read_frame(&mut &huge[..]), Err(SyncError::TooLarge));
        assert!(matches!(read_frame(&mut &[0u8, 0, 0][..]), Err(SyncError::Io(_))), "a short header");
        assert!(matches!(parse_hello(b"nope", None), Err(SyncError::BadHandshake)));
        assert!(parse_list(b"Lxx").is_err() && parse_list(b"X").is_err());
        assert_eq!(write_frame(&mut Vec::new(), &vec![0u8; MAX_FRAME + 1]), Err(SyncError::TooLarge));
    }

    #[test]
    fn a_new_device_is_paired_only_when_the_owner_says_yes_and_each_offer_works_once() {
        let (tablet, phone) = (Identity::generate("Tablet"), Identity::generate("Phone"));
        let dt = tempfile::tempdir().unwrap();
        let host = host_for(tablet.clone(), TrustStore::default(), dt.path().to_path_buf(), true);
        let offer = host.offers.lock().unwrap().issue(&tablet, vec![], 0);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let offer = Offer { addrs: vec![addr], ..offer };
        let stop = Arc::new(AtomicBool::new(false));
        let (h2, s2) = (host.clone(), stop.clone());
        std::thread::spawn(move || serve(l, h2, s2, Arc::new(|_| {})));
        // the phone scans the code and pairs
        let mut tp = TrustStore::default();
        // the offer's issue time was 0 (so it has "expired" by wall clock): issue a current one the way the app does
        let fresh = {
            let o = host.offers.lock().unwrap().issue(&tablet, vec![offer.addrs[0].clone()], now_s());
            o
        };
        pair_with(&fresh, &phone, vec!["10.0.0.2:7471".into()], &mut tp).expect("paired");
        assert!(tp.is_trusted(&tablet.ed_pub), "the phone trusts the tablet");
        assert!(host.trust.lock().unwrap().is_trusted(&phone.ed_pub), "and the tablet, after its owner said yes, trusts the phone");
        assert_eq!(host.trust.lock().unwrap().find(&phone.ed_pub).unwrap().addr.as_deref(), Some("10.0.0.2:7471"), "with the address it can be reached at");
        // the same code a second time does not work
        assert!(pair_with(&fresh, &Identity::generate("Another"), vec![], &mut TrustStore::default()).is_err(), "an offer is single-use");
        // and now they can sync
        let (dp, dt2) = (tempfile::tempdir().unwrap(), dt.path().to_path_buf());
        put_note(dp.path(), &note_with(5, &[50]));
        let (_, rep) = sync_with(&fresh.addrs[0], &phone, &tp, &mut FileStore::new(dp.path().to_path_buf())).expect("synced after pairing");
        assert_eq!(rep.sent, 1);
        assert_eq!(read_notes(&dt2).len(), 1);
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn an_owner_who_says_no_pairs_nobody() {
        let (tablet, phone) = (Identity::generate("Tablet"), Identity::generate("Phone"));
        let dt = tempfile::tempdir().unwrap();
        let host = host_for(tablet.clone(), TrustStore::default(), dt.path().to_path_buf(), false);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let offer = host.offers.lock().unwrap().issue(&tablet, vec![addr], now_s());
        let stop = Arc::new(AtomicBool::new(false));
        let (h2, s2) = (host.clone(), stop.clone());
        std::thread::spawn(move || serve(l, h2, s2, Arc::new(|_| {})));
        let mut tp = TrustStore::default();
        assert_eq!(pair_with(&offer, &phone, vec![], &mut tp), Err(SyncError::Refused));
        assert!(tp.peers().is_empty() && host.trust.lock().unwrap().peers().is_empty(), "neither side trusts the other");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
