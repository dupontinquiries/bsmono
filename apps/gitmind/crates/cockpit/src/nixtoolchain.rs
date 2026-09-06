//! nixtoolchain — cross-compiling when the toolchain came from Nix rather than rustup.
//!
//! Three assumptions run through the build path, and all three are false on a Nix machine:
//!
//! 1. **`rustup target add` installs a std for the target.** With rust from nixpkgs (or a
//!    `fenix` / `oxalica` overlay) there may be no rustup at all — and the worse case is
//!    when there IS one, because `rustup target add` then succeeds and adds a target to a
//!    toolchain that is not the `cargo` on PATH. The build still fails with `can't find
//!    crate for std`, and the one diagnostic that would have explained it has just
//!    reported success.
//! 2. **The cross linker is called `aarch64-linux-gnu-gcc`.** Nix's cross compilers come
//!    from `pkgsCross.<name>.stdenv.cc` and are prefixed with the full target triple —
//!    `aarch64-unknown-linux-gnu-gcc` — and exist only inside the shell that provides them.
//! 3. **Missing system libraries can be installed.** `ensure_build_essentials` shells out
//!    to dnf/apk/apt/pacman. NixOS has none of them, and installing into the system
//!    imperatively is not how the machine works. The right output is not an install: it is
//!    the lines to add to a `devShell`.
//!
//! So: detect, then adapt. Everything here is pure — detection takes the environment as
//! arguments and probing takes a closure — because the point is to be able to test the
//! Nix path on a machine that does not have Nix.

/// Where the Rust toolchain came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    /// rustup manages it: `rustup target add` is the right move.
    Rustup,
    /// Nix provides it (a store path, or a nix-shell). Targets come from the flake.
    Nix,
    /// A distro package, or something else. Neither assumption is safe.
    System,
}

/// Detect from the environment. `cargo_path` is the resolved cargo binary; `in_nix_shell`
/// is `$IN_NIX_SHELL`; `rustup_home` is `$RUSTUP_HOME` and `home` is `$HOME`.
///
/// The store path is checked FIRST, before rustup's own markers: a machine can have both,
/// and what matters is which cargo is about to run, not which tools are installed.
pub fn detect(cargo_path: &str, in_nix_shell: Option<&str>, rustup_home: Option<&str>, home: Option<&str>) -> Toolchain {
    if cargo_path.starts_with("/nix/store/") {
        return Toolchain::Nix;
    }
    if in_nix_shell.is_some_and(|v| !v.trim().is_empty()) {
        return Toolchain::Nix;
    }
    if let Some(rh) = rustup_home.filter(|r| !r.trim().is_empty()) {
        if cargo_path.starts_with(rh) {
            return Toolchain::Rustup;
        }
    }
    if let Some(h) = home.filter(|h| !h.trim().is_empty()) {
        if cargo_path.starts_with(&format!("{h}/.cargo/")) || cargo_path.starts_with(&format!("{h}/.rustup/")) {
            return Toolchain::Rustup;
        }
    }
    Toolchain::System
}

/// Detect from this process's environment.
pub fn detect_here(cargo_path: &str) -> Toolchain {
    detect(
        cargo_path,
        std::env::var("IN_NIX_SHELL").ok().as_deref(),
        std::env::var("RUSTUP_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Linker binaries that could serve `triple`, best first.
///
/// The full-triple form is first because that is what Nix's `pkgsCross` produces and what a
/// cross toolchain built from source usually installs; the short Debian/Fedora name is
/// second. Trying both is strictly better than picking one: a wrong linker fails at link
/// time with an error about object formats, which reads like a code problem.
pub fn linker_candidates(triple: &str) -> Vec<String> {
    let mut v = vec![format!("{triple}-gcc"), format!("{triple}-cc")];
    match triple {
        "aarch64-unknown-linux-gnu" => {
            v.push("aarch64-linux-gnu-gcc".into());
            v.push("aarch64-none-linux-gnu-gcc".into());
        }
        "aarch64-unknown-linux-musl" => {
            v.push("aarch64-linux-musl-gcc".into());
            v.push("aarch64-unknown-linux-musl-cc".into());
        }
        "armv7-unknown-linux-gnueabihf" => {
            v.push("arm-linux-gnueabihf-gcc".into());
        }
        "x86_64-unknown-linux-musl" => {
            v.push("x86_64-linux-musl-gcc".into());
            v.push("musl-gcc".into());
        }
        _ => {}
    }
    v
}

/// The first candidate `present` says exists. `None` means none of them do, which is a
/// real answer: the caller should say which names it looked for rather than guess.
pub fn pick_linker(triple: &str, present: impl Fn(&str) -> bool) -> Option<String> {
    linker_candidates(triple).into_iter().find(|c| present(c))
}

/// Is `name` an executable on this `PATH`? (`PATH` passed in so it is the same one the
/// build will use, not this process's.)
pub fn on_path(name: &str, path_env: &str) -> bool {
    path_env.split(':').filter(|p| !p.is_empty()).any(|dir| {
        let p = std::path::Path::new(dir).join(name);
        p.is_file() || p.is_symlink()
    })
}

/// The nixpkgs `pkgsCross` attribute for a triple, when nixpkgs has one.
pub fn cross_attr(triple: &str) -> Option<&'static str> {
    match triple {
        "aarch64-unknown-linux-gnu" => Some("aarch64-multiplatform"),
        "aarch64-unknown-linux-musl" => Some("aarch64-multiplatform-musl"),
        "armv7-unknown-linux-gnueabihf" => Some("armv7l-hf-multiplatform"),
        "x86_64-unknown-linux-musl" => Some("musl64"),
        _ => None,
    }
}

/// What to put in a `flake.nix` devShell so this cross-build works, as text to paste.
///
/// A hint rather than an action, and deliberately so: gitmind installing packages
/// imperatively is exactly what a Nix machine exists to avoid, and a devShell edit is
/// reproducible where an `apk add` is not.
pub fn devshell_hint(triple: &str, extra_pkgs: &[String]) -> String {
    let mut out = String::from(
        "this toolchain came from Nix, so gitmind will not install anything. Add the \
         target to your devShell:\n\n",
    );
    match cross_attr(triple) {
        Some(attr) => {
            out.push_str(&format!(
                "  # flake.nix — the cross compiler and a rustc that has {triple}'s std\n\
                 devShells.default = pkgs.mkShell {{\n\
                 \x20 nativeBuildInputs = [\n\
                 \x20   pkgs.pkgsCross.{attr}.stdenv.cc\n\
                 \x20   (pkgs.rust-bin.stable.latest.default.override {{\n\
                 \x20     targets = [ \"{triple}\" ];\n\
                 \x20   }})\n"
            ));
            for p in extra_pkgs {
                out.push_str(&format!("\x20   pkgs.{p}\n"));
            }
            out.push_str(
                "\x20 ];\n\
                 \x20 # cargo reads this; the binary is prefixed with the full triple\n",
            );
            out.push_str(&format!(
                "\x20 CARGO_TARGET_{}_LINKER = \"{triple}-gcc\";\n}};\n",
                triple.to_uppercase().replace('-', "_")
            ));
            out.push_str(
                "\n(`rust-bin` is the oxalica overlay; with plain nixpkgs use \
                 `pkgs.rustPlatform` and a toolchain that already has the target.)\n",
            );
        }
        None => {
            out.push_str(&format!(
                "  # nixpkgs has no pkgsCross attribute for {triple} that gitmind knows.\n\
                 \x20 # Add a cross stdenv for it and a rustc carrying its std, then set\n\
                 \x20 # CARGO_TARGET_{}_LINKER to the linker's name.\n",
                triple.to_uppercase().replace('-', "_")
            ));
        }
    }
    out
}

/// Why a cross-build cannot proceed on this machine, with what to do about it.
///
/// `None` when nothing is in the way.
pub fn blocker(
    toolchain: Toolchain,
    triple: &str,
    std_present: bool,
    linker: Option<&str>,
) -> Option<String> {
    if triple.trim().is_empty() {
        return None; // a native build needs neither
    }
    match (toolchain, std_present, linker) {
        (_, true, Some(_)) => None,
        (Toolchain::Nix, false, _) => Some(format!(
            "the Rust toolchain came from Nix and has no std for {triple}. `rustup target \
             add` would not help — it manages a different toolchain from the cargo that is \
             about to run, and on a machine with both it SUCCEEDS and changes nothing, \
             which is why this says so instead of trying.\n\n{}",
            devshell_hint(triple, &[])
        )),
        (Toolchain::Nix, true, None) => Some(format!(
            "no cross linker for {triple} on PATH. Looked for: {}.\n\n{}",
            linker_candidates(triple).join(", "),
            devshell_hint(triple, &[])
        )),
        (_, false, _) => Some(format!(
            "no std for {triple}. Run `rustup target add {triple}` (gitmind does this for \
             you when rustup owns the toolchain)."
        )),
        (_, true, None) => Some(format!(
            "no cross linker for {triple} on PATH. Looked for: {}. Install a cross \
             toolchain for the target, or set CARGO_TARGET_{}_LINKER to one you have.",
            linker_candidates(triple).join(", "),
            triple.to_uppercase().replace('-', "_")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_path_is_nix_whatever_else_is_installed() {
        // The case that matters: rustup IS installed, and the cargo about to run is Nix's.
        // Answering "Rustup" here is what makes `rustup target add` succeed and change
        // nothing.
        assert_eq!(
            detect("/nix/store/abc-cargo-1.90/bin/cargo", None, Some("/home/k/.rustup"), Some("/home/k")),
            Toolchain::Nix
        );
        assert_eq!(detect("/home/k/.cargo/bin/cargo", None, None, Some("/home/k")), Toolchain::Rustup);
        assert_eq!(detect("/home/k/.cargo/bin/cargo", Some("1"), None, Some("/home/k")), Toolchain::Nix, "inside a nix shell");
        assert_eq!(detect("/home/k/.cargo/bin/cargo", Some(""), None, Some("/home/k")), Toolchain::Rustup, "an empty IN_NIX_SHELL is not a shell");
        assert_eq!(detect("/usr/bin/cargo", None, None, Some("/home/k")), Toolchain::System);
        assert_eq!(detect("cargo", None, None, None), Toolchain::System, "an unresolved name says nothing");
    }

    #[test]
    fn the_full_triple_linker_is_tried_before_the_debian_name() {
        let c = linker_candidates("aarch64-unknown-linux-gnu");
        assert_eq!(c[0], "aarch64-unknown-linux-gnu-gcc", "what pkgsCross produces");
        assert!(c.contains(&"aarch64-linux-gnu-gcc".to_string()), "what Debian ships");
        // picked by what is actually there, not by what the distro usually calls it
        let only_debian = |n: &str| n == "aarch64-linux-gnu-gcc";
        assert_eq!(pick_linker("aarch64-unknown-linux-gnu", only_debian).as_deref(), Some("aarch64-linux-gnu-gcc"));
        let only_nix = |n: &str| n == "aarch64-unknown-linux-gnu-gcc";
        assert_eq!(pick_linker("aarch64-unknown-linux-gnu", only_nix).as_deref(), Some("aarch64-unknown-linux-gnu-gcc"));
        assert_eq!(pick_linker("aarch64-unknown-linux-gnu", |_| false), None, "none present is an answer, not a guess");
    }

    #[test]
    fn the_devshell_hint_names_the_cross_attribute_and_the_linker_variable() {
        let h = devshell_hint("aarch64-unknown-linux-musl", &["pkg-config".into()]);
        assert!(h.contains("pkgsCross.aarch64-multiplatform-musl.stdenv.cc"), "{h}");
        assert!(h.contains("targets = [ \"aarch64-unknown-linux-musl\" ]"), "{h}");
        assert!(h.contains("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER"), "{h}");
        assert!(h.contains("pkgs.pkg-config"), "extra packages are listed: {h}");
        // an unknown triple says so rather than inventing an attribute
        let u = devshell_hint("riscv64-unknown-linux-gnu", &[]);
        assert!(u.contains("no pkgsCross attribute") && !u.contains("pkgsCross."), "{u}");
    }

    /// Under Nix the missing-std message must NOT tell the operator to run rustup: on a
    /// machine with both, that command succeeds and the build fails exactly as before.
    #[test]
    fn the_nix_blocker_does_not_send_you_to_rustup() {
        let nix = blocker(Toolchain::Nix, "aarch64-unknown-linux-musl", false, None).unwrap();
        assert!(nix.contains("devShell") && nix.contains("pkgsCross"), "{nix}");
        assert!(!nix.contains("rustup target add aarch64"), "it recommended the command that does nothing: {nix}");

        let rustup = blocker(Toolchain::Rustup, "aarch64-unknown-linux-musl", false, None).unwrap();
        assert!(rustup.contains("rustup target add aarch64-unknown-linux-musl"), "{rustup}");

        // std present, linker missing: both toolchains say which names were tried
        let no_linker = blocker(Toolchain::System, "aarch64-unknown-linux-gnu", true, None).unwrap();
        assert!(no_linker.contains("aarch64-unknown-linux-gnu-gcc") && no_linker.contains("aarch64-linux-gnu-gcc"));

        // ready, and a native build is never blocked
        assert!(blocker(Toolchain::Nix, "aarch64-unknown-linux-musl", true, Some("cc")).is_none());
        assert!(blocker(Toolchain::Nix, "", false, None).is_none());
    }

    #[test]
    fn on_path_reads_the_path_it_is_given() {
        let dir = std::env::temp_dir().join(format!("gm-nix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("aarch64-unknown-linux-gnu-gcc");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        let path = format!("/nonexistent:{}", dir.display());
        assert!(on_path("aarch64-unknown-linux-gnu-gcc", &path));
        assert!(!on_path("aarch64-linux-gnu-gcc", &path));
        assert!(!on_path("anything", ""), "an empty PATH has nothing on it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
