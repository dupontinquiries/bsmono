//! logscope-fabricd — the fabric daemon.
//!
//! Exposes this box's models to the LAN so other machines can *join the workspace* (see `fabric_net`):
//! it stands up the `/fabric/hello` + `/fabric/gen` endpoints, advertises this box's PUBLIC models, and
//! serves generation requests — every peer that `join`s sees the models in its pipeline menu and can
//! route to them.
//!
//! Built to run under a supervisor (systemd `Type=simple` / OpenRC) — foreground, logging to stdout so
//! journald captures it — or to self-detach with `--daemon`. It shuts down cleanly on SIGTERM/SIGINT
//! (in-flight requests drain) and removes its PID file.
//!
//! Real-model serving needs the `wgpu-engine` feature (a `LocalWorker` over a GGUF). Without it a mock
//! is served, so the mesh — join/discovery/routing/public-private — still stands up and is testable.
//!
//! ```text
//! logscope-fabricd --addr 0.0.0.0:7777 --device studio-m2 \
//!                  --model llama-1b=/models/llama-1b.gguf:1.0        # real (wgpu-engine)
//! logscope-fabricd --daemon --pid-file /run/logscope-fabricd.pid \
//!                  --log-file /var/log/logscope-fabricd.log          # detached
//! ```

use logscope_egui::fabric::Worker;
use logscope_egui::fabric_net;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Signal handlers may only touch async-signal-safe state — an atomic flag the serve loop polls.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as usize);
        libc::signal(libc::SIGINT, on_signal as usize);
        libc::signal(libc::SIGHUP, libc::SIG_IGN); // survive a terminal hangup
        libc::signal(libc::SIGPIPE, libc::SIG_IGN); // a peer vanishing mid-write must not kill us
    }
}

/// Classic double-fork + setsid: the daemon outlives its shell and has no controlling tty. stdio is
/// redirected to `log_file` (append) or `/dev/null`.
fn daemonize(log_file: Option<&str>) {
    unsafe {
        if libc::fork() > 0 {
            libc::_exit(0); // original parent returns to the shell
        }
        libc::setsid(); // new session, drop the controlling tty
        if libc::fork() > 0 {
            libc::_exit(0); // session leader exits so we can never reacquire a tty
        }
        libc::umask(0o027);

        // stdout/stderr → log file (or /dev/null); stdin → /dev/null
        let out = std::ffi::CString::new(log_file.unwrap_or("/dev/null")).unwrap();
        let fd = libc::open(out.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o640);
        if fd >= 0 {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            if fd > 2 {
                libc::close(fd);
            }
        }
        let devnull = std::ffi::CString::new("/dev/null").unwrap();
        let nfd = libc::open(devnull.as_ptr(), libc::O_RDONLY, 0);
        if nfd >= 0 {
            libc::dup2(nfd, 0);
            if nfd > 2 {
                libc::close(nfd);
            }
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "logscope".to_string())
}

/// Build the fabric worker this daemon serves. Real GGUF-backed `LocalWorker` under `wgpu-engine`;
/// otherwise a mock so the mesh stands up without a GPU. `--model name=path[:params_b]`.
fn build_worker(device: &str, author: &str, models: &[String]) -> Arc<dyn Worker + Send + Sync> {
    #[cfg(feature = "wgpu-engine")]
    {
        if let Some(spec) = models.first() {
            let (name, rest) = spec.split_once('=').unwrap_or(("default", spec.as_str()));
            let (path, params_b) = match rest.rsplit_once(':') {
                Some((p, b)) => (p, b.parse::<f32>().unwrap_or(1.0)),
                None => (rest, 1.0),
            };
            match logscope_egui::backend::LocalWorker::load(device, author, name, true, params_b, path) {
                Ok(w) => return Arc::new(w),
                Err(e) => eprintln!("# model '{name}' load failed: {e} — serving a mock instead"),
            }
        }
    }
    let _ = models;
    Arc::new(logscope_egui::fabric::MockWorker::new(device, author, &[("mock", true, 0.0)]))
}

fn main() {
    let cli = cli_kit::Cli::from(std::env::args().skip(1).collect::<Vec<String>>());
    let addr = cli.opt("addr").unwrap_or("0.0.0.0:7777").to_string();
    let device = cli.opt("device").map(String::from).unwrap_or_else(hostname);
    let author = cli.opt("author").unwrap_or("logscope").to_string();
    let models: Vec<String> = cli.opt_all("model").iter().map(|s| s.to_string()).collect();
    let pid_file = cli.opt("pid-file").map(String::from);
    let log_file = cli.opt("log-file").map(String::from);
    let detach = cli.flag("daemon");

    if detach {
        daemonize(log_file.as_deref());
    }
    install_signals();
    if let Some(pf) = &pid_file {
        if let Err(e) = std::fs::write(pf, format!("{}\n", std::process::id())) {
            eprintln!("# warning: could not write pid file {pf}: {e}");
        }
    }

    let worker = build_worker(&device, &author, &models);
    eprintln!(
        "# logscope-fabricd: {author}@{device} on {addr} — {} model(s) [pid {}]",
        worker.models().len(),
        std::process::id()
    );
    for m in worker.models() {
        eprintln!("#   {:<20} {:>5.1}B{}", m.name, m.params_b, if m.public { "" } else { "  [private]" });
    }

    // bridge the async-signal flag → the serve loop's shutdown handle
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            while !SHUTDOWN.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
            s.store(true, Ordering::SeqCst);
        });
    }

    let code = match fabric_net::serve_until(&addr, worker, shutdown) {
        Ok(()) => {
            eprintln!("# logscope-fabricd: shut down cleanly");
            0
        }
        Err(e) => {
            eprintln!("# logscope-fabricd: serve failed on {addr}: {e}");
            1
        }
    };
    if let Some(pf) = &pid_file {
        let _ = std::fs::remove_file(pf);
    }
    std::process::exit(code);
}
