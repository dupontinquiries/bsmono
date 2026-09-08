//! video_probe — asking the system which encoders it actually has.
//!
//! `video_backend::choose` and `unavailable_report` are pure over two closures — "is this
//! gstreamer element present", "does ffmpeg have this encoder" — so the priority order and
//! its reasoning can be tested without either tool installed. Nothing ever supplied those
//! closures, so both functions had no caller and the app could only say "no encoder linked
//! yet" without being able to say which.
//!
//! This is the layer that asks. Kept out of `video_backend` for the reason that module gives:
//! choosing a backend is a pure function of what is installed, running one is not.
//!
//! ## Asked once
//!
//! Each probe spawns a process. `gst-inspect-1.0` and `ffmpeg -encoders` are not fast, the
//! answer does not change while the app runs, and this is consulted from a UI path that draws
//! every frame — so the results are cached for the process's life. Installing an encoder
//! while the app is open needs a restart, which is a fair trade for not forking twice a frame.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn cache() -> &'static Mutex<HashMap<String, bool>> {
    static C: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Is a gstreamer element installed?
///
/// `gst-inspect-1.0 <name>` exits non-zero when the element is unknown, which is the whole
/// check. Missing `gst-inspect-1.0` is the same answer as a missing element — from the app's
/// side both mean "cannot use this backend".
pub fn gst_element(name: &str) -> bool {
    probe(&format!("gst:{name}"), || {
        run_ok("gst-inspect-1.0", &[name])
    })
}

/// Does ffmpeg have this encoder?
///
/// `ffmpeg -hide_banner -encoders` lists them; the name is matched as a WHOLE WORD in the
/// output. A substring match would find `libsvtav1` inside a longer name and, worse, find
/// `av1` inside half the table — and a backend chosen because of a substring fails at encode
/// time with an error nobody connects to this decision.
pub fn ffmpeg_encoder(name: &str) -> bool {
    probe(&format!("ffmpeg:{name}"), || {
        match run_out("ffmpeg", &["-hide_banner", "-encoders"]) {
            Some(out) => lists_encoder(&out, name),
            None => false,
        }
    })
}

/// Does `ffmpeg -encoders` output list this encoder?
///
/// Pure, so the parsing is testable without ffmpeg. The table's rows are
/// ` V....D libsvtav1  SVT-AV1(Scalable Video Technology for AV1) encoder`, so the name is a
/// whitespace-delimited field and matching it as one is what keeps `av1` from matching
/// `libaom-av1`.
pub fn lists_encoder(output: &str, name: &str) -> bool {
    // A name with no letter or digit in it is not an encoder. The LEGEND rows have the same
    // shape as the table's — ` V..... = Video` parses as flags `V.....` and name `=` — and a
    // parser that took the second field on trust reported an encoder called "=". Found by the
    // test, which is why it lists the legend as well as the rows.
    if !name.chars().any(|c| c.is_alphanumeric()) {
        return false;
    }
    output.lines().any(|l| {
        let mut f = l.split_whitespace();
        // The flags field is exactly six characters of the documented set. Requiring it
        // skips the header, the `------` rule and anything else that is not a row.
        let flags = f.next().unwrap_or("");
        if flags.len() != 6 || !flags.chars().all(|c| "VASFXBD.".contains(c)) {
            return false;
        }
        f.next().is_some_and(|n| n == name)
    })
}

fn probe(key: &str, f: impl FnOnce() -> bool) -> bool {
    if let Ok(c) = cache().lock() {
        if let Some(v) = c.get(key) {
            return *v;
        }
    }
    let v = f();
    if let Ok(mut c) = cache().lock() {
        c.insert(key.to_string(), v);
    }
    v
}

fn run_ok(bin: &str, args: &[&str]) -> bool {
    std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_out(bin: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(bin).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Which backend this machine can use, and — when none — why not, one line per backend.
///
/// The report is the point: "no encoder linked yet" is true and unactionable, while
/// "gstreamer element `av1enc` missing; ffmpeg encoder `libsvtav1` missing" tells someone
/// what to install.
pub fn status(vvc_opt_in: bool) -> Result<crate::video_backend::Backend, Vec<String>> {
    match crate::video_backend::choose(&gst_element, &ffmpeg_encoder, vvc_opt_in) {
        Some(b) => Ok(b),
        None => Err(crate::video_backend::unavailable_report(
            &gst_element,
            &ffmpeg_encoder,
            vvc_opt_in,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A substring match would find `av1` inside `libaom-av1` and half the rest of the table.
    /// A backend chosen on a substring fails at encode time with an error nobody connects
    /// back to this decision.
    #[test]
    fn an_encoder_name_is_matched_as_a_whole_field() {
        let table = "\
Encoders:
 V..... = Video
 ------
 V....D libaom-av1           libaom AV1
 V....D libsvtav1            SVT-AV1 encoder
 A....D aac                  AAC (Advanced Audio Coding)
";
        assert!(lists_encoder(table, "libsvtav1"));
        assert!(lists_encoder(table, "libaom-av1"));
        assert!(lists_encoder(table, "aac"));

        // The traps.
        assert!(!lists_encoder(table, "av1"), "a substring matched a whole encoder name");
        assert!(!lists_encoder(table, "svtav1"));
        assert!(!lists_encoder(table, "lib"));
        // The legend row's second field is not an encoder either.
        assert!(!lists_encoder(table, "="));
        assert!(!lists_encoder("", "libsvtav1"));
    }

    /// The report is the point: "no encoder linked yet" is true and unactionable, while a
    /// named missing element tells someone what to install.
    #[test]
    fn with_nothing_installed_every_backend_says_what_it_wanted() {
        let none = |_: &str| false;
        let report = crate::video_backend::unavailable_report(&none, &none, false);
        assert_eq!(report.len(), crate::video_backend::Backend::priority_order().len());
        assert!(report.iter().any(|l| l.contains("missing")), "{report:?}");
        // Every line names the backend AND the thing it needed — a list of backend names
        // alone would not tell anyone what to install.
        for line in &report {
            assert!(line.contains(':'), "{line}");
        }
        // VVC is off by opt-in rather than missing, and says so: "not installed" would send
        // someone hunting for a package they already have.
        assert!(report.iter().any(|l| l.contains("not enabled")), "{report:?}");
    }

    /// The cache is per-key, so one missing tool does not answer for another.
    #[test]
    fn probes_are_cached_per_name() {
        // Two names that cannot exist; both must come back false and independently.
        assert!(!gst_element("definitely-not-an-element-xyz"));
        assert!(!ffmpeg_encoder("definitely-not-an-encoder-xyz"));
        // Second call hits the cache and must agree.
        assert!(!gst_element("definitely-not-an-element-xyz"));
    }
}
