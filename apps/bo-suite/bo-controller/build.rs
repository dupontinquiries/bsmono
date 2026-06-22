// build.rs
//
// qmetaobject 0.2 setup + build-stamp emitter + QML pre-flight checks.
//
// qmetaobject's own build script (pulled in as a build-dependency)
// handles cpp! closure compilation + Qt discovery via qmake — so we
// don't need cpp_build calls here. What we DO need:
//
//   1. Tell Cargo to rebuild when QML/assets change (dev convenience).
//   2. Emit BO_BUILD_STAMP so the running binary can log which build
//      it is.
//   3. Run two QML pre-flight checks that fail the build on issue:
//      a. qmllint (syntax + Connections binding shape)
//      b. property-existence cross-reference (catches the
//         `MixerPage { accent: ... }` class of bug that Qt5 qmllint
//         doesn't surface)
//
// The property-existence checker lives in src/qml_lint.rs so it can
// be exercised by `cargo test` too. We include! it here so the same
// code runs at build time without a circular dep.

// The QML linter module — included via path so both build.rs and the
// regular `cargo test` compilation can share one definition.
include!("src/qml_lint.rs");

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=qml/");
    println!("cargo:rerun-if-changed=assets/");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/qml_lint.rs");
    // Force a re-stamp on every build by depending on a non-existent
    // file. Cargo's "rerun-if-changed" with a missing path always
    // triggers (the file's mtime is "never"), which means every
    // `cargo build` re-runs this script and the stamp reflects the
    // *current* build time, not the time build.rs itself last changed.
    println!("cargo:rerun-if-changed=NONEXISTENT_REBUILD_TRIGGER");

    if std::env::var("BO_SKIP_QML_LINT").as_deref() != Ok("1") {
        run_qml_lint();
        run_qml_property_check();
    }

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = format_unix_utc(secs);
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=BO_BUILD_STAMP={} ({})", stamp, target);
}

/// Pass 1: run Qt's `qmllint` for syntax issues + Connections
/// binding-shape verification. Skipped (with warning) when qmllint
/// isn't installed.
fn run_qml_lint() {
    use std::process::Command;
    let bin = ["qmllint6", "qmllint"]
        .iter()
        .find(|name| {
            Command::new(name).arg("--version").output()
                .map(|o| o.status.success() || !o.stderr.is_empty())
                .unwrap_or(false)
        });
    let Some(bin) = bin else {
        println!("cargo:warning=qmllint not found — syntax pre-flight skipped \
                  (install qt6-base-dev or qt5-qttools to enable)");
        return;
    };
    let qml_dir = std::path::Path::new("qml");
    let entries = match std::fs::read_dir(qml_dir) {
        Ok(e) => e,
        Err(e) => {
            println!("cargo:warning=qml/ unreadable for lint: {} (skipping)", e);
            return;
        }
    };
    let mut failures: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("qml") {
            continue;
        }
        let basename = path.file_name()
            .and_then(|n| n.to_str()).unwrap_or("");
        if basename == "qmldir" { continue; }

        let out = Command::new(bin)
            .args(["-I", "qml"])
            .arg(&path)
            .output();
        let Ok(out) = out else { continue; };
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        let has_issue = combined.lines().any(|line| {
            line.starts_with("Error")
                || line.starts_with("Warning")
                || line.starts_with("[Warning]")
                || line.starts_with("[Error]")
                || line.contains(": Cannot assign")
                || line.contains(": Connections: Detected function")
                || line.contains("Unable to assign")
        });
        if has_issue {
            failures.push((basename.to_string(), combined));
        }
    }
    if !failures.is_empty() {
        for (name, out) in &failures {
            println!("cargo:warning=qmllint issue in {}", name);
            for line in out.lines().take(8) {
                if !line.trim().is_empty() {
                    println!("cargo:warning=    {}", line);
                }
            }
        }
        panic!(
            "{} QML file(s) failed qmllint — fix above or set \
             BO_SKIP_QML_LINT=1 to bypass.",
            failures.len()
        );
    }
}

/// Pass 2: cross-reference every component instantiation against the
/// target component's declared properties. Catches the exact bug
/// class that Qt5 qmllint misses:
///
///     MixerPage { accent: ... }    when MixerPage doesn't have accent
fn run_qml_property_check() {
    let qml_dir = std::path::Path::new("qml");
    let issues = match check_qml_properties(qml_dir) {
        Ok(v) => v,
        Err(e) => {
            println!("cargo:warning=qml property check skipped: {}", e);
            return;
        }
    };
    if !issues.is_empty() {
        for issue in &issues {
            println!(
                "cargo:warning={}:{}: assignment to undeclared property \
                 '{}' on {}",
                issue.source_file, issue.line_number,
                issue.bad_property, issue.component_type,
            );
        }
        panic!(
            "{} undeclared-property assignment(s) in QML — see warnings \
             above. (set BO_SKIP_QML_LINT=1 to bypass, but this catches \
             real runtime breakage.)",
            issues.len(),
        );
    }
}

/// Convert Unix-epoch seconds to "YYYY-MM-DD HH:MM:SS UTC" without
/// pulling chrono just for a banner string. Uses Howard Hinnant's
/// civil_from_days algorithm for the date breakdown.
fn format_unix_utc(secs: u64) -> String {
    let s = (secs % 60) as u32;
    let m = ((secs / 60) % 60) as u32;
    let h = ((secs / 3600) % 24) as u32;
    let days = secs / 86_400;

    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_civ = if mp < 10 { mp + 3 } else { mp - 9 };
    let y_civ = y + if m_civ <= 2 { 1 } else { 0 };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            y_civ, m_civ, d, h, m, s)
}
