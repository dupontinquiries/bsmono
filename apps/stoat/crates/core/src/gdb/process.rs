use super::translate::{encode, translate, Pending, PendingMap};
use crate::backend::{Backend, Channel, Dir, Event, ReqId, Request};
use async_channel::{Receiver, Sender};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command as Proc, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use stoat_mi::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Debuginfod {
    /// Never fetch. The default: on a metered connection a single stop inside
    /// GTK can pull hundreds of MB, and gdb does not ask first under MI.
    #[default]
    Off,
    On,
    /// Whatever gdb and the environment decide.
    Inherit,
}

#[derive(Debug, Clone)]
pub struct GdbConfig {
    pub gdb: PathBuf,
    pub debuginfod: Debuginfod,
    /// A Rust sysroot to load std's pretty-printers from. Plain `gdb` does not do
    /// this; `rust-gdb` is a shell wrapper that passes these same flags.
    pub rust_sysroot: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl Default for GdbConfig {
    fn default() -> Self {
        GdbConfig { gdb: "gdb".into(), debuginfod: Debuginfod::default(), rust_sysroot: detect_rust_sysroot(), extra_args: Vec::new() }
    }
}

pub fn detect_rust_sysroot() -> Option<PathBuf> {
    let out = Proc::new("rustc").arg("--print=sysroot").output().ok()?;
    let root = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    root.join("lib/rustlib/etc/gdb_load_rust_pretty_printers.py").is_file().then_some(root)
}

pub struct GdbBackend {
    child: Child,
    stdin: ChildStdin,
    next: u64,
    pending: Arc<Mutex<PendingMap>>,
    tx: Sender<Event>,
    pty: File,
    stop: Arc<AtomicBool>,
}

impl GdbBackend {
    pub fn spawn(cfg: &GdbConfig) -> io::Result<(GdbBackend, Receiver<Event>)> {
        let (tx, rx) = async_channel::unbounded();
        let (pty, slave_path, slave_keepalive) = open_pty()?;

        let mut cmd = Proc::new(&cfg.gdb);
        // -nx: a user's .gdbinit (gef, pwndbg, dashboards) prints to the console
        // on every stop and changes settings the frontend relies on. Raw commands
        // can still `source` it deliberately.
        cmd.args(["-q", "-nx", "--interpreter=mi3"]);
        if let Some(root) = &cfg.rust_sysroot {
            let etc = root.join("lib/rustlib/etc");
            cmd.arg(format!("--directory={}", etc.display()));
            cmd.args(["-iex", &format!("add-auto-load-safe-path {}", etc.display())]);
        }
        cmd.args(&cfg.extra_args);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let pending = Arc::new(Mutex::new(PendingMap::new()));
        let stop = Arc::new(AtomicBool::new(false));

        std::thread::Builder::new().name("stoat-gdb-out".into()).spawn({
            let (tx, pending) = (tx.clone(), pending.clone());
            move || {
                let mut r = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match r.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let at = Instant::now();
                    let rec = stoat_mi::parse_line(&line);
                    let text = String::from_utf8_lossy(&line).trim_end().to_string();
                    let events = translate(rec, at, &mut pending.lock().unwrap());
                    if tx.send_blocking(Event::Wire { dir: Dir::FromBackend, line: text }).is_err() {
                        return;
                    }
                    for e in events {
                        if tx.send_blocking(e).is_err() {
                            return;
                        }
                    }
                }
                let _ = tx.send_blocking(Event::BackendGone { detail: "gdb closed its output".into() });
            }
        })?;

        std::thread::Builder::new().name("stoat-gdb-err".into()).spawn({
            let tx = tx.clone();
            move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if tx.send_blocking(Event::Output { channel: Channel::Log, text: line + "\n" }).is_err() {
                        return;
                    }
                }
            }
        })?;

        std::thread::Builder::new().name("stoat-pty".into()).spawn({
            let (tx, stop) = (tx.clone(), stop.clone());
            let mut master = pty.try_clone()?;
            move || {
                // Holding the slave open here is what keeps read() from returning
                // EIO in the window before gdb has opened it for the inferior, and
                // again between runs.
                let _keep = slave_keepalive;
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::Relaxed) {
                    let mut pfd = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                    // SAFETY: one valid pollfd, count 1.
                    let n = unsafe { libc::poll(&mut pfd, 1, 200) };
                    if n <= 0 {
                        continue;
                    }
                    match master.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                            if tx.send_blocking(Event::Output { channel: Channel::Target, text }).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        })?;

        let mut me = GdbBackend { child, stdin, next: 1, pending, tx, pty, stop };
        // mi-async must be set before the target exists; without it gdb does not
        // read commands while the inferior runs and -exec-interrupt cannot work.
        me.send(Command::new("gdb-set").opt("mi-async").opt("on"), Pending::Plain);
        me.send(Command::new("gdb-set").opt("confirm").opt("off"), Pending::Plain);
        me.send(Command::new("gdb-set").opt("print").opt("elements").opt("64"), Pending::Plain);
        match cfg.debuginfod {
            Debuginfod::Inherit => {}
            d => {
                let v = if d == Debuginfod::On { "on" } else { "off" };
                me.send(Command::new("gdb-set").opt("debuginfod").opt("enabled").opt(v), Pending::Plain);
            }
        }
        me.send(Command::new("inferior-tty-set").arg(slave_path.display()), Pending::Plain);
        Ok((me, rx))
    }

    fn send(&mut self, cmd: Command, p: Pending) -> ReqId {
        let token = self.next;
        self.next += 1;
        self.pending.lock().unwrap().insert(token, p);
        let line = cmd.encode(token);
        let _ = self.tx.send_blocking(Event::Wire { dir: Dir::ToBackend, line: line.trim_end().to_string() });
        if self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).is_err() {
            self.pending.lock().unwrap().remove(&token);
            let _ = self.tx.send_blocking(Event::Reply { id: token, result: Err("gdb is not running".into()) });
        }
        token
    }
}

impl Backend for GdbBackend {
    /// For multi-command requests the returned id is the last command's; earlier
    /// ones still produce `Reply` events, so a failure in any of them is visible.
    fn submit(&mut self, req: Request) -> ReqId {
        if let Request::TargetInput(text) = &req {
            let _ = self.pty.write_all(text.as_bytes());
            return 0;
        }
        let mut last = 0;
        for (cmd, p) in encode(&req) {
            last = self.send(cmd, p);
        }
        last
    }
}

impl Drop for GdbBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.stdin.write_all(b"-gdb-exit\n");
        let _ = self.stdin.flush();
        // gdb kills a process it started and detaches from one it attached to.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn open_pty() -> io::Result<(File, PathBuf, File)> {
    // SAFETY: plain libc calls; every return value is checked and the fd is
    // wrapped in a File immediately so it cannot leak on the error paths.
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let master = File::from_raw_fd(fd);
        if libc::grantpt(fd) != 0 || libc::unlockpt(fd) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = [0 as libc::c_char; 128];
        if libc::ptsname_r(fd, buf.as_mut_ptr(), buf.len()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let path = PathBuf::from(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned());
        let slave = std::fs::OpenOptions::new().read(true).write(true).custom_flags_noctty().open(&path)?;
        // Leave the line discipline alone except for NL -> CRNL on output; the
        // shells render text, not a terminal, and do not want the \r.
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave.as_raw_fd(), &mut tio) == 0 {
            tio.c_oflag &= !libc::ONLCR;
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &tio);
        }
        Ok((master, path, slave))
    }
}

trait NoCtty {
    fn custom_flags_noctty(&mut self) -> &mut Self;
}

impl NoCtty for std::fs::OpenOptions {
    fn custom_flags_noctty(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        // Without O_NOCTTY a frontend started without a controlling terminal
        // (from a .desktop file) would adopt the inferior's pty as its own.
        self.custom_flags(libc::O_NOCTTY)
    }
}
