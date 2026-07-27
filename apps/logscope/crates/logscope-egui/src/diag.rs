//! diag.rs — capture engine output into a bounded report and write it as a zip.
//!
//! Gated on `LOGSCOPE_DIAG=1` (or `--diag`). Off by default because it tees every engine line, and a
//! diagnostic that costs something when nobody asked for it will eventually be removed by someone
//! chasing a millisecond.

use std::sync::Mutex;
use logscope::diagreport::Report;

static CAPTURE: Mutex<Option<Report>> = Mutex::new(None);

/// Is capture on? Checked once and cached, so the hot path is an atomic load rather than a getenv.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LOGSCOPE_DIAG").as_deref() == Ok("1")
        || std::env::args().any(|a| a == "--diag"))
}

/// Process start, for the monotonic offset every line carries.
fn t0() -> std::time::Instant {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *T0.get_or_init(std::time::Instant::now)
}

fn wall_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Record a line, timestamped. Cheap no-op when disabled.
///
/// ALSO PRINTS IT. The engine's own eprintln goes to the terminal, and a capture that swallowed lines
/// would make the diagnostic mode quieter than the normal one — which is the opposite of what someone
/// turning on diagnostics wants.
pub fn note(line: &str) {
    if !enabled() { return; }
    let mono = t0().elapsed().as_millis() as u64;
    let mut g = CAPTURE.lock().unwrap();
    let r = g.get_or_insert_with(Report::new);
    // classify first, then stamp: absorb() picks the section from content
    let before = r.populated().len();
    r.absorb(line);
    let _ = before;
    r.push_at("TAIL", "", wall_secs(), mono);   // keeps the clock advancing even for quiet stretches
}

/// Time a named stage and record how long it took.
///
/// Returns the closure's value, so it wraps an existing call without restructuring it.
pub fn stage<T>(name: &str, detail: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() { return f(); }
    let s = std::time::Instant::now();
    let out = f();
    let ms = s.elapsed().as_millis() as u64;
    let mut g = CAPTURE.lock().unwrap();
    g.get_or_insert_with(Report::new).stage(name, ms, detail);
    out
}

/// Add a line to a named section directly, for facts the engine never prints.
pub fn note_in(section: &str, line: impl Into<String>) {
    if !enabled() { return; }
    let mut g = CAPTURE.lock().unwrap();
    g.get_or_insert_with(Report::new).push(section, line);
}

/// Environment facts worth having in every report, gathered once.
pub fn capture_env() {
    if !enabled() { return; }
    note_in("ENV", format!("logscope {} · {} {}", env!("CARGO_PKG_VERSION"),
        std::env::consts::OS, std::env::consts::ARCH));
    // Only LOGSCOPE_* vars: the rest of the environment is the user's business and may hold secrets.
    let mut set: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("LOGSCOPE_"))
        .map(|(k, v)| format!("{k}={v}")).collect();
    set.sort();
    if set.is_empty() { note_in("ENV", "no LOGSCOPE_* variables set"); }
    for s in set { note_in("ENV", s); }
}

/// Write the report if capture is on, reporting the path on stderr.
///
/// Called from a Drop guard so it runs on normal exit AND on the panic path — a report you only get
/// when the program ends cleanly is missing precisely the runs worth reporting.
pub struct DiagGuard;
impl Drop for DiagGuard {
    fn drop(&mut self) {
        if !enabled() { return; }
        let dir = std::env::var("LOGSCOPE_DIAG_DIR").map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        match write_zip(&dir) {
            Ok(p) => eprintln!("logscope: diagnostic report written to {}", p.display()),
            Err(e) => eprintln!("logscope: could not write diagnostic report: {e}"),
        }
    }
}

/// Install the exit-time writer and record the environment. Call once at startup.
pub fn install() -> DiagGuard {
    if enabled() {
        capture_env();
        eprintln!("logscope: LOGSCOPE_DIAG=1 — a diagnostic zip will be written on exit \
(LOGSCOPE_DIAG_DIR to choose where)");
    }
    DiagGuard
}

/// Write the report as `logscope-diag-<n>.log` inside a zip. Returns the path.
///
/// Zipped because the point is to hand it over in one piece; a bare .log invites pasting a fragment,
/// and a fragment is how a section that mattered goes missing.
pub fn write_zip(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let text = {
        let g = CAPTURE.lock().unwrap();
        g.as_ref().map(|r| r.render()).unwrap_or_else(|| Report::new().render())
    };
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    let name = format!("logscope-diag-{stamp}.log");
    let zip = crate::workspace::zip_store(&[(name, text.into_bytes())]);
    let path = dir.join(format!("logscope-diag-{stamp}.zip"));
    std::fs::write(&path, zip)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_is_a_no_op_when_disabled() {
        // A diagnostic that costs something when nobody asked for it gets removed by the next person
        // chasing a millisecond, and then it is not there when it is needed.
        if !enabled() {
            note("wgpu-engine: profile — 1 submits");
            assert!(CAPTURE.lock().unwrap().is_none(), "nothing allocated while off");
        }
    }

    #[test]
    fn only_logscope_variables_are_captured() {
        // The rest of the environment is the user's business and can hold credentials. A diagnostic
        // bundle that leaks them is worse than no diagnostic bundle.
        std::env::set_var("LOGSCOPE_TEST_MARKER", "1");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "shouldnotappear");
        let mut r = Report::new();
        let mut set: Vec<String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("LOGSCOPE_"))
            .map(|(k, v)| format!("{k}={v}")).collect();
        set.sort();
        for s in set { r.push("ENV", s); }
        let out = r.render();
        assert!(out.contains("LOGSCOPE_TEST_MARKER"));
        assert!(!out.contains("shouldnotappear"));
        std::env::remove_var("LOGSCOPE_TEST_MARKER");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
    }
}
