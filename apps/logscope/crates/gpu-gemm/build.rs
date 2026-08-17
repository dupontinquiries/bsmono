//! Stamp the build with its commit, so a log identifies the binary that produced it.
//!
//! Three times now a log has been read against the wrong build — a fix reported as not working when
//! the binary predated it, and a fix credited when it was already present. The engine prints thirty
//! lines of provenance about the MODEL and none about itself, and "did you rebuild" is not a
//! question a transcript should have to answer.
fn main() {
    let git = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"]).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"]).output().ok()
        .map(|o| !o.stdout.is_empty()).unwrap_or(false);
    println!("cargo:rustc-env=LOGSCOPE_BUILD={}{}", git, if dirty { "+dirty" } else { "" });
    // Re-stamp when the commit moves; without this the value sticks from the first build.
    println!("cargo:rerun-if-changed=../../../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../../../.git/index");
}
