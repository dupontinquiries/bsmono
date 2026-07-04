//! sysdeps — make sure the machine can actually build what we're asking it to.
//!
//! The commonest build failure on a fresh setup isn't the code — it's a missing
//! prerequisite: the cross target's `std` isn't installed (`can't find crate for std` →
//! "rustup target add …"), or a system library/tool (cmake, gcc, gtk) is absent. This module
//! auto-installs those the right way: rustup for Rust std targets here, and (via `pydeps`)
//! the distro package manager (dnf on Fedora, apk on Alpine) for system packages.

use std::path::Path;
use std::process::Command;

/// Parse `rustup target list --installed` output into the installed triples.
pub fn parse_installed_targets(stdout: &str) -> Vec<String> {
    stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// The command to add a rust std target (pure).
pub fn rustup_target_add_cmd(triple: &str) -> (String, Vec<String>) {
    ("rustup".to_string(), vec!["target".to_string(), "add".to_string(), triple.to_string()])
}

/// Is a rustup std target already installed?
pub fn rust_target_installed(triple: &str, path_env: &str) -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .env("PATH", path_env)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_installed_targets(&String::from_utf8_lossy(&o.stdout)).iter().any(|t| t == triple))
        .unwrap_or(false)
}

/// Ensure a rust std target is installed, adding it via rustup if missing. Returns
/// `Ok(true)` if it was just installed, `Ok(false)` if already present. This is what turns
/// "can't find crate for std … rustup target add aarch64-unknown-linux-musl" into a no-op:
/// gitmind runs the `rustup target add` for you before a cross-build.
pub fn ensure_rust_target(triple: &str, path_env: &str) -> Result<bool, String> {
    if triple.trim().is_empty() || rust_target_installed(triple, path_env) {
        return Ok(false);
    }
    let out = Command::new("rustup")
        .args(["target", "add", triple])
        .env("PATH", path_env)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "rustup isn't installed, so the cross target's std can't be added. Install rustup (https://rustup.rs) or add the target manually.".to_string()
            } else {
                format!("rustup: {e}")
            }
        })?;
    if out.status.success() {
        Ok(true)
    } else {
        Err(format!("rustup target add {triple} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// A base set of build essentials per package manager (compiler, make, cmake, pkg-config,
/// openssl headers) — enough for most Rust+C / CMake projects.
pub fn build_essential_packages(mgr: crate::pydeps::PkgManager) -> Vec<String> {
    use crate::pydeps::PkgManager::*;
    let list: &[&str] = match mgr {
        Dnf | Zypper => &["gcc", "gcc-c++", "make", "cmake", "pkgconf-pkg-config", "openssl-devel"],
        Apk => &["build-base", "cmake", "pkgconf", "openssl-dev"],
        Apt => &["build-essential", "cmake", "pkg-config", "libssl-dev"],
        Pacman => &["base-devel", "cmake", "pkgconf", "openssl"],
        _ => &["cmake"],
    };
    list.iter().map(|s| s.to_string()).collect()
}

/// Install system build essentials via the distro package manager (dnf/apk/…), escalating
/// privileges as the host allows. `extra` are additional package names (e.g. a project's
/// gtk4-devel / SDL2-devel).
pub fn ensure_build_essentials(extra: &[String]) -> Result<String, String> {
    let path_env = crate::ops::tool_path_env();
    let mgr = crate::pydeps::detect_pkg_manager(&path_env);
    if matches!(mgr, crate::pydeps::PkgManager::None) {
        return Err("no supported package manager found (dnf/apk/apt/pacman)".to_string());
    }
    let mut pkgs = build_essential_packages(mgr);
    pkgs.extend(extra.iter().cloned());
    if crate::pydeps::install_system(mgr, &pkgs, &path_env) {
        Ok(format!("installed build essentials via {}", mgr.name()))
    } else {
        Err(format!("couldn't install build essentials via {} — need root, or install manually: {}", mgr.name(), pkgs.join(" ")))
    }
}

/// Detect a CMake project needing a cross toolchain file (informational — the file itself is
/// project-specific and must exist on-device).
pub fn cmake_needs_toolchain_file(app_dir: &Path, target: &str, native: &str) -> bool {
    !target.trim().is_empty() && target != native && app_dir.join("CMakeLists.txt").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_installed_targets_works() {
        let out = "aarch64-unknown-linux-gnu (installed)\naarch64-unknown-linux-musl\n\nx86_64-unknown-linux-gnu\n";
        let t = parse_installed_targets(out);
        assert!(t.iter().any(|x| x.contains("musl")));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn target_add_cmd_is_correct() {
        let (prog, args) = rustup_target_add_cmd("aarch64-unknown-linux-musl");
        assert_eq!(prog, "rustup");
        assert_eq!(args, vec!["target", "add", "aarch64-unknown-linux-musl"]);
    }

    #[test]
    fn ensure_native_target_is_noop() {
        // empty triple = native build → nothing to install, never errors
        assert_eq!(ensure_rust_target("", "/usr/bin").unwrap(), false);
    }

    #[test]
    fn cmake_cross_detection() {
        let dir = std::env::temp_dir();
        // no CMakeLists in temp dir → false regardless
        assert!(!cmake_needs_toolchain_file(&dir, "aarch64-unknown-linux-musl", "x86_64-unknown-linux-gnu"));
    }
}
