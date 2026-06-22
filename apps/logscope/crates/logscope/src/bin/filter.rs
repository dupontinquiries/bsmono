/// logscope-filter — standalone headless binary
///
/// Runs without GTK. Called from logrotate prerotate hooks and systemd units.
///
/// Usage:
///   logscope-filter --filter  --config <pipeline.toml> --input <logfile>
///   logscope-filter --rotate  --config <policy.toml>
///   logscope-filter --rotate-all                      # rotate all ~/.config/logscope/rotate/*.toml
///   logscope-filter --probe   <path>                  # print probe JSON to /dev/tty
///
/// Stdout/stderr are silenced so logrotate never sees stray output.
/// Errors go to /tmp/logscope-filter.err (appended, timestamped).

use std::path::PathBuf;
use std::io::Write;

#[cfg(unix)]
use nix::unistd::dup2;

// ── silence layer (must run before any output) ────────────────────────────────

fn silence() {
    #[cfg(unix)]
    if std::env::var_os("LOGSCOPE_DEBUG").is_none() {
        use std::os::unix::io::IntoRawFd;
        if let Ok(null) = std::fs::File::open("/dev/null") {
            let fd = null.into_raw_fd();
            // Redirect stdout and stderr to /dev/null
            // logrotate treats any output as an error signal
            let _ = nix::unistd::dup2(fd, 1);
            let _ = nix::unistd::dup2(fd, 2);
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    silence();

    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&args) {
        write_err(&e.to_string());
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut mode         = Mode::Filter;
    let mut config_path: Option<PathBuf> = None;
    let mut input_path:  Option<PathBuf> = None;
    let mut probe_path:  Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--filter"     => mode = Mode::Filter,
            "--rotate"     => mode = Mode::Rotate,
            "--rotate-all" => mode = Mode::RotateAll,
            "--probe"      => { mode = Mode::Probe; i += 1; probe_path = args.get(i).map(PathBuf::from); }
            "--config"     => { i += 1; config_path = args.get(i).map(PathBuf::from); }
            "--input"      => { i += 1; input_path  = args.get(i).map(PathBuf::from); }
            _              => {}
        }
        i += 1;
    }

    match mode {
        Mode::Filter => {
            let cfg  = config_path.ok_or("--config required")?;
            let inp  = input_path.ok_or("--input required")?;
            let mut config = logscope::pipeline::load_config(&cfg)?;
            logscope::pipeline::run(&mut config, &inp)?;
        }

        Mode::Rotate => {
            let cfg    = config_path.ok_or("--config required")?;
            let policy = logscope::rotation::load_policy(&cfg)?;
            logscope::rotation::rotate_native(&policy)?;
        }

        Mode::RotateAll => {
            // Walk ~/.config/logscope/rotate/*.toml and rotate each one
            let rotate_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from(std::env::var("HOME")
                    .unwrap_or_else(|_| "~".into())))
                .join("logscope/rotate");

            if !rotate_dir.exists() {
                return Ok(()); // nothing configured yet — silent success
            }

            let mut any_err = false;
            for entry in std::fs::read_dir(&rotate_dir)?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") { continue; }
                match logscope::rotation::load_policy(&path)
                    .and_then(|p| logscope::rotation::rotate_native(&p))
                {
                    Ok(_)  => {}
                    Err(e) => {
                        write_err(&format!("{}: {e}", path.display()));
                        any_err = true;
                    }
                }
            }
            if any_err { return Err("one or more rotations failed (see /tmp/logscope-filter.err)".into()); }
        }

        Mode::Probe => {
            // Re-open stdout to /dev/tty for interactive use only
            #[cfg(unix)]
            {
                let path = probe_path.ok_or("path required for --probe")?;
                let result = logscope::probe::probe(&path);
                let json   = serde_json::to_string_pretty(&result)?;
                if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
                    let _ = writeln!(tty, "{json}");
                }
            }
        }
    }

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn write_err(msg: &str) {
    let err_path = PathBuf::from("/tmp/logscope-filter.err");
    let line = format!("{}: {msg}\n", chrono::Utc::now().to_rfc3339());
    let _ = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open(&err_path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

enum Mode { Filter, Rotate, RotateAll, Probe }
