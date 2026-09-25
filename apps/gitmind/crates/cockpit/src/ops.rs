//! Cross-cutting operations shared by every frontend: resolving cargo, building locally
//! (native vs cross, shared target cache), target detection, gitmind-root discovery,
//! gitignore upkeep, and fleet-user provisioning. Frontend-agnostic — no Qt, no I/O
//! beyond std::process.

use std::path::{Path, PathBuf};

/// The commit oid of the last change touching an app's directory — the seed for its tile
/// colour. Empty if not in a git repo or git isn't available.
pub fn app_oid(root: &Path, dir: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C").arg(root)
        .args(["log", "-1", "--format=%H", "--"])
        .arg(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Resolve the cargo binary even when launched from a desktop launcher (whose PATH
/// usually omits ~/.cargo/bin): honour $CARGO, then ~/.cargo/bin/cargo, then the name.
pub fn cargo_bin() -> String {
    if let Ok(c) = std::env::var("CARGO") {
        if !c.trim().is_empty() {
            return c;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let cand = format!("{home}/.cargo/bin/cargo");
        if std::path::Path::new(&cand).is_file() {
            return cand;
        }
    }
    "cargo".to_string()
}

/// The toolchain name in `rustup show active-toolchain`'s reply: its first
/// token. Current rustup prints `stable-aarch64-unknown-linux-gnu (default)` or
/// `1.96.0-x86_64-unknown-linux-gnu (overridden by '/path/rust-toolchain.toml')`;
/// `info:` / `error:` lines and blanks are skipped. None when there is no name.
pub fn parse_active_toolchain(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("info:") && !l.starts_with("error:") && !l.starts_with("warning:"))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

/// `RUSTUP_TOOLCHAIN=<the toolchain active for app_dir>`, to pin every nested
/// rustc. Cargo runs `rustc` for each registry crate FROM THAT CRATE'S source
/// directory, and the rustup proxy picks a toolchain from a `rust-toolchain`
/// file found there unless RUSTUP_TOOLCHAIN says otherwise: hound 3.5.1 ships
/// one pinning 1.40.0, dashmap 5.5.3 one pinning 1.65 with clippy -- and a
/// build of unendlich or studio launched from gitmind on Fedora Asahi tried to
/// install both and failed in `hound`. Resolved for the APP's directory, so the
/// app's own pin (or the user's default) is what everything builds with.
/// None when the caller already set RUSTUP_TOOLCHAIN (theirs passes through),
/// or rustup is absent or says nothing usable.
pub fn rustup_pin(app_dir: &Path, path_env: &str) -> Option<(String, String)> {
    if std::env::var_os("RUSTUP_TOOLCHAIN").is_some() {
        return None;
    }
    let out = std::process::Command::new("rustup")
        .args(["show", "active-toolchain"])
        .current_dir(app_dir)
        .env("PATH", path_env)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_active_toolchain(&String::from_utf8_lossy(&out.stdout)).map(|t| ("RUSTUP_TOOLCHAIN".to_string(), t))
}

/// A PATH that includes ~/.cargo/bin and the standard locations, so a build (and its
/// rustc/linker subprocesses) resolves even from a launcher.
pub fn tool_path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cur = std::env::var("PATH").unwrap_or_default();
    format!("{home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin{}", if cur.is_empty() { String::new() } else { format!(":{cur}") })
}

/// This device's native target triple, normalized to the gitmind vocabulary (e.g.
/// Alpine's `aarch64-alpine-linux-musl` → `aarch64-unknown-linux-musl`). From rustc;
/// falls back to an Alpine check only if rustc isn't reachable.
pub fn native_triple_local() -> String {
    std::process::Command::new("rustc")
        .arg("-vV")
        .env("PATH", tool_path_env())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::buildflags::host_triple_from_rustc(&String::from_utf8_lossy(&o.stdout)))
        .map(|h| crate::buildflags::normalize_native(&h))
        .unwrap_or_else(|| {
            let libc = if std::path::Path::new("/etc/alpine-release").exists() { "musl" } else { "gnu" };
            format!("aarch64-unknown-linux-{libc}")
        })
}

/// Walk up from `start` to the gitmind repo root — the directory holding a `.gitmind`
/// marker. None if not under a gitmind-managed repo.
pub fn gitmind_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join(".gitmind").exists() {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

/// The base directory all builds go into, so they never spill `target/` trees across the
/// repos themselves. Override with ~/.config/gitmind/build-base; otherwise ~/.var/gitmind/
/// build, falling back to /tmp/gitmind/build when there's no usable HOME.
pub fn build_base() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let cfg = PathBuf::from(format!("{home}/.config/gitmind/build-base"));
            if let Ok(s) = std::fs::read_to_string(&cfg) {
                let t = s.trim();
                if !t.is_empty() {
                    return PathBuf::from(crate::places::expand_home(t, &home));
                }
            }
            return PathBuf::from(format!("{home}/.var/gitmind/build"));
        }
    }
    PathBuf::from("/tmp/gitmind/build")
}

/// Remember a build base for next time (created lazily when a build runs).
pub fn set_build_base(path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let dir = PathBuf::from(format!("{home}/.config/gitmind"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let want = crate::places::expand_home(path.trim(), &home);
    std::fs::write(dir.join("build-base"), format!("{want}\n")).map_err(|e| e.to_string())
}

/// The per-project build directory: `<build_base>/<crate>-<project-key>`. Every copy of a
/// project resolves to the same directory (keyed by git root identity), so duplicates
/// build once instead of each spilling their own multi-GB target/.
pub fn build_dir_for(app_dir: &Path, path_env: &str) -> PathBuf {
    build_base().join(crate::identity::build_slug(app_dir, path_env))
}

/// Where cargo writes the binary for `app` built in `app_dir`. `cargo_target` is the
/// `--target` value ("" for native). `target_dir_env` is an optional CARGO_TARGET_DIR
/// override — applied to the metadata query too so the resolved path matches.
pub fn resolve_binary(cargo: &str, app_dir: &Path, cargo_target: &str, app: &str, target_dir_env: Option<&str>) -> PathBuf {
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(app_dir)
        .env("PATH", tool_path_env());
    if let Some(td) = target_dir_env {
        cmd.env("CARGO_TARGET_DIR", td);
    }
    let target_dir = cmd
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::buildflags::parse_target_directory(&String::from_utf8_lossy(&o.stdout)))
        .map(PathBuf::from)
        .unwrap_or_else(|| app_dir.join("target"));
    let dir = if cargo_target.trim().is_empty() {
        target_dir.join("release")
    } else {
        target_dir.join(cargo_target).join("release")
    };
    dir.join(app)
}

/// Build `app` in `app_dir` for `target` and return the binary path + byte size. Native
/// (no --target) when the libc matches this device; cross otherwise. gitmind repos share
/// the cargo target cache.
/// The monorepo root, so repo-relative app paths (`apps/<name>`) resolve no matter what
/// directory gitmind was launched from. Order: `$GITMIND_ROOT`, then walk up from the current
/// dir looking for the repo root (a dir with an `apps/` subdir plus a `.git`/`.gitmind` marker),
/// True when `app`'s `manifest/projects.toml` entry says `role = "lib"` --
/// a headless core with no `[[bin]]` at all, by design, so a missing binary
/// after a clean `cargo build` is the correct outcome, not a failure to
/// explain away. Pure (takes the manifest TEXT, not a path) so it is
/// testable without a real checkout; the real caller in
/// `build_locally_cancellable` reads `manifest/projects.toml` from
/// [`repo_root`] and passes the text in.
pub fn is_lib_role(app: &str, manifest_text: &str) -> bool {
    crate::appcatalog::parse_manifest(manifest_text)
        .into_iter()
        .find(|e| e.app_id == app)
        .map(|e| e.role == "lib")
        .unwrap_or(false)
}

/// then fall back to the current dir.
pub fn repo_root() -> PathBuf {
    if let Ok(r) = std::env::var("GITMIND_ROOT") {
        let p = PathBuf::from(r);
        if p.join("apps").is_dir() {
            return p;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: &Path = &cwd;
        loop {
            let has_apps = cur.join("apps").is_dir();
            let marked = cur.join(".git").exists() || cur.join(".gitmind").exists();
            if has_apps && marked {
                return cur.to_path_buf();
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
        return cwd;
    }
    PathBuf::from(".")
}

/// Resolve an app directory that may be repo-relative (`apps/foo`) to an absolute path against
/// [`repo_root`]. Absolute paths pass through unchanged.
pub fn resolve_app_dir(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let root = repo_root();
    // An explicit repo-relative path (contains a separator) that exists is honored as-is.
    if path.contains('/') {
        let direct = root.join(p);
        if direct.exists() {
            return direct;
        }
    }
    // Otherwise treat `path` as a bare app/game NAME and DISCOVER where it lives, so we never hard-code
    // "apps/<name>" vs "games/<name>" — the filesystem is the source of truth. Games and apps can move
    // trees without touching the store.
    // `system` is here because `system/photosync` and `system/snapclone` are
    // real, manifest-registered projects, and a bare name for either resolved
    // to nothing — which surfaced as "photosync produces no build artifact",
    // the build having been pointed at a directory that does not exist. The
    // comment in `flow.rs`'s store list asserted photosync was "a plain
    // package at apps/photosync"; it never has been.
    for base in ["apps", "games", "system"] {
        let cand = root.join(base).join(path);
        if cand.exists() {
            return cand;
        }
    }
    // Fall back to the literal join so the caller still surfaces a clear "not found" error.
    root.join(p)
}

/// Like [`build_locally`] but streams cargo's output line-by-line to `on_line` as it runs, so a
/// UI can show a live build log. Returns the built binary path + size on success.
/// Build with no cancellation — the original signature, unchanged for callers
/// that have no flag to offer.
/// The first `n` bytes of a file. Enough for an ELF header and its program-header table,
/// without reading a 200 MB debug binary into memory to look at 64 bytes of it.
fn read_head(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let got = f.read(&mut buf)?;
    buf.truncate(got);
    Ok(buf)
}

/// Build for `target`, falling back to a podman container when the local/cross
/// attempt fails AND a native podman image exists for `target` -- the
/// composition `recommend_strategy` describes, applied at RUNTIME rather than
/// only shown as advice: a person should not have to already know their host
/// lacks a cross-linker before the right thing happens.
///
/// Tries local first even when a cross-linker looks absent: `native` (same
/// arch+libc) needs no linker at all, and this way one code path covers every
/// case rather than asking the caller to pre-classify itself. Only on FAILURE,
/// and only if podman looks usable for this target, is the container tried;
/// its own failure is reported ALONGSIDE the local one, not instead of it, so
/// a person is not left choosing which single error to trust.
pub fn build_with_fallback_logged(
    app: &str,
    app_dir: &Path,
    target: &str,
    feats: &[String],
    no_default: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<(PathBuf, u64), String> {
    match build_locally_logged(app, app_dir, target, feats, no_default, on_line) {
        Ok(r) => Ok(r),
        Err(local_err) => {
            let path_env = tool_path_env();
            let podman_ok = crate::podman::which_podman(&path_env) && crate::podman::image_for(target).is_some();
            if !podman_ok {
                return Err(local_err);
            }
            on_line(&format!("• local build failed ({local_err}); podman has a native image for {target} — trying that"));
            match crate::podman::build_in_container_cancellable(app, app_dir, target, feats, no_default, on_line) {
                Ok(r) => Ok(r),
                Err(podman_err) => Err(format!("local: {local_err}\npodman: {podman_err}")),
            }
        }
    }
}

pub fn build_locally_logged(
    app: &str,
    app_dir: &Path,
    target: &str,
    feats: &[String],
    no_default: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<(PathBuf, u64), String> {
    build_locally_cancellable(app, app_dir, target, feats, no_default, on_line, None)
}

/// Build, stopping early if `cancel` is set.
pub fn build_locally_cancellable(
    app: &str,
    app_dir: &Path,
    target: &str,
    feats: &[String],
    no_default: bool,
    on_line: &mut dyn FnMut(&str),
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(PathBuf, u64), String> {
    use std::io::{BufRead, BufReader};
    if !app_dir.join("Cargo.toml").exists() {
        return Err(format!(
            "no Cargo.toml at {} — the app path didn't resolve to a crate (launch dir: {}). Set GITMIND_ROOT or run from the repo.",
            app_dir.display(),
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let cargo = cargo_bin();
    let native = native_triple_local();
    let path_env = tool_path_env();
    let cargo_target = if crate::buildflags::is_native_build(target, &native) { String::new() } else { target.to_string() };
    // WHO OWNS THE TOOLCHAIN decides what "ensure the target" even means. Under rustup it
    // is `rustup target add`. Under Nix that command is worse than useless: on a machine
    // with both installed it manages a DIFFERENT toolchain from the cargo about to run, so
    // it succeeds, changes nothing, and the build still fails with `can't find crate for
    // std` — with the one diagnostic that could have explained it now reporting success.
    let toolchain = crate::nixtoolchain::detect_here(&cargo);
    if !cargo_target.is_empty() {
        match toolchain {
            crate::nixtoolchain::Toolchain::Nix => {
                let has_std = crate::sysdeps::rust_target_installed(&cargo_target, &path_env);
                let linker = crate::nixtoolchain::pick_linker(&cargo_target, |n| {
                    crate::nixtoolchain::on_path(n, &path_env)
                });
                if let Some(why) = crate::nixtoolchain::blocker(toolchain, &cargo_target, has_std, linker.as_deref()) {
                    on_line("• Nix toolchain detected — not calling rustup");
                    for l in why.lines() {
                        on_line(l);
                    }
                    return Err(format!("cross-build setup: no {cargo_target} toolchain in this Nix environment"));
                }
                on_line(&format!("• Nix toolchain has {cargo_target} (linker: {})", linker.unwrap_or_default()));
            }
            _ => {
                on_line(&format!("• ensuring rust target {cargo_target}…"));
                crate::sysdeps::ensure_rust_target(&cargo_target, &path_env)
                    .map_err(|e| format!("cross-build setup: {e}"))?;
            }
        }
    }
    let want = build_dir_for(app_dir, &path_env);
    let build_dir = if std::fs::create_dir_all(&want).is_ok() {
        want
    } else {
        let fb = PathBuf::from("/tmp/gitmind/build").join(crate::identity::build_slug(app_dir, &path_env));
        let _ = std::fs::create_dir_all(&fb);
        fb
    };
    // Workspace root with no [package] (gitmind: package `gitmind-cli`, binary `gitmind`) → build
    // `--bin <name>`, since `-p <name>` names a package that doesn't exist. The binary is still
    // named <name>, so binary resolution is unchanged.
    let (build_id, as_bin) = cargo_selector(app, app_dir);
    let root_toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let argv = crate::buildflags::cargo_build_argv_kind(&build_id, &cargo_target, feats, no_default, as_bin);
    // The environment the target needs. For a WINDOWED app on musl this adds
    // `-C target-feature=-crt-static`, without which the build succeeds and the app cannot
    // open a window anywhere: winit dlopens libwayland-client and libX11 at run time, and a
    // static binary cannot dlopen at all, so it dies with NoWaylandLib / "Failed to
    // initialize any backend" even on a device that has both. See `buildflags::gui_env`.
    // GUI-ness from the DIRECTORY, not the one manifest. `apps/voiceterm/Cargo.toml` is the
    // `web2qml` package and `apps/snapclone` has no top-level manifest at all — both look
    // like CLI apps to a single-file check, both miss the `-crt-static` flag, and both then
    // ship a static musl binary that cannot dlopen libwayland-client. That surfaces as a
    // winit Wayland error on the phone, which sends you looking for a missing system
    // library that is already installed.
    let gui = crate::buildflags::is_gui_app_dir(app_dir);
    // THE EFFECTIVE TRIPLE, not the `--target` string. For a native build
    // `cargo_target` is "" -- no --target is passed -- and `gui_env` decides on
    // `triple.contains("musl")`. "" contains nothing, so on the phone, the exact
    // case this flag was written for, a native build never got -crt-static and
    // shipped a static binary that died with NoWaylandLib. Reported as exactly
    // that, with the build log showing NO environment lines at all.
    let mut build_env = crate::buildflags::build_env_for_native(
        &cargo_target,
        &native,
        gui,
        std::env::var("RUSTFLAGS").ok().as_deref(),
        &path_env,
    );
    // Pin nested rustc proxies to the app's toolchain (see rustup_pin): a
    // dependency's own rust-toolchain file must not pick the compiler.
    if let Some(pin) = rustup_pin(app_dir, &path_env) {
        build_env.push(pin);
    }
    on_line(&format!("$ cargo {}", argv[1..].join(" ")));
    on_line(&format!("  (in {})", app_dir.display()));
    for (k, v) in &build_env {
        on_line(&format!("  {k}={v}"));
    }
    let mut child = std::process::Command::new(&cargo)
        .args(&argv[1..])
        .current_dir(app_dir)
        .envs(build_env.iter().map(|(k, v)| (k.clone(), v.clone())))
        .env("PATH", &path_env)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TARGET_DIR", &build_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "couldn't find cargo. Install Rust, or run from a shell where cargo is on PATH (~/.cargo/bin).".to_string()
            } else {
                format!("couldn't run the build: {e}")
            }
        })?;
    // cargo writes progress + errors to stderr; stream it live.
    //
    // This loop is also where cancellation acts: it runs for the whole build,
    // so checking the flag per line is the finest granularity available without
    // a second thread. A cargo build emits lines steadily, so "per line" is in
    // practice sub-second — and a cancel that waits for the NEXT line is still
    // vastly better than one that cannot interrupt at all.
    let mut cancelled = false;
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                // KILL, then keep draining. Dropping the child without killing
                // leaves cargo running to completion in the background — the
                // build the user just cancelled still finishes, still writes
                // its target dir, and still competes for the one CPU.
                let _ = child.kill();
                cancelled = true;
                on_line("build cancelled");
                break;
            }
            on_line(&line);
        }
    }
    let status = child.wait().map_err(|e| format!("build wait failed: {e}"))?;
    if cancelled {
        // Distinct from a failure: a cancelled build did not fail, and
        // reporting it as one would put a red error in the log for something
        // the user chose.
        return Err(format!("{app}: build cancelled"));
    }
    if status.success() {
        // The ARTIFACT name, not the package name: an explicit [[bin]]
        // overrides [package], and studio is exactly that shape (package
        // studio-app, bin studio). Passing build_id here looked for a file
        // cargo never emitted.
        let artifact = crate::buildflags::artifact_name(app, &root_toml);
        let bin = resolve_binary(&cargo, app_dir, &cargo_target, &artifact, Some(&build_dir.display().to_string()));
        // A MISSING ARTIFACT IS A FAILURE, not a zero-byte success. This used
        // to `unwrap_or(0)` and print "✓ built bsettings (0 KB)" — a tick, a
        // name, and no binary, which is the most misleading line this function
        // can produce: the build genuinely succeeded, so the error is not in
        // cargo's output either.
        //
        // It happens when the produced binary is not where resolve_binary
        // looked — a package whose bin name differs from the crate name, a bin
        // behind required-features, a workspace laying targets out elsewhere.
        // All three exist in this fleet.
        match std::fs::metadata(&bin) {
            Ok(m) if m.len() > 0 => {
                let size = m.len();
                on_line(&format!("✓ built {} ({} KB)", app, size / 1024));
                // A WINDOWED APP THAT CANNOT OPEN A WINDOW is a successful build and a
                // broken install: winit dlopens libwayland-client and libX11 at run time,
                // and a static binary cannot dlopen, so it dies with NoWaylandLib on a
                // device that has both libraries. Caught here, where the cause is still
                // visible, rather than on the phone where it looks like a missing package.
                if gui {
                    if let Ok(head) = read_head(&bin, 4096) {
                        if crate::buildflags::elf_is_dynamic(&head) == Some(false) {
                            if let Some(why) = crate::buildflags::explain_no_backend(&cargo_target) {
                                on_line(&format!("✗ {why}"));
                                return Err(why.to_string());
                            }
                            on_line("⚠ this is a static binary and the app opens a window — it will fail to find a display backend at run time");
                        }
                    }
                }
                Ok((bin, size))
            }
            _ => {
                // A MISSING BINARY IS NOT ALWAYS A FAILURE: `role = "lib"` in
                // manifest/projects.toml means exactly this -- a headless
                // core with no [[bin]] at all, by design (photosync's own
                // manifest entry documents this precisely: "role=lib is the
                // honest description until that shell exists... there is
                // nothing to run yet"). This function used to treat every
                // missing binary as an error regardless of role, which is
                // wrong for a project that was never supposed to produce
                // one -- `cargo build` finishing clean on a lib-only crate
                // IS the successful outcome, not a symptom to explain away.
                let is_lib = std::fs::read_to_string(repo_root().join("manifest/projects.toml"))
                    .map(|text| is_lib_role(app, &text))
                    .unwrap_or(false);
                if is_lib {
                    on_line(&format!("✓ {app}: library builds clean (role=lib, no binary expected)"));
                    return Ok((bin, 0));
                }
                on_line(&format!(
                    "✗ {app}: cargo succeeded but no binary at {}", bin.display()));
                on_line("   the bin name may differ from the crate name, or sit behind a feature");
                Err(format!(
                    "build produced no artifact for {app} (looked for {})", bin.display()))
            }
        }
    } else {
        Err(format!("build failed (exit {})", status.code().unwrap_or(-1)))
    }
}

/// Run the fleet-wide `build/build.sh` orchestrator (the same script driving CI/manual builds —
/// the manifest/projects.toml-driven "build everything" path, distinct from `build_locally_logged`
/// above which builds ONE app's crate directly). `only` narrows to a single manifest app id
/// (matches `build.sh --only NAME`); `None` builds the whole fleet. Always passes `--run` — a plan-
/// only dry run wouldn't give the GUI anything meaningful to report back. Streams both stdout and
/// stderr live, interleaved in the order the script actually produced them (build.sh writes plan
/// info to stdout and per-app build progress to stderr-or-stdout depending on the tool; separating
/// the two would reorder them in a way that's harder to follow, not easier).
pub fn run_build_all_script(repo_root: &Path, only: Option<&str>, on_line: &mut dyn FnMut(&str)) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let script = repo_root.join("build").join("build.sh");
    if !script.exists() {
        return Err(format!("no build/build.sh at {} — is this the monorepo root?", repo_root.display()));
    }
    let mut argv = vec!["--run".to_string()];
    if let Some(name) = only {
        argv.push("--only".to_string());
        argv.push(name.to_string());
    }
    on_line(&format!("$ bash build/build.sh {}", argv.join(" ")));
    let mut child = std::process::Command::new("bash")
        .arg(&script)
        .args(&argv)
        .current_dir(repo_root)
        .env("PATH", tool_path_env())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run build.sh: {e}"))?;
    // interleave stdout/stderr by reading stdout on this thread and stderr on a spawned one,
    // both pushing into the same on_line sink — `on_line` itself isn't required to be Sync, so
    // route stderr lines through a channel instead of sharing the callback across threads.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let stderr = child.stderr.take();
    let stderr_thread = stderr.map(|err| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        })
    });
    drop(tx);
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            on_line(&line);
        }
    }
    if let Some(h) = stderr_thread { let _ = h.join(); }
    for line in rx.try_iter() { on_line(&line); }
    let status = child.wait().map_err(|e| format!("build.sh wait failed: {e}"))?;
    if status.success() {
        Ok(match only {
            Some(name) => format!("✓ {name} built"),
            None => "✓ fleet build complete".to_string(),
        })
    } else {
        Err(format!("build.sh failed (exit {})", status.code().unwrap_or(-1)))
    }
}

/// Does a workspace under `dir` contain a MEMBER package literally named `name`? If so, `-p name`
/// resolves (build that member); if not (gitmind: members are `gitmind-cli` etc., no `gitmind`), we
/// must build `--bin name` instead. Bounded walk; skips target/.git.
/// How cargo is told which package to build for `app` in `app_dir`: the REAL
/// package name and whether to select it with `--bin` instead of `-p`. Shared by
/// the local build and the podman build -- the podman path passed the raw app id
/// (`-p tideshell`, where the package is `tideshell-egui`) and cargo refused it
/// as "cannot specify features for packages outside of workspace".
///
/// Workspace root with no [package] (gitmind: package `gitmind-cli`, binary
/// `gitmind`) → `--bin <name>`, since `-p <name>` names a package that doesn't
/// exist. Otherwise the real [package] name, which can differ from `app`, the
/// manifest/display id (voiceterm's crate is still named "web2qml"); `app` when
/// there is no [package] name to read.
pub fn cargo_selector(app: &str, app_dir: &Path) -> (String, bool) {
    let root_toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let as_bin = crate::buildflags::declares_workspace(&root_toml)
        && crate::buildflags::crate_name(&root_toml).is_none()
        && !has_member_package(app_dir, app); // logscope HAS a member pkg `logscope` → keep -p
    (crate::buildflags::build_id(app, &root_toml), as_bin)
}

fn has_member_package(dir: &Path, name: &str) -> bool {
    fn walk(dir: &Path, name: &str, depth: u32) -> bool {
        if depth > 3 {
            return false;
        }
        if let Ok(txt) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            if crate::buildflags::crate_name(&txt).as_deref() == Some(name) {
                return true;
            }
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let n = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && n != "target" && !n.starts_with('.') && walk(&p, name, depth + 1) {
                    return true;
                }
            }
        }
        false
    }
    walk(dir, name, 0)
}

pub fn build_locally(app: &str, app_dir: &Path, target: &str, feats: &[String], no_default: bool) -> Result<(PathBuf, u64), String> {
    if !app_dir.join("Cargo.toml").exists() {
        return Err(format!(
            "no Cargo.toml at {} — the app path didn't resolve to a crate (launch dir: {}). Set GITMIND_ROOT or run from the repo.",
            app_dir.display(),
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let cargo = cargo_bin();
    let native = native_triple_local();
    let path_env = tool_path_env();
    let cargo_target = if crate::buildflags::is_native_build(target, &native) { String::new() } else { target.to_string() };
    // Cross-build? Make sure the target's std is installed first, so we don't fail with
    // "can't find crate for std" — gitmind runs `rustup target add <triple>` for you.
    if !cargo_target.is_empty() {
        crate::sysdeps::ensure_rust_target(&cargo_target, &path_env)
            .map_err(|e| format!("cross-build setup: {e}"))?;
    }
    // one canonical build dir per project identity — no target/ spilled into the repo, and
    // every copy of the project shares it. Fall back to /tmp if ~/.var isn't writable.
    let want = build_dir_for(app_dir, &path_env);
    let build_dir = if std::fs::create_dir_all(&want).is_ok() {
        want
    } else {
        let fb = PathBuf::from("/tmp/gitmind/build").join(crate::identity::build_slug(app_dir, &path_env));
        let _ = std::fs::create_dir_all(&fb);
        fb
    };
    // Workspace root with no [package] (gitmind: package `gitmind-cli`, binary `gitmind`) → build
    // `--bin <name>`, since `-p <name>` names a package that doesn't exist. The binary is still
    // named <name>, so binary resolution is unchanged.
    let root_toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let as_bin = crate::buildflags::declares_workspace(&root_toml)
        && crate::buildflags::crate_name(&root_toml).is_none()
        && !has_member_package(app_dir, app); // logscope HAS a member pkg `logscope` → keep -p
    let argv = crate::buildflags::cargo_build_argv_kind(app, &cargo_target, feats, no_default, as_bin);
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(&argv[1..]).current_dir(app_dir).env("PATH", &path_env).env("CARGO_TARGET_DIR", &build_dir);
    match cmd.output() {
        Ok(o) if o.status.success() => {
            let bin = resolve_binary(&cargo, app_dir, &cargo_target, app, Some(&build_dir.display().to_string()));
            let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
            Ok((bin, size))
        }
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(
            "couldn't find cargo. Install Rust, or run from a shell where cargo is on PATH (it lives in ~/.cargo/bin).".to_string()
        ),
        Err(e) => Err(format!("couldn't run the build: {e}")),
    }
}

/// Append any missing lines to a directory's .gitignore (creating it if needed).
pub fn ensure_gitignore(dir: &Path, lines: &[&str]) {
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = existing.clone();
    for line in lines {
        let present = existing.lines().any(|l| l.trim() == *line);
        if !present {
            if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
            out.push_str(line);
            out.push('\n');
        }
    }
    if out != existing {
        let _ = std::fs::write(&path, out);
    }
}

/// Current time as an OpenSSH expiry-time stamp (YYYYMMDDHHMMSS).
pub fn now_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::deploy::key_expiry_time(now, 0)
}

/// Mark a freshly-installed binary executable (best-effort; Unix perms).
pub fn set_executable(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Create the fleet user on a device and install the restricted key into it, over ssh as
/// the sudo-capable `admin` account. On-device (useradd/adduser + sudo).
pub fn provision_fleet_user(admin: &str, host: &str, password: &str) -> Result<String, String> {
    let user = crate::fleet::fleet_user();
    let pubkey = crate::deploy::ensure_local_pubkey()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expiry = crate::deploy::key_expiry_time(now, crate::deploy::KEY_EXPIRY_DAYS);
    let key_line = crate::deploy::restricted_key_line(&crate::deploy::authorized_keys_options(host, Some(&expiry)), &pubkey);
    let script = crate::fleet::provision_user_script(&user, &key_line);
    let remote = crate::fleet::provision_remote_command(&crate::fleet::base64_encode(script.as_bytes()));
    let out = crate::deploy::ssh_sudo_run(admin, host, password, &remote, &format!("{password}\n"))?;
    if out.contains("gitmind-fleet-user-ready") {
        Ok(format!("created fleet user '{user}' on {host} — set the user to '{user}' to connect"))
    } else {
        Err("provisioning didn't confirm — does that account have sudo rights?".into())
    }
}

#[cfg(test)]
mod is_lib_role_tests {
    use super::*;

    const SAMPLE: &str = r#"
[apps.photosync]
path = "apps/photosync"
kind = "cargo"
role = "lib"
daemon = false

[apps.voiceterm]
path = "apps/voiceterm"
kind = "cargo"
role = "app"
daemon = false
"#;

    #[test]
    fn a_lib_role_entry_is_recognised() {
        assert!(is_lib_role("photosync", SAMPLE));
    }

    #[test]
    fn an_app_role_entry_is_not_lib() {
        assert!(!is_lib_role("voiceterm", SAMPLE));
    }

    #[test]
    fn an_unknown_app_is_not_lib() {
        // No entry at all -- must not panic, must not default to true (a
        // typo'd or unregistered app should still get the real "no binary"
        // error, not be silently waved through as an expected lib).
        assert!(!is_lib_role("does-not-exist", SAMPLE));
    }

    #[test]
    fn empty_manifest_text_is_not_lib() {
        assert!(!is_lib_role("photosync", ""));
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;

    /// GITMIND_ROOT IS PROCESS-GLOBAL, SO THESE TESTS CANNOT RUN AT ONCE.
    ///
    /// `repo_root` honours the env var, one test sets it to a temp fixture,
    /// and cargo runs tests on parallel threads — so while
    /// `discovers_apps_vs_games_by_name` held it, the other test resolved
    /// `apps/gitmind` against `/tmp/gm-disc-<pid>` and found no Cargo.toml.
    /// That is exactly how it failed in the fleet run:
    ///
    ///   resolved /tmp/gm-disc-580267/apps/gitmind should be a crate
    ///
    /// Neither test is wrong and the product is fine. An env var is shared
    /// mutable state, and the only fix that does not weaken what either test
    /// checks is to stop them overlapping. A mutex over the pair rather than
    /// `--test-threads=1`, which would serialise all 327.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take the lock without inheriting a poisoning from an unrelated failure
    /// — a panic in one of these tests should fail that test, not cascade.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn repo_root_finds_monorepo_and_resolves_app_paths() {
        let _g = env_guard();
        // running under cargo, CWD is the crate dir; walking up must find the repo root
        // (a dir with apps/ + a .git/.gitmind marker).
        let root = repo_root();
        assert!(root.join("apps").is_dir(), "repo_root has an apps/ dir: {}", root.display());
        // a repo-relative app path resolves to an existing crate with a Cargo.toml
        let gm = resolve_app_dir("apps/gitmind");
        assert!(gm.is_absolute());
        assert!(gm.join("Cargo.toml").exists(), "resolved {} should be a crate", gm.display());
        // absolute paths pass through
        assert_eq!(resolve_app_dir("/tmp/x"), std::path::PathBuf::from("/tmp/x"));
    }

    #[test]
    fn discovers_apps_vs_games_by_name() {
        let _g = env_guard();
        // a temp repo with an app under apps/ and a game under games/
        let root = std::env::temp_dir().join(format!("gm-disc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(root.join("apps").join("gitmind"));
        let _ = std::fs::create_dir_all(root.join("games").join("unendlich"));
        std::env::set_var("GITMIND_ROOT", &root);
        // bare names resolve to the tree that actually contains them
        assert_eq!(resolve_app_dir("gitmind"), root.join("apps").join("gitmind"));
        assert_eq!(resolve_app_dir("unendlich"), root.join("games").join("unendlich"));
        // an explicit existing path is honored as-is
        assert_eq!(resolve_app_dir("apps/gitmind"), root.join("apps").join("gitmind"));
        std::env::remove_var("GITMIND_ROOT");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn member_package_check_distinguishes_gitmind_from_logscope() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(4).unwrap().to_path_buf();
        // gitmind: NO member package named "gitmind" (it's gitmind-cli, bin gitmind) → build --bin
        assert!(!super::has_member_package(&repo.join("apps/gitmind"), "gitmind"));
        // logscope: HAS a member package named "logscope" (the core lib) → keep -p logscope
        assert!(super::has_member_package(&repo.join("apps/logscope"), "logscope"));
    }

    #[test]
    fn build_locally_errors_clearly_on_bad_path() {
        // a path with no Cargo.toml fails fast with a helpful message, not a silent no-op
        let err = build_locally("nope", Path::new("/definitely/not/a/crate"), "", &[], false).unwrap_err();
        assert!(err.contains("no Cargo.toml"), "clear error: {err}");
    }
}

/// Does this repo have that commit?
///
/// The question `incoming::plan` needs answered and cannot answer itself. A
/// bare `cat-file -t` rather than `rev-parse`: rev-parse succeeds on an
/// abbreviated sha that matches nothing yet, and a prerequisite check that
/// says yes for a commit git cannot read is the failure this whole flow exists
/// to catch early.
pub fn has_commit(sha: &str) -> bool {
    if sha.trim().is_empty() {
        return false;
    }
    std::process::Command::new("git")
        .args(["cat-file", "-t", sha])
        .env("PATH", tool_path_env())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "commit")
        .unwrap_or(false)
}

/// What a bundle carries and what it needs: `(refs, prerequisites)`.
///
/// Parsed from `git bundle verify`, which prints both even when it then
/// refuses the bundle — that refusal is exactly the case worth reporting, so
/// the exit status is deliberately ignored and only the output is read.
pub fn bundle_refs(path: &str) -> (Vec<String>, Vec<String>) {
    let out = std::process::Command::new("git")
        .args(["bundle", "verify", path])
        .env("PATH", tool_path_env())
        .output();
    let Ok(o) = out else { return (Vec::new(), Vec::new()) };
    // verify writes its listing to stderr on some versions and stdout on
    // others; reading both avoids a silent empty result.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let mut refs = Vec::new();
    let mut requires = Vec::new();
    let mut in_requires = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("The bundle requires") {
            in_requires = true;
            continue;
        }
        if l.starts_with("The bundle contains") {
            in_requires = false;
            continue;
        }
        let mut parts = l.split_whitespace();
        let (Some(sha), rest) = (parts.next(), parts.next()) else { continue };
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if in_requires {
            requires.push(sha.to_string());
        } else if let Some(name) = rest {
            refs.push(name.to_string());
        }
    }
    (refs, requires)
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    /// No Cargo.toml -> the local build fails fast and clearly. Podman is
    /// genuinely absent from THIS machine (confirmed: `podman --version` ->
    /// "not found"), so this exercises the real "no fallback available"
    /// branch against the real environment, not a stand-in for it: the local
    /// error must surface UNCHANGED, with no podman attempt mentioned.
    #[test]
    fn without_podman_the_local_error_surfaces_unchanged() {
        let dir = std::env::temp_dir().join(format!("fallback_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines = Vec::new();
        let err = build_with_fallback_logged("nope", &dir, "aarch64-unknown-linux-musl", &[], false, &mut |l| lines.push(l.to_string()))
            .unwrap_err();
        assert!(err.contains("no Cargo.toml"), "{err}");
        assert!(!err.contains("podman"), "no fallback was attempted, so podman must not appear: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod rustup_pin_tests {
    use super::parse_active_toolchain;

    #[test]
    fn the_active_toolchain_is_read_from_rustups_reply() {
        assert_eq!(parse_active_toolchain("stable-aarch64-unknown-linux-gnu (default)\n").as_deref(), Some("stable-aarch64-unknown-linux-gnu"));
        assert_eq!(parse_active_toolchain("1.96.0-x86_64-unknown-linux-gnu (overridden by '/src/app/rust-toolchain.toml')\n").as_deref(),
                   Some("1.96.0-x86_64-unknown-linux-gnu"));
        assert_eq!(parse_active_toolchain("info: syncing channel updates for 'stable'\nstable-x86_64-unknown-linux-gnu (default)\n").as_deref(),
                   Some("stable-x86_64-unknown-linux-gnu"), "info lines are skipped");
        assert_eq!(parse_active_toolchain(""), None);
        assert_eq!(parse_active_toolchain("error: no default toolchain configured\n"), None);
    }
}
