//! App-tile previews. After a build, the binary is right here, so we can launch it
//! on an offscreen X display (Xvfb), let it settle ~1s, and grab a 1000x1000 square
//! as the app's tile. Headless apps render nothing — the grab comes back near-
//! uniform, which we detect and drop so the GUI falls back to the hash monogram.
//!
//! The launch + grab run on-device (need Xvfb + ImageMagick); the cache paths and
//! the command construction here are pure and unit-tested.

use std::path::PathBuf;

/// `~/.cache/gitmind/shots`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(base).join(".cache/gitmind/shots")
}

/// First 10 chars of an oid (enough to key a build).
pub fn oid_short(oid: &str) -> String {
    oid.chars().take(10).collect()
}

/// Where an app's screenshot for a given build is cached — keyed by app + oid so a
/// rebuilt commit gets a fresh tile and old ones stay around. Pure — unit-tested.
pub fn shot_path(app: &str, oid: &str) -> PathBuf {
    let key = if oid.trim().is_empty() {
        app.to_string()
    } else {
        format!("{app}-{}", oid_short(oid))
    };
    cache_dir().join(format!("{key}.png"))
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The capture pipeline as a portable shell script. Pure — unit-tested (structure).
/// Runs locally where the binary was just built:
///  - Xvfb offscreen at 1000x1000 (so the grab is a square by construction),
///  - launch the binary under the xcb platform with stdin from /dev/null,
///  - 1s idle, grab the root window,
///  - drop it if the frame is near-uniform (a headless app drew nothing),
///  - print `shot:<path>` or `noshot`, and always clean up the display + process.
pub fn capture_script(binary: &str, out: &str) -> String {
    let bin = q(binary);
    let outq = q(out);
    format!(
        "D=\":$(( ($$ % 60) + 100 ))\"\n\
         out={outq}\n\
         Xvfb \"$D\" -screen 0 1000x1000x24 >/dev/null 2>&1 &\n\
         XV=$!\n\
         sleep 0.5\n\
         DISPLAY=\"$D\" QT_QPA_PLATFORM=xcb {bin} </dev/null >/dev/null 2>&1 &\n\
         AP=$!\n\
         sleep 1\n\
         DISPLAY=\"$D\" import -window root \"$out\" >/dev/null 2>&1 || true\n\
         kill \"$AP\" >/dev/null 2>&1 || true\n\
         kill \"$XV\" >/dev/null 2>&1 || true\n\
         if [ -s \"$out\" ]; then \
           sd=$(convert \"$out\" -format '%[fx:standard_deviation]' info: 2>/dev/null || echo 0); \
           if awk \"BEGIN{{exit !($sd<0.02)}}\"; then rm -f \"$out\"; echo noshot; \
           else echo \"shot:$out\"; fi; \
         else echo noshot; fi"
    )
}

/// Parse the script's last line into the cached path (or None for a headless app).
/// Pure — unit-tested.
pub fn parse_capture(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("shot:").map(|p| p.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shot_path_keyed_by_app_and_build() {
        std::env::set_var("HOME", "/home/kesslerd");
        let p = shot_path("voiceterm", "4a9f1c7e2b8d6035af19c4e7d2b0a6f3e8c1d574");
        assert!(p.ends_with("voiceterm-4a9f1c7e2b.png"), "{}", p.display());
        assert!(p.to_string_lossy().contains(".cache/gitmind/shots"));
        // no oid → just the app name
        assert!(shot_path("photosync", "").ends_with("photosync.png"));
    }

    #[test]
    fn capture_script_is_offscreen_and_guards_blanks() {
        let s = capture_script("target/release/voice term", "/c/shots/v.png");
        assert!(s.contains("Xvfb") && s.contains("1000x1000x24"), "offscreen square");
        assert!(s.contains("QT_QPA_PLATFORM=xcb"), "qt under xcb");
        assert!(s.contains("import -window root"), "grab");
        assert!(s.contains("standard_deviation") && s.contains("noshot"), "blank guard");
        assert!(s.contains("'target/release/voice term'"), "binary quoted (space-safe)");
        assert!(s.contains("kill \"$AP\"") && s.contains("kill \"$XV\""), "always cleans up");
    }

    #[test]
    fn parse_capture_reads_result() {
        assert_eq!(parse_capture("warn\nshot:/c/shots/v.png").as_deref(), Some("/c/shots/v.png"));
        assert_eq!(parse_capture("noshot"), None);
        assert_eq!(parse_capture(""), None);
    }
}
