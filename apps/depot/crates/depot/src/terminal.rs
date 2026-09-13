//! An in-browser terminal: a real shell ON THE DEPOT HOST, reachable from the
//! UI. This is the piece voiceterm-egui already solved on the desktop —
//! `portable-pty` for the process, a reader thread, keycap byte sequences —
//! reused here for a Fairphone that IS its own depot, so "install a package"
//! is tapping keys on the screen instead of finding a laptop.
//!
//! Why this is the honest shape for a std HTTP server. A desktop terminal
//! pushes output the instant it arrives; this server's `Body` is a finished
//! buffer, not a stream, and giving a handler the raw socket to do
//! Server-Sent Events would tear a hole in the `Handler` abstraction the
//! whole app rests on. So the browser SHORT-POLLS: it POSTs the bytes you
//! typed and asks "anything since offset N?" a few times a second. Higher
//! latency than a pty over a websocket, invisible for tapping commands, and
//! it cannot wedge the server — every response is an ordinary buffered body.
//!
//! Safety. Off unless `[terminal] enabled = true`; every endpoint needs the
//! admin token; under the hardened unit it is the unprivileged `DynamicUser`
//! with no sudo and a filesystem view restricted to the depot. It is still
//! remote code execution over plain HTTP — for a device you hold on a LAN
//! you own, which is exactly the Fairphone case. DECISIONS §17.

use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A ring of recent PTY output. The browser addresses bytes by absolute
/// offset since the session began; we keep the last `CAP` bytes and the
/// total ever produced, so a client that asks for an offset we have already
/// discarded is told to resync from the current base.
const CAP: usize = 256 * 1024;

struct Ring {
    buf: Vec<u8>,
    /// Absolute offset of `buf[0]` in the full output stream.
    base: u64,
    total: u64,
}

impl Ring {
    fn new() -> Ring {
        Ring { buf: Vec::new(), base: 0, total: 0 }
    }
    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        self.total += data.len() as u64;
        if self.buf.len() > CAP {
            let drop = self.buf.len() - CAP;
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }
    /// Bytes at or after `from`. Returns (next_offset, bytes). If `from` is
    /// behind what we still hold, the caller resyncs from `self.base`.
    fn since(&self, from: u64) -> (u64, Vec<u8>) {
        let start = from.max(self.base);
        let off = (start - self.base) as usize;
        (self.total, self.buf.get(off..).unwrap_or(&[]).to_vec())
    }
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    out: Arc<Mutex<Ring>>,
    alive: Arc<AtomicBool>,
    last_active: AtomicU64,
    cols: u16,
    rows: u16,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Session {
    fn spawn(shell: &str, cwd: &std::path::Path, cols: u16, rows: u16) -> std::io::Result<Session> {
        let sys = NativePtySystem::default();
        let pair = sys.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(oe)?;
        let prog = if shell.is_empty() { std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()) } else { shell.to_string() };
        let mut cmd = CommandBuilder::new(&prog);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        // A calm prompt for a small screen; the user's rc may override it.
        cmd.env("PS1", "depot:\\w$ ");
        let child = pair.slave.spawn_command(cmd).map_err(oe)?;
        drop(pair.slave);
        let writer = pair.master.take_writer().map_err(oe)?;
        let mut reader = pair.master.try_clone_reader().map_err(oe)?;
        let out = Arc::new(Mutex::new(Ring::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let out2 = Arc::clone(&out);
        let alive2 = Arc::clone(&alive);
        // One blocking reader thread per session. Ends when the shell closes
        // the pty (EOF) — no polling, no shared UI loop to wake.
        std::thread::spawn(move || {
            let mut b = [0u8; 8192];
            loop {
                match reader.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => out2.lock().unwrap().push(&b[..n]),
                }
            }
            alive2.store(false, Ordering::Relaxed);
        });
        Ok(Session { master: pair.master, writer, child, out, alive, last_active: AtomicU64::new(now()), cols, rows })
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.last_active.store(now(), Ordering::Relaxed);
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != (self.cols, self.rows) {
            let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            self.cols = cols;
            self.rows = rows;
        }
    }

    fn running(&mut self) -> bool {
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }
        matches!(self.child.try_wait(), Ok(None))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.alive.store(false, Ordering::Relaxed);
    }
}

fn oe(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// All live sessions for one depot. Cheap to hold in the `App`.
pub struct Terminals {
    shell: String,
    idle_timeout: u64,
    root: std::path::PathBuf,
    sessions: Mutex<HashMap<String, Session>>,
    counter: AtomicU64,
}

pub struct Chunk {
    pub next: u64,
    pub data: Vec<u8>,
    pub running: bool,
    /// True when `from` was behind the retained window and the client must
    /// treat `data` as a fresh screen from `next - data.len()`.
    pub resynced: bool,
}

impl Terminals {
    pub fn new(shell: String, idle_timeout: u64, root: std::path::PathBuf) -> Terminals {
        Terminals { shell, idle_timeout, root, sessions: Mutex::new(HashMap::new()), counter: AtomicU64::new(0) }
    }

    /// Open a session; returns its id. Reaps idle/dead sessions first so a
    /// tab left open does not hold a shell forever.
    pub fn open(&self, cols: u16, rows: u16) -> std::io::Result<String> {
        self.reap();
        let s = Session::spawn(&self.shell, &self.root, cols.clamp(20, 400), rows.clamp(4, 200))?;
        let id = format!("t{}-{}", std::process::id(), self.counter.fetch_add(1, Ordering::Relaxed));
        self.sessions.lock().unwrap().insert(id.clone(), s);
        Ok(id)
    }

    pub fn input(&self, id: &str, bytes: &[u8]) -> Option<()> {
        let mut g = self.sessions.lock().unwrap();
        let s = g.get_mut(id)?;
        s.write(bytes).ok()
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Option<()> {
        let mut g = self.sessions.lock().unwrap();
        let s = g.get_mut(id)?;
        s.resize(cols.clamp(20, 400), rows.clamp(4, 200));
        Some(())
    }

    pub fn poll(&self, id: &str, from: u64) -> Option<Chunk> {
        let mut g = self.sessions.lock().unwrap();
        let s = g.get_mut(id)?;
        s.last_active.store(now(), Ordering::Relaxed);
        let ring = s.out.lock().unwrap();
        let resynced = from < ring.base;
        let (next, data) = ring.since(from);
        drop(ring);
        Some(Chunk { next, data, running: s.running(), resynced })
    }

    pub fn close(&self, id: &str) -> bool {
        match self.sessions.lock().unwrap().remove(id) {
            Some(mut s) => {
                s.kill();
                true
            }
            None => false,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// Drop sessions whose shell exited or that have been idle too long.
    pub fn reap(&self) {
        let now = now();
        let mut g = self.sessions.lock().unwrap();
        let mut dead: Vec<String> = Vec::new();
        for (k, s) in g.iter_mut() {
            if !s.running() || now.saturating_sub(s.last_active.load(Ordering::Relaxed)) > self.idle_timeout {
                dead.push(k.clone());
            }
        }
        for k in dead {
            if let Some(mut s) = g.remove(&k) {
                s.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait_for<F: Fn() -> bool>(f: F) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_shell_runs_a_command_and_output_is_polled_by_offset() {
        let dir = std::env::temp_dir().join(format!("depot-term-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Terminals::new("/bin/sh".into(), 900, dir.clone());
        let id = t.open(80, 24).unwrap();
        assert_eq!(t.count(), 1);
        // Drain the prompt/banner, then run a command with a unique marker.
        let mut off = 0u64;
        wait_for(|| t.poll(&id, off).map(|c| c.next > 0).unwrap_or(false));
        off = t.poll(&id, off).unwrap().next;
        t.input(&id, b"echo depot-marker-42\n").unwrap();
        let saw = wait_for(|| {
            let c = t.poll(&id, off).unwrap();
            String::from_utf8_lossy(&c.data).contains("depot-marker-42")
        });
        assert!(saw, "command output should appear");
        // Offset monotonic: a second poll from the new offset returns nothing new yet.
        let c = t.poll(&id, off).unwrap();
        let c2 = t.poll(&id, c.next).unwrap();
        assert!(c2.data.is_empty() || c2.next >= c.next);

        // exit ends the shell; running() flips and the session reaps.
        t.input(&id, b"exit\n").unwrap();
        assert!(wait_for(|| !t.poll(&id, 0).map(|c| c.running).unwrap_or(true)), "shell should exit");
        t.reap();
        assert_eq!(t.count(), 0, "dead session reaped");
        assert!(t.poll(&id, 0).is_none(), "gone after reap");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_session_is_none_and_close_reports() {
        let t = Terminals::new(String::new(), 900, std::env::temp_dir());
        assert!(t.poll("nope", 0).is_none());
        assert!(t.input("nope", b"x").is_none());
        assert!(!t.close("nope"));
        let id = t.open(80, 24).unwrap();
        assert!(t.resize(&id, 100, 30).is_some());
        assert!(t.close(&id));
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn ring_discards_old_bytes_and_reports_resync() {
        let mut r = Ring::new();
        r.push(&vec![b'a'; CAP - 10]);
        let (next, data) = r.since(0);
        assert_eq!(next, (CAP - 10) as u64);
        assert_eq!(data.len(), CAP - 10);
        r.push(&vec![b'b'; 100]); // now over CAP: oldest 90 dropped
        assert_eq!(r.base, 90);
        assert_eq!(r.total, (CAP + 90) as u64);
        let (_, tail) = r.since(0); // asking for a dropped offset
        assert_eq!(tail.len(), CAP, "only the retained window comes back");
        assert!(0 < r.base, "a client at offset 0 is behind base → resync");
    }
}
