//! jotter -- the headless side of the app, for a laptop or a phone's shell: the same data directory as the
//! writing app (`~/.local/share/jotter`: `identity`, `trusted`, `notes/*.jnt`), driven from a terminal.
//!
//! What it does, each a thin shell over the library: show this device's PAIRING code (as half-block text for a
//! terminal, or a PNG) and optionally listen for the device that scans it; SCAN a code out of a photo or a
//! screenshot -- a pairing offer, or a note's mega-QR mosaic -- and finish the pairing or merge the note; show a
//! NOTE as its frames (text) or as the MOSAIC the tablet would draw (PNG, one file a screen); list notes and trusted
//! devices; SYNC with a trusted device. Nothing here trusts a device without a yes typed at the prompt.
//!
//! Reading an image needs the `scan` feature (on by default; it is what pulls in `image` and `rqrr`). A build without
//! it still shows codes as text and syncs; `scan` and `--png` say so and exit 2.

use jotter::identity::{self, Identity};
use jotter::note::Note;
use jotter::pairing::{Offer, OfferBook};
use jotter::share::{self, Cell, Kind, Matrix};
use jotter::sync::{self, FileStore, Store};
use jotter::trust::TrustStore;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
usage: jotter [--dir DIR] [--name NAME] COMMAND ...

  id                                  this device: name, fingerprint, id, data directory (made on first use)
  devices                             the devices this one trusts
  notes                               the notes in the data directory
  pair [--addr HOST:PORT]... [--png FILE] [--module PX] [--serve] [--port N]
                                      show a pairing code; --serve then listens for the device that scans it
                                      and asks before trusting it (Ctrl-C to stop)
  scan IMAGE... [--pair] [--save] [--addr HOST:PORT]...
                                      read the code(s) in photos or screenshots: an offer is shown (--pair:
                                      claim it over the network and trust the device on its owner's yes);
                                      a note is shown (--save: merge it into the notes directory)
  show NOTE [--png FILE] [--w PX] [--h PX] [--screen N]
                                      a note (a .jnt path, or an id prefix from the notes directory) as QR
                                      frames in the terminal, or as the mosaic screens a tablet shows (PNG,
                                      FILE-N.png per screen when there are several; default 1620x2160)
  serve [--port N]                    listen for trusted devices to sync (default port 7471)
  sync HOST:PORT                      bring notes level with a trusted device

  --dir DIR    the data directory ($XDG_DATA_HOME/jotter or ~/.local/share/jotter)
  --name NAME  this device's name when its identity is first made (default: the hostname)
";

/// Flags that take a value. Fixed, so `--serve` never swallows the word after it and `--png=x` and `--png x` agree.
const VALUED: &[&str] = &["dir", "name", "addr", "png", "module", "w", "h", "screen", "port"];
const BARE: &[&str] = &["serve", "pair", "save", "help"];

struct Opts { pos: Vec<String>, flags: Vec<(String, String)> }

fn parse(args: impl IntoIterator<Item = String>) -> Result<Opts, String> {
    let mut o = Opts { pos: vec![], flags: vec![] };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let Some(f) = a.strip_prefix("--") else { o.pos.push(a); continue };
        if f.is_empty() { o.pos.extend(it.by_ref()); break; } // `--`: the rest are names, even ones starting with --
        let (k, inline) = match f.split_once('=') { Some((k, v)) => (k.to_string(), Some(v.to_string())), None => (f.to_string(), None) };
        let v = if VALUED.contains(&k.as_str()) {
            match inline { Some(v) => v, None => it.next().ok_or_else(|| format!("--{k} needs a value"))? }
        } else if BARE.contains(&k.as_str()) {
            if inline.is_some() { return Err(format!("--{k} takes no value")); }
            String::new()
        } else {
            return Err(format!("unknown option --{k}"));
        };
        o.flags.push((k, v));
    }
    Ok(o)
}

impl Opts {
    fn has(&self, k: &str) -> bool { self.flags.iter().any(|(f, _)| f == k) }
    /// The last `--k`, so a later one on the line wins.
    fn get(&self, k: &str) -> Option<&str> { self.flags.iter().rev().find(|(f, _)| f == k).map(|(_, v)| v.as_str()) }
    fn all(&self, k: &str) -> Vec<String> { self.flags.iter().filter(|(f, _)| f == k).map(|(_, v)| v.clone()).collect() }
    fn num(&self, k: &str, default: u32) -> Result<u32, String> {
        match self.get(k) { None => Ok(default), Some(v) => v.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| format!("--{k} wants a positive number, not {v:?}")) }
    }
}

#[derive(Debug)]
enum Fail { Usage(String), Run(String) }
impl From<String> for Fail { fn from(m: String) -> Fail { Fail::Run(m) } }
fn usage(m: impl Into<String>) -> Fail { Fail::Usage(m.into()) }

fn main() {
    let code = match run(std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => 0,
        Err(Fail::Usage(m)) => { eprintln!("jotter: {m}\n\n{USAGE}"); 2 }
        Err(Fail::Run(m)) => { eprintln!("jotter: {m}"); 1 }
    };
    std::process::exit(code);
}

fn run(args: Vec<String>) -> Result<(), Fail> {
    let o = parse(args).map_err(usage)?;
    if o.has("help") || o.pos.is_empty() { println!("{USAGE}"); return Ok(()); }
    let dir = data_dir(&o)?;
    match o.pos[0].as_str() {
        "id" => cmd_id(&o, &dir),
        "devices" => cmd_devices(&dir),
        "notes" => cmd_notes(&dir),
        "pair" => cmd_pair(&o, &dir),
        "scan" => cmd_scan(&o, &dir),
        "show" => cmd_show(&o, &dir),
        "serve" => cmd_serve(&o, &dir),
        "sync" => cmd_sync(&o, &dir),
        other => Err(usage(format!("unknown command {other:?}"))),
    }
}

// ---- the data directory and who we are ------------------------------------------------------------

fn data_dir(o: &Opts) -> Result<PathBuf, Fail> {
    match o.get("dir") {
        Some(d) => Ok(PathBuf::from(d)),
        None => Identity::default_path().and_then(|p| p.parent().map(Path::to_path_buf)).ok_or_else(|| usage("no HOME or XDG_DATA_HOME: pass --dir")),
    }
}

/// This device's identity, made on first use (which is why the read-only commands do not call this).
fn me(o: &Opts, dir: &Path) -> Result<Identity, Fail> {
    let name = o.get("name").map(str::to_string).or_else(hostname).unwrap_or_else(|| "laptop".into());
    Identity::load_or_create(&dir.join("identity"), &name).map_err(|e| Fail::Run(format!("{}: {e}", dir.join("identity").display())))
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
}

fn now_s() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// The address another device on the LAN would reach us at: the interface the default route uses, found by
/// CONNECTING a UDP socket (no packet is sent; connect only picks the route). `None` when there is no route.
fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET-1: never answers, never needs to
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

/// `--addr` values, else our routed address on `port`, else none (the code then carries no address, and a
/// scanner can only read who we are).
fn our_addrs(o: &Opts, port: u16) -> Vec<String> {
    let given = o.all("addr");
    if !given.is_empty() { return given; }
    local_ip().map(|ip| vec![format!("{ip}:{port}")]).unwrap_or_default()
}

fn port(o: &Opts) -> Result<u16, Fail> {
    let p = o.num("port", sync::PORT as u32).map_err(usage)?;
    u16::try_from(p).map_err(|_| usage(format!("--port {p} is not a port")))
}

// ---- id / devices / notes ---------------------------------------------------------------------

fn cmd_id(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let me = me(o, dir)?;
    println!("name         {}\nfingerprint  {}\nid           {}\ndata         {}", me.name, me.fingerprint(), me.id(), dir.display());
    Ok(())
}

fn cmd_devices(dir: &Path) -> Result<(), Fail> {
    let t = TrustStore::load(&dir.join("trusted"));
    if t.peers().is_empty() { println!("no trusted devices (pair one: `jotter pair --serve` here, `jotter scan` there)"); }
    for p in t.peers() {
        println!("{}  {}  {:<32}  {}", p.id(), p.fingerprint(), p.name, p.addr.as_deref().unwrap_or("-"));
    }
    Ok(())
}

fn notes_in(dir: &Path) -> Vec<(PathBuf, Note)> {
    let mut v: Vec<(PathBuf, Note)> = std::fs::read_dir(dir.join("notes")).into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jnt"))
        .filter_map(|p| std::fs::read(&p).ok().and_then(|b| Note::from_bytes(&b)).map(|n| (p, n)))
        .collect();
    // newest writing first, as the app lists them
    v.sort_by_key(|(_, n)| std::cmp::Reverse(n.strokes().last().map(|s| s.id.0).unwrap_or(0)));
    v
}

fn cmd_notes(dir: &Path) -> Result<(), Fail> {
    let notes = notes_in(dir);
    if notes.is_empty() { println!("no notes in {}", dir.join("notes").display()); }
    for (_, n) in &notes {
        let bytes = n.to_bytes().len();
        let frames = bytes.div_ceil(share::PAYLOAD).max(1);
        println!("{}  {:>5} strokes  {:>7} bytes  {:>2} frame{}  {}", identity::hex(&n.id), n.len(), bytes, frames, if frames == 1 { " " } else { "s" }, n.title());
    }
    Ok(())
}

/// A note by path, or by a prefix of its id in the notes directory.
fn find_note(dir: &Path, what: &str) -> Result<(PathBuf, Vec<u8>), Fail> {
    let p = Path::new(what);
    if p.is_file() {
        let b = std::fs::read(p).map_err(|e| Fail::Run(format!("{what}: {e}")))?;
        if Note::from_bytes(&b).is_none() { return Err(Fail::Run(format!("{what} is not a jotter note"))); }
        return Ok((p.to_path_buf(), b));
    }
    let want = what.trim().to_ascii_lowercase();
    let hits: Vec<(PathBuf, Note)> = if want.len() >= 2 && want.chars().all(|c| c.is_ascii_hexdigit()) {
        notes_in(dir).into_iter().filter(|(_, n)| identity::hex(&n.id).starts_with(&want)).collect()
    } else { vec![] };
    match hits.len() {
        1 => Ok((hits[0].0.clone(), hits[0].1.to_bytes())),
        0 => Err(Fail::Run(format!("{what}: not a file, and no note in {} has that id", dir.join("notes").display()))),
        n => Err(Fail::Run(format!("{what} matches {n} notes; give more of the id"))),
    }
}

// ---- codes ------------------------------------------------------------------------------------

/// Every frame of `data` as a symbol (frames are cut to fit, so this only fails on the empty-after-cap case).
fn symbols(kind: Kind, data: &[u8]) -> Result<Vec<Matrix>, Fail> {
    share::frames(kind, data).iter().map(|f| share::encode(f)).collect::<Option<Vec<_>>>().ok_or_else(|| Fail::Run("too large to show as codes".into()))
}

/// Stops quietly when stdout goes away (`| head`): Rust ignores SIGPIPE, so a `print!` there would panic instead.
fn print_frames(mats: &[Matrix]) {
    use std::io::Write;
    let out = std::io::stdout();
    let mut out = out.lock();
    for (i, m) in mats.iter().enumerate() {
        if mats.len() > 1 && writeln!(out, "frame {} of {} ({} modules)", i + 1, mats.len(), m.size).is_err() { return; }
        if write!(out, "{}", m.ascii()).is_err() { return; }
    }
}

/// `path` with `-N` before its extension, for the N-th of several screens.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mosaic");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    path.with_file_name(format!("{stem}-{n}.{ext}"))
}

fn cmd_pair(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let me = me(o, dir)?;
    let port = port(o)?;
    let addrs = our_addrs(o, port);
    let mut book = OfferBook::default();
    let offer = book.issue(&me, addrs.clone(), now_s());
    let mats = symbols(Kind::Offer, &offer.encode(&me))?;
    match o.get("png") {
        Some(png) => {
            let m = o.num("module", 8).map_err(usage)?;
            let side = (mats[0].size as u32 + 2 * share::QUIET) * m;
            write_png(Path::new(png), side, side, &[Cell { frame: 0, x: 0, y: 0, module_px: m, side_px: side }], &mats)?;
            println!("wrote {png} ({side}x{side}, {m} px a module)");
        }
        None => print_frames(&mats),
    }
    println!("{} · {}", me.name, me.fingerprint());
    match addrs.as_slice() {
        [] => println!("the code carries no address (no route found; pass --addr HOST:PORT): a scanner can see who you are but not pair"),
        a => println!("reachable at {}; the code is good for {} minutes, once", a.join(", "), jotter::pairing::OFFER_TTL_S / 60),
    }
    if o.has("serve") { serve(me, dir, port, book)?; }
    else if !addrs.is_empty() { println!("(add --serve to wait here for the device that scans it)"); }
    Ok(())
}

fn cmd_show(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let what = o.pos.get(1).ok_or_else(|| usage("show wants a note: a .jnt path or an id prefix"))?;
    let (path, bytes) = find_note(dir, what)?;
    let note = Note::from_bytes(&bytes).expect("find_note checked");
    let mats = symbols(Kind::Note, &bytes)?;
    eprintln!("{}: {:?}, {} strokes, {} bytes, {} frame{}", path.display(), note.title(), note.len(), bytes.len(), mats.len(), if mats.len() == 1 { "" } else { "s" });
    let Some(png) = o.get("png") else { print_frames(&mats); return Ok(()) };
    let (w, h) = (o.num("w", 1620).map_err(usage)?, o.num("h", 2160).map_err(usage)?);
    let modules = mats.iter().map(|m| m.size).max().unwrap_or(21);
    let screens = share::mosaic(mats.len(), modules, w, h, share::MIN_MODULE_PX);
    let only = match o.get("screen") {
        Some(s) => {
            let n = s.parse::<usize>().ok().filter(|n| (1..=screens.len()).contains(n)).ok_or_else(|| usage(format!("--screen wants 1..={}, not {s:?}", screens.len())))?;
            Some(n - 1)
        }
        None => None,
    };
    for (i, cells) in screens.iter().enumerate() {
        if only.is_some_and(|k| k != i) { continue; }
        let out = if screens.len() == 1 || only.is_some() { PathBuf::from(png) } else { numbered(Path::new(png), i + 1) };
        write_png(&out, w, h, cells, &mats)?;
        println!("wrote {} (screen {} of {}: {} code{} at {} px a module)", out.display(), i + 1, screens.len(), cells.len(), if cells.len() == 1 { "" } else { "s" }, cells[0].module_px);
    }
    Ok(())
}

// ---- scanning ---------------------------------------------------------------------------------

fn cmd_scan(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let images = &o.pos[1..];
    if images.is_empty() { return Err(usage("scan wants at least one image")); }
    let mut frames: Vec<Vec<u8>> = vec![];
    for img in images {
        let codes = codes_in(Path::new(img))?;
        let seen = codes.len();
        let before = frames.len();
        frames.extend(codes.into_iter().filter(|c| share::parse_frame(c).is_some()));
        let ours = frames.len() - before;
        eprintln!("{img}: {seen} code{}, {ours} jotter frame{}", if seen == 1 { "" } else { "s" }, if ours == 1 { "" } else { "s" });
    }
    if frames.is_empty() { return Err(Fail::Run(format!("no jotter code in {} image{}", images.len(), if images.len() == 1 { "" } else { "s" }))); }
    let Some((kind, data)) = share::reassemble(&frames) else {
        // say which parts are there, so the next photo can be of the missing ones
        let mut parts: Vec<(u16, u16)> = frames.iter().filter_map(|f| share::parse_frame(f)).map(|(_, p, c, _, _)| (p, c)).collect();
        parts.sort();
        parts.dedup();
        let count = parts.iter().map(|(_, c)| *c).max().unwrap_or(0);
        let have: Vec<String> = parts.iter().map(|(p, _)| (p + 1).to_string()).collect();
        let missing: Vec<String> = (0..count).filter(|p| !parts.iter().any(|(q, _)| q == p)).map(|p| (p + 1).to_string()).collect();
        return Err(Fail::Run(format!("incomplete: have part{} {} of {count}, missing {}{}", if have.len() == 1 { "" } else { "s" }, have.join(","), if missing.is_empty() { "none -- frames of different sets, or a damaged one" } else { "" }, missing.join(","))));
    };
    match kind {
        Kind::Offer => {
            let offer = Offer::decode(&data).map_err(|e| Fail::Run(format!("the offer does not verify: {e:?}")))?;
            println!("pairing offer from {:?}\nfingerprint  {}\nid           {}\naddresses    {}\nissued       {} ({})", offer.name, identity::fingerprint(&offer.ed_pub), identity::hex(&offer.ed_pub[..8]),
                if offer.addrs.is_empty() { "none".to_string() } else { offer.addrs.join(", ") }, offer.issued, if offer.expired(now_s()) { "EXPIRED" } else { "current" });
            if !o.has("pair") { println!("(add --pair to claim it and trust that device when its owner says yes)"); return Ok(()); }
            if offer.addrs.is_empty() { return Err(Fail::Run("the code carries no address to connect to".into())); }
            let me = me(o, dir)?;
            let mut trust = TrustStore::load(&dir.join("trusted"));
            println!("connecting; the other device's owner has to accept {} {} ...", me.name, me.fingerprint());
            sync::pair_with(&offer, &me, our_addrs(o, port(o)?), &mut trust).map_err(|e| Fail::Run(format!("pairing failed: {e:?}")))?;
            trust.save(&dir.join("trusted")).map_err(|e| Fail::Run(format!("{}: {e}", dir.join("trusted").display())))?;
            println!("trusted {} {}", offer.name, identity::fingerprint(&offer.ed_pub));
        }
        Kind::Note => {
            let note = Note::from_bytes(&data).ok_or_else(|| Fail::Run("the frames assembled but are not a note".into()))?;
            println!("note {} {:?}: {} strokes, {} bytes", identity::hex(&note.id), note.title(), note.len(), data.len());
            if !o.has("save") { println!("(add --save to merge it into {})", dir.join("notes").display()); return Ok(()); }
            let mut store = FileStore::new(dir.join("notes"));
            store.put(&data).map_err(Fail::Run)?;
            println!("{} {}", if store.changed > 0 { "merged into" } else { "already in" }, dir.join("notes").join(format!("{}.jnt", identity::hex(&note.id))).display());
        }
    }
    Ok(())
}

// ---- serving and syncing ----------------------------------------------------------------------

/// Listen on `port` for syncs from trusted devices and for the claim of an offer in `book`; a claim is put to the
/// terminal (`trust NAME FINGERPRINT? [y/N]`) and nothing is trusted on anything but a typed yes.
fn serve(me: Identity, dir: &Path, port: u16, book: OfferBook) -> Result<(), Fail> {
    let listener = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| Fail::Run(format!("cannot listen on port {port}: {e}")))?;
    let trust_path = dir.join("trusted");
    let host = std::sync::Arc::new(sync::Host {
        me,
        trust: std::sync::Mutex::new(TrustStore::load(&trust_path)),
        trust_path: Some(trust_path),
        offers: std::sync::Mutex::new(book),
        policy: std::sync::Mutex::new(Box::new(|who: &Offer| {
            println!("\ntrust {:?} {}? [y/N] ", who.name, identity::fingerprint(&who.ed_pub));
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
        })),
        store_dir: dir.join("notes"),
        changes: std::sync::atomic::AtomicU64::new(0),
    });
    println!("listening on port {port} (Ctrl-C to stop)");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    sync::serve(listener, host, stop, std::sync::Arc::new(|m: String| println!("{m}")));
    Ok(())
}

fn cmd_serve(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let me = me(o, dir)?;
    serve(me, dir, port(o)?, OfferBook::default())
}

fn cmd_sync(o: &Opts, dir: &Path) -> Result<(), Fail> {
    let addr = o.pos.get(1).ok_or_else(|| usage("sync wants HOST:PORT (see `jotter devices`)"))?;
    let me = me(o, dir)?;
    let trust = TrustStore::load(&dir.join("trusted"));
    let mut store = FileStore::new(dir.join("notes"));
    let (peer, r) = sync::sync_with(addr, &me, &trust, &mut store).map_err(|e| Fail::Run(format!("sync with {addr} failed: {e:?}")))?;
    println!("synced with {} {}: sent {}, received {}, {} note{} changed here", peer.name, peer.fingerprint(), r.sent, r.received, store.changed, if store.changed == 1 { "" } else { "s" });
    Ok(())
}

// ---- pixels (feature `scan`: the only thing that needs `image`) ---------------------------------

/// Draw `cells` of `mats` (as `share::mosaic` laid them out) on a white `w` x `h` canvas, dark modules black.
#[cfg(feature = "scan")]
fn render(w: u32, h: u32, cells: &[Cell], mats: &[Matrix]) -> image::GrayImage {
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255u8]));
    for c in cells {
        let m = &mats[c.frame];
        let q = share::QUIET * c.module_px;
        for y in 0..m.size {
            for x in 0..m.size {
                if !m.get(x, y) { continue; }
                for dy in 0..c.module_px {
                    for dx in 0..c.module_px {
                        let (px, py) = (c.x + q + x as u32 * c.module_px + dx, c.y + q + y as u32 * c.module_px + dy);
                        if px < w && py < h { img.put_pixel(px, py, image::Luma([0u8])); }
                    }
                }
            }
        }
    }
    img
}

#[cfg(feature = "scan")]
fn write_png(path: &Path, w: u32, h: u32, cells: &[Cell], mats: &[Matrix]) -> Result<(), Fail> {
    render(w, h, cells, mats).save(path).map_err(|e| Fail::Run(format!("{}: {e}", path.display())))
}

/// The payloads of every QR symbol in a greyscale plane (a mosaic has many), raw bytes, undecodable ones skipped.
#[cfg(feature = "scan")]
fn codes_in_luma(luma: &[u8], w: usize, h: usize) -> Vec<Vec<u8>> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma[y * w + x]);
    img.detect_grids().iter().filter_map(|g| { let mut b = Vec::new(); g.decode_to(&mut b).ok().map(|_| b) }).collect()
}

#[cfg(feature = "scan")]
fn codes_in(path: &Path) -> Result<Vec<Vec<u8>>, Fail> {
    let img = image::open(path).map_err(|e| Fail::Run(format!("{}: {e}", path.display())))?.to_luma8();
    let (w, h) = img.dimensions();
    Ok(codes_in_luma(img.as_raw(), w as usize, h as usize))
}

#[cfg(not(feature = "scan"))]
fn write_png(_: &Path, _: u32, _: u32, _: &[Cell], _: &[Matrix]) -> Result<(), Fail> {
    Err(usage("this jotter was built without the `scan` feature, which is what writes PNGs"))
}

#[cfg(not(feature = "scan"))]
fn codes_in(_: &Path) -> Result<Vec<Vec<u8>>, Fail> {
    Err(usage("this jotter was built without the `scan` feature (cargo build --features scan)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> { s.split_whitespace().map(str::to_string).collect() }

    #[test]
    fn options_take_values_only_where_declared_and_the_rest_are_positional() {
        let o = parse(args("--dir /d show n.jnt --png=o.png --w 10 --screen 2 --serve --addr a:1 --addr b:2")).unwrap();
        assert_eq!(o.pos, ["show", "n.jnt"]);
        assert_eq!((o.get("dir"), o.get("png"), o.get("w"), o.get("screen")), (Some("/d"), Some("o.png"), Some("10"), Some("2")));
        assert!(o.has("serve") && !o.has("pair"));
        assert_eq!(o.all("addr"), ["a:1", "b:2"]);
        assert_eq!(o.num("w", 5).unwrap(), 10);
        assert_eq!(o.num("h", 5).unwrap(), 5, "absent: the default");
        assert!(o.num("screen", 1).is_ok() && parse(args("show --w 0")).unwrap().num("w", 1).is_err(), "zero is not a size");
        assert!(parse(args("show --w")).is_err(), "a valued flag at the end has no value");
        assert!(parse(args("show --serve=1")).is_err(), "a bare flag takes none");
        assert!(parse(args("show --bogus")).is_err(), "typos are refused, not ignored");
        assert_eq!(parse(args("show -- --weird")).unwrap().pos, ["show", "--weird"]);
        // the last of a repeated valued flag wins
        assert_eq!(parse(args("show --w 1 --w 2")).unwrap().get("w"), Some("2"));
    }

    #[test]
    fn a_note_is_found_by_path_or_by_an_id_prefix_and_junk_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        let n = Note::new([0xab, 0xcd, 1, 2, 3, 4, 5, 6], "Plan", 1, [1; 8]);
        let p = dir.join("notes").join("abcd010203040506.jnt");
        std::fs::write(&p, n.to_bytes()).unwrap();
        std::fs::write(dir.join("junk.jnt"), b"not a note").unwrap();
        assert_eq!(find_note(dir, p.to_str().unwrap()).ok().map(|(_, b)| b), Some(n.to_bytes()));
        assert_eq!(find_note(dir, "abcd").ok().map(|(q, _)| q), Some(p.clone()), "an id prefix");
        assert!(matches!(find_note(dir, dir.join("junk.jnt").to_str().unwrap()), Err(Fail::Run(m)) if m.contains("not a jotter note")));
        assert!(matches!(find_note(dir, "ffff"), Err(Fail::Run(_))));
        assert!(matches!(find_note(dir, "a"), Err(Fail::Run(_))), "one hex digit is not a prefix worth matching");
        // two notes sharing a prefix: ambiguous
        let m = Note::new([0xab, 0xce, 0, 0, 0, 0, 0, 0], "Other", 1, [1; 8]);
        std::fs::write(dir.join("notes").join("abce000000000000.jnt"), m.to_bytes()).unwrap();
        assert!(matches!(find_note(dir, "abc"), Err(Fail::Run(m)) if m.contains("matches 2")));
        assert_eq!(find_note(dir, "abcd").unwrap().0, p);
        assert_eq!(numbered(Path::new("/x/out.png"), 3), PathBuf::from("/x/out-3.png"));
    }

    fn stroke(id: u64, n: usize) -> ink_kit::stroke::Stroke {
        use ink_kit::stroke::{InkPoint, Stroke, StrokeId, StrokeStyle, ToolKind};
        let mut s = Stroke::new(StrokeId(id), StrokeStyle { width: 2.5, color: [10, 10, 10, 255], tool: ToolKind::Pen });
        for k in 0..n { s.points.push(InkPoint::new(ink_kit::Vec2d::new(k as f64 * 1.3, (k as f64 * 0.7).sin() * 40.0), 0.5, 0.0)); }
        s
    }

    /// The whole scan path, without a camera: a code rendered the way `--png` renders it, read back the way `scan`
    /// reads it. At the SMALLEST module the mosaic allows, since that is what a tablet screen full of codes uses.
    #[cfg(feature = "scan")]
    #[test]
    fn a_pairing_code_rendered_at_the_minimum_module_scans_back_to_the_same_offer() {
        let me = Identity::generate("Tablet");
        let offer = Offer::new(&me, vec!["192.168.2.185:7471".into()], 1_000);
        let mats = symbols(Kind::Offer, &offer.encode(&me)).unwrap();
        assert_eq!(mats.len(), 1);
        for m in [share::MIN_MODULE_PX, 8] {
            let side = (mats[0].size as u32 + 2 * share::QUIET) * m;
            let img = render(side, side, &[Cell { frame: 0, x: 0, y: 0, module_px: m, side_px: side }], &mats);
            let codes = codes_in_luma(img.as_raw(), side as usize, side as usize);
            assert_eq!(codes.len(), 1, "one symbol at {m} px a module");
            let (kind, data) = share::reassemble(&codes).expect("one frame reassembles");
            assert_eq!((kind, Offer::decode(&data).unwrap()), (Kind::Offer, offer.clone()), "{m} px a module");
        }
        // the render is the picture the matrix describes: a dark module is black, the quiet zone white
        let side = (mats[0].size as u32 + 2 * share::QUIET) * 4;
        let img = render(side, side, &[Cell { frame: 0, x: 0, y: 0, module_px: 4, side_px: side }], &mats);
        assert_eq!(img.get_pixel(0, 0).0, [255], "quiet zone");
        assert_eq!(img.get_pixel(4 * share::QUIET, 4 * share::QUIET).0, [0], "the finder's corner is dark");
        assert_eq!(img.get_pixel(4 * share::QUIET + 4, 4 * share::QUIET + 4).0, [255], "and the module inside the finder ring is light");
    }

    /// A note too big for one code, laid out as the tablet lays it out, scanned from the screens as `scan` would
    /// from screenshots: the frames come back out of order across several images and still make the note.
    #[cfg(feature = "scan")]
    #[test]
    fn a_multi_frame_note_mosaic_scans_back_from_its_screens() {
        let mut n = Note::new([9; 8], "Long", 1, [1; 8]);
        for i in 0..6 { n.add(stroke((i + 1) << 24, 900)); }
        let bytes = n.to_bytes();
        let mats = symbols(Kind::Note, &bytes).unwrap();
        assert!(mats.len() >= 3, "{} bytes should be several frames, got {}", bytes.len(), mats.len());
        assert!(mats[..mats.len() - 1].iter().all(|m| m.size == 177), "every full frame is a version-40 symbol; only the tail is smaller");
        // a small screen, so the frames spread over MORE than one screen (the user takes several screenshots)
        let (w, h) = (1200u32, 1200u32);
        let screens = share::mosaic(mats.len(), 177, w, h, share::MIN_MODULE_PX);
        assert!(screens.len() >= 2, "{} screens", screens.len());
        let mut frames = vec![];
        for cells in screens.iter().rev() {
            let img = render(w, h, cells, &mats);
            let got = codes_in_luma(img.as_raw(), w as usize, h as usize);
            assert_eq!(got.len(), cells.len(), "every code on the screen reads ({} px a module)", cells[0].module_px);
            frames.extend(got);
        }
        let (kind, data) = share::reassemble(&frames).expect("all frames, out of order");
        assert_eq!(kind, Kind::Note);
        assert!(data == bytes, "the bytes come back exactly ({} of them)", bytes.len());
        assert_eq!(Note::from_bytes(&data).map(|m| (m.id, m.len())), Some((n.id, n.len())));
        // one screen short: nothing assembles, and the caller can say which parts are missing
        assert!(share::reassemble(&frames[..frames.len() - 1]).is_none());
    }

    /// `scan --save` is `FileStore::put`: a note arriving by QR merges with the copy on disk rather than replacing it.
    #[test]
    fn a_scanned_note_merges_with_the_one_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let mut here = Note::new([3; 8], "Shared", 1, [1; 8]);
        let mut there = here.clone();
        here.add(stroke(5 << 24, 3));
        there.add(stroke(6 << 24, 3));
        let notes = d.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join(format!("{}.jnt", identity::hex(&here.id))), here.to_bytes()).unwrap();
        let mut store = FileStore::new(notes.clone());
        store.put(&there.to_bytes()).unwrap();
        assert_eq!(store.changed, 1);
        let merged = Note::from_bytes(&std::fs::read(notes.join(format!("{}.jnt", identity::hex(&here.id)))).unwrap()).unwrap();
        assert_eq!(merged.len(), 2, "both strokes");
        store.put(&there.to_bytes()).unwrap();
        assert_eq!(store.changed, 1, "a second scan of the same code changes nothing");
    }
}
