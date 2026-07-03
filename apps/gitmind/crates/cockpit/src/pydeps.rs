//! Cross-distro Python dependency coverage. A venv installs Python packages, but some
//! (pygame above all) need *system* libraries that differ per distro — sdl2-dev on Alpine,
//! SDL2-devel on Fedora, libsdl2-dev on Debian — and on musl (Alpine/postmarketOS) there's
//! often no wheel, so the package compiles from source and needs those dev headers present.
//!
//! Strategy is an array of fallbacks, built into the build:
//!   1. try pip with prebuilt wheels first (works on glibc: Fedora, Ubuntu, …);
//!   2. if that fails, install the mapped system libraries via the detected package
//!      manager (apk / dnf / apt / pacman / zypper / brew), then build from source;
//!   3. if that can't be done (no privilege / unknown distro), fail with the exact
//!      command to run — including a nix suggestion when nix is present.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PkgManager { Apk, Dnf, Apt, Pacman, Zypper, Brew, Nix, None }

impl PkgManager {
    pub fn name(&self) -> &'static str {
        match self {
            PkgManager::Apk => "apk", PkgManager::Dnf => "dnf", PkgManager::Apt => "apt",
            PkgManager::Pacman => "pacman", PkgManager::Zypper => "zypper", PkgManager::Brew => "brew",
            PkgManager::Nix => "nix", PkgManager::None => "none",
        }
    }
    /// The base install command (no privilege prefix). None for Nix/None.
    pub fn install_argv(&self, pkgs: &[String]) -> Option<Vec<String>> {
        let head: Vec<&str> = match self {
            PkgManager::Apk => vec!["apk", "add", "--no-cache"],
            PkgManager::Dnf => vec!["dnf", "install", "-y"],
            PkgManager::Apt => vec!["apt-get", "install", "-y", "--no-install-recommends"],
            PkgManager::Pacman => vec!["pacman", "-S", "--needed", "--noconfirm"],
            PkgManager::Zypper => vec!["zypper", "--non-interactive", "install"],
            PkgManager::Brew => vec!["brew", "install"],
            PkgManager::Nix | PkgManager::None => return None,
        };
        let mut a: Vec<String> = head.into_iter().map(String::from).collect();
        a.extend(pkgs.iter().cloned());
        Some(a)
    }
}

fn which(cmd: &str, path_env: &str) -> bool {
    path_env.split(':').any(|d| !d.is_empty() && Path::new(d).join(cmd).exists())
}

/// Detect the system package manager available on this device.
pub fn detect_pkg_manager(path_env: &str) -> PkgManager {
    for (cmd, mgr) in [
        ("apk", PkgManager::Apk), ("dnf", PkgManager::Dnf), ("apt-get", PkgManager::Apt),
        ("pacman", PkgManager::Pacman), ("zypper", PkgManager::Zypper), ("brew", PkgManager::Brew),
        ("nix-env", PkgManager::Nix),
    ] {
        if which(cmd, path_env) { return mgr; }
    }
    PkgManager::None
}

/// System dev packages a given pip package needs, for the given manager. Empty when the
/// package is pure-Python or ships a self-contained wheel. Keyed on the common cases that
/// actually need system libraries.
pub fn system_deps(pip_pkg: &str, mgr: PkgManager) -> Vec<String> {
    let pick = |apk: &[&str], dnf: &[&str], apt: &[&str], pacman: &[&str]| -> Vec<String> {
        let list: &[&str] = match mgr {
            PkgManager::Apk => apk,
            PkgManager::Dnf | PkgManager::Zypper => dnf,
            PkgManager::Apt => apt,
            PkgManager::Pacman => pacman,
            _ => &[],
        };
        list.iter().map(|s| s.to_string()).collect()
    };
    match pip_pkg.to_lowercase().as_str() {
        "pygame" | "pygame-ce" => pick(
            &["python3-dev", "gcc", "musl-dev", "sdl2-dev", "sdl2_image-dev", "sdl2_mixer-dev", "sdl2_ttf-dev", "freetype-dev", "portmidi-dev"],
            &["python3-devel", "gcc", "SDL2-devel", "SDL2_image-devel", "SDL2_mixer-devel", "SDL2_ttf-devel", "freetype-devel", "portmidi-devel"],
            &["python3-dev", "gcc", "libsdl2-dev", "libsdl2-image-dev", "libsdl2-mixer-dev", "libsdl2-ttf-dev", "libfreetype6-dev", "libportmidi-dev"],
            &["python", "gcc", "sdl2", "sdl2_image", "sdl2_mixer", "sdl2_ttf", "freetype2", "portmidi"],
        ),
        "pillow" => pick(
            &["jpeg-dev", "zlib-dev", "freetype-dev"],
            &["libjpeg-turbo-devel", "zlib-devel", "freetype-devel"],
            &["libjpeg-dev", "zlib1g-dev", "libfreetype6-dev"],
            &["libjpeg-turbo", "zlib", "freetype2"],
        ),
        "pyaudio" => pick(&["portaudio-dev"], &["portaudio-devel"], &["portaudio19-dev"], &["portaudio"]),
        "cryptography" => pick(
            &["openssl-dev", "libffi-dev", "cargo"],
            &["openssl-devel", "libffi-devel", "cargo"],
            &["libssl-dev", "libffi-dev", "cargo"],
            &["openssl", "libffi", "rust"],
        ),
        "lxml" => pick(&["libxml2-dev", "libxslt-dev"], &["libxml2-devel", "libxslt-devel"], &["libxml2-dev", "libxslt1-dev"], &["libxml2", "libxslt"]),
        "numpy" | "scipy" => pick(&["openblas-dev", "gfortran"], &["openblas-devel", "gcc-gfortran"], &["libopenblas-dev", "gfortran"], &["openblas", "gcc-fortran"]),
        _ => vec![],
    }
}

fn run_ok(argv: &[String], path_env: &str) -> bool {
    argv.first().map(|cmd| {
        Command::new(cmd).args(&argv[1..]).env("PATH", path_env)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output().map(|o| o.status.success()).unwrap_or(false)
    }).unwrap_or(false)
}

/// Try to install system packages, escalating privilege only if needed. Best-effort:
/// returns whether it succeeded. Tries as-is (root), then `sudo -n`, then `doas`.
pub fn install_system(mgr: PkgManager, pkgs: &[String], path_env: &str) -> bool {
    if pkgs.is_empty() { return true; }
    let Some(base) = mgr.install_argv(pkgs) else { return false };
    let mut prefixes: Vec<Vec<String>> = vec![vec![]];
    if which("sudo", path_env) { prefixes.push(vec!["sudo".into(), "-n".into()]); }
    if which("doas", path_env) { prefixes.push(vec!["doas".into()]); }
    for pre in prefixes {
        let mut argv = pre;
        argv.extend(base.clone());
        if run_ok(&argv, path_env) { return true; }
    }
    false
}

/// A copy-pasteable install command for the person to run by hand, when automatic install
/// isn't possible. Includes a nix hint if nothing else is available.
pub fn install_hint(mgr: PkgManager, pkgs: &[String]) -> String {
    if pkgs.is_empty() { return String::new(); }
    match mgr.install_argv(pkgs) {
        Some(argv) => format!("sudo {}", argv.join(" ")),
        None => {
            let names: Vec<String> = pkgs.iter().map(|p| p.trim_end_matches("-dev").trim_end_matches("-devel").to_string()).collect();
            format!("(no known package manager) with nix: nix-shell -p python3 {}", names.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_pygame_system_deps_per_distro() {
        assert!(system_deps("pygame", PkgManager::Apk).contains(&"sdl2-dev".to_string()));
        assert!(system_deps("pygame", PkgManager::Dnf).contains(&"SDL2-devel".to_string()));
        assert!(system_deps("pygame", PkgManager::Apt).contains(&"libsdl2-dev".to_string()));
        assert!(system_deps("pygame", PkgManager::Pacman).contains(&"sdl2".to_string()));
        // musl needs a compiler + musl-dev to build from source
        assert!(system_deps("pygame", PkgManager::Apk).contains(&"musl-dev".to_string()));
        // pure-python / wheel-only packages need nothing
        assert!(system_deps("requests", PkgManager::Apt).is_empty());
    }

    #[test]
    fn detects_this_box_and_builds_commands() {
        let path = std::env::var("PATH").unwrap_or_default();
        let mgr = detect_pkg_manager(&path);
        assert_ne!(mgr, PkgManager::None, "the sandbox has a package manager");
        let argv = mgr.install_argv(&["libsdl2-dev".into()]).unwrap();
        assert_eq!(argv[0], mgr.name().replace("apt", "apt-get"), "manager command leads the argv");
        let hint = install_hint(PkgManager::Apk, &system_deps("pygame", PkgManager::Apk));
        assert!(hint.contains("apk add") && hint.contains("sdl2-dev"));
    }

    #[test]
    fn install_system_runs_the_manager() {
        // real, cheap proof the install path executes: (re)install an already-present pkg
        let path = std::env::var("PATH").unwrap_or_default();
        if detect_pkg_manager(&path) == PkgManager::Apt {
            assert!(install_system(PkgManager::Apt, &["ca-certificates".into()], &path), "apt install of a present pkg succeeds");
        }
    }
}
