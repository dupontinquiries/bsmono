//! podman — building a target natively inside a container, when the host has
//! no cross-linker for it.
//!
//! ## Why this exists
//!
//! `buildflags::cross_linker` names a linker for each target this fleet
//! builds, and its own doc says why aarch64-musl usually has none on a
//! desktop: "not in Debian's archive". So for this fleet's actual phone
//! target, `DeployStrategy::CrossCompile` is often not a real option, and the
//! fallback used to be `RemoteNative` -- asking a phone to compile an egui/wgpu
//! GUI, which is slow and, for some crates, not realistic at all.
//!
//! A container with a NATIVE musl toolchain sidesteps needing a cross-linker
//! on the host altogether: the compiler inside the container simply IS a
//! musl-targeting compiler. `messense/rust-musl-cross` is that image --
//! actively maintained, widely used (500k+ pulls), and it bundles a working
//! `aarch64-linux-musl-gcc` plus static OpenSSL, which is exactly the shape of
//! problem ("no dbus/pkg-config in the image") a hand-rolled Alpine image
//! keeps rediscovering one dependency at a time.
//!
//! ## What went wrong once already, and is designed around here
//!
//! A build attempted before the container's virtiofs mounts finish settling
//! fails with an opaque "host directory cannot be empty" -- not a build error
//! at all, a boot race. `wait_until_ready` polls `podman machine list --format
//! json` and waits for `Running` rather than guessing a sleep, because a fixed
//! sleep is exactly the fragile fix that (by report) needed retrying by hand.
//!
//! ## Verified and not
//!
//! `image_for`, `parse_machine_running` and `build_argv` are pure and tested.
//! `wait_until_ready` and `build_in_container_cancellable` run real `podman`
//! commands; this container has no podman at all (confirmed: `podman
//! --version` fails with "not found"), so that half is UNVERIFIED here -- the
//! same caveat every other real-hardware/real-tool path in this codebase
//! already carries. What compiles and what is pure logic are still checked.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Is `podman` on this PATH? Pure over the PATH string, like
/// `pydeps::which`, so it is testable without touching the real environment.
pub fn which_podman(path_env: &str) -> bool {
    path_env.split(':').any(|d| !d.is_empty() && Path::new(d).join("podman").exists())
}

/// A `messense/rust-musl-cross` tag for `triple`, if this fleet knows one --
/// this project's musl target only for now; the fleet's other targets already
/// have working native/cross paths (see `recommend_strategy`), and a wrong
/// guess here is worse than "no image".
pub fn image_for(triple: &str) -> Option<&'static str> {
    match triple {
        // The bare upstream image; it lacks pkg-config and dbus's static lib,
        // which some apps need at build time (bluer/dbus-sys -- see
        // `apps/gitmind/podman/Dockerfile.aarch64-musl`, which extends this
        // exact tag with them). A local `podman build` from that file, tagged
        // over this same name, upgrades every build transparently; nothing
        // here needs to know which one is actually present.
        //
        // FULLY QUALIFIED: podman on Fedora (and anything with
        // short-name-mode = "enforcing") refuses an unqualified name when it
        // cannot prompt for a registry -- "short-name resolution enforced but
        // cannot prompt without a TTY" -- which is always, run from the GUI.
        "aarch64-unknown-linux-musl" => Some("docker.io/messense/rust-musl-cross:aarch64-musl"),
        _ => None,
    }
}

/// Where podman keeps a LOCAL build tagged with the upstream image's short name:
/// `podman build -t messense/rust-musl-cross:aarch64-musl` stores it as
/// `localhost/messense/...`. A qualified docker.io name would never find it,
/// so the upgrade `image_for`'s doc promises needs this.
pub fn local_override(image: &str) -> String {
    let short = image.strip_prefix("docker.io/").unwrap_or(image);
    format!("localhost/{short}")
}

/// The image to run: the local build when there is one (it adds what the bare
/// upstream image lacks), else the upstream image.
pub fn choose_image(image: &str, local_exists: bool) -> String {
    if local_exists { local_override(image) } else { image.to_string() }
}

fn local_image_exists(name: &str) -> bool {
    Command::new("podman").args(["image", "exists", name]).status().map(|s| s.success()).unwrap_or(false)
}

/// Does a `podman machine list --format json` reply show a running machine?
///
/// `None` when the field cannot be found at all -- no machines defined, or a
/// reply shape this does not recognise -- so the caller can tell "definitely
/// not running" from "could not tell", which want different messages (retry
/// vs. give up).
///
/// A HAND-ROLLED SCAN, not a JSON library: this crate has no `serde_json`
/// dependency and pulling one in for a single boolean field is not worth it.
/// Looks for `"Running": true` or `"Running":true` (podman's exact spacing
/// varies by version) as a literal token search -- fragile to a genuinely
/// different reply shape, which is exactly why it degrades to `None` rather
/// than guessing.
pub fn parse_machine_running(json: &str) -> Option<bool> {
    let key = json.find("\"Running\"")?;
    let after = &json[key + "\"Running\"".len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    if after.starts_with("true") {
        Some(true)
    } else if after.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// The `podman run` argv to build `app` for `triple` inside `image`, mounting
/// `app_dir` read-write at `/home/rust/src` (the mount point
/// `rust-musl-cross`'s own README uses) and reusing
/// `buildflags::cargo_build_argv` for the actual cargo invocation, so the
/// features/`--no-default-features` logic is not duplicated between the
/// local and containerised build paths.
pub fn build_argv(app: &str, app_dir: &Path, triple: &str, features: &[String], no_default: bool, image: &str) -> Vec<String> {
    build_argv_at(app, app_dir, &mount_root(app_dir), triple, features, no_default, image)
}

/// What to mount: the repository the app lives in (the nearest ancestor with
/// a `.git`), else the app's own directory. An app's `path =` dependencies
/// point OUTSIDE its directory (tideshell: ../crates/tideshell-core,
/// ../../../shared/...), so mounting only the app left every one of them
/// missing inside the container.
pub fn mount_root(app_dir: &Path) -> PathBuf {
    app_dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| app_dir.to_path_buf())
}

/// The podman argv, mounting `root` and building from `app_dir`'s place in it.
/// `--security-opt label=disable` rather than `:z`: on SELinux hosts (Fedora)
/// `:z` relabels every file under the mount -- the whole checkout -- which is
/// slow and changes the user's files; disabling labelling for this one
/// container touches nothing.
pub fn build_argv_at(app: &str, app_dir: &Path, root: &Path, triple: &str, features: &[String], no_default: bool, image: &str) -> Vec<String> {
    let cargo_argv = crate::buildflags::cargo_build_argv(app, triple, features, no_default);
    let rel = app_dir.strip_prefix(root).unwrap_or(Path::new(""));
    let workdir = Path::new("/home/rust/src").join(rel);
    let mut argv = vec![
        "run".to_string(), "--rm".to_string(),
        "--security-opt".to_string(), "label=disable".to_string(),
        "-v".to_string(), format!("{}:/home/rust/src", root.display()),
        "-w".to_string(), workdir.to_string_lossy().into_owned(),
        image.to_string(),
    ];
    argv.extend(cargo_argv);
    argv
}

/// Wait for a podman machine to report `Running`, polling rather than
/// sleeping a guessed duration (see the module doc). `Ok(())` immediately if
/// no machine is needed at all (a native-Linux podman with no VM layer).
pub fn wait_until_ready(timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let out = Command::new("podman").args(["machine", "list", "--format", "json"]).output();
        match out {
            // No `podman machine` subcommand output we can parse, or none
            // configured, is not necessarily a failure -- native-Linux podman
            // needs no VM at all. Proceed; a real problem surfaces at build time.
            Err(_) => return Ok(()),
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                match parse_machine_running(&text) {
                    Some(true) | None => return Ok(()),
                    Some(false) => {
                        if Instant::now() >= deadline {
                            return Err("podman machine did not report Running in time".into());
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        }
    }
}

/// Build inside a container, matching `ops::build_locally_cancellable`'s
/// signature and return shape so a caller can swap this in for that.
///
/// Waits for the machine first (`wait_until_ready`), then runs `build_argv`'s
/// command and locates the produced binary under
/// `app_dir/target/<triple>/release/<app>` -- the same layout a local
/// `cargo build --target` produces, since the container runs the same
/// `cargo_build_argv`.
pub fn build_in_container_cancellable(
    app: &str, app_dir: &Path, triple: &str, features: &[String], no_default: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<(PathBuf, u64), String> {
    let upstream = image_for(triple).ok_or_else(|| format!("no podman image known for {triple}"))?;
    let chosen = choose_image(upstream, local_image_exists(&local_override(upstream)));
    let image = chosen.as_str();
    on_line(&format!("• waiting for podman to be ready…"));
    wait_until_ready(Duration::from_secs(30))?;
    let argv = build_argv(app, app_dir, triple, features, no_default, image);
    on_line(&format!("• podman {}", argv.join(" ")));
    let out = Command::new("podman").args(&argv).output().map_err(|e| format!("running podman: {e}"))?;
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        on_line(line);
    }
    if !out.status.success() {
        return Err(format!("podman build failed (exit {})", out.status));
    }
    let bin = app_dir.join("target").join(triple).join("release").join(app);
    let size = std::fs::metadata(&bin).map(|m| m.len()).map_err(|e| format!("no binary at {}: {e}", bin.display()))?;
    Ok((bin, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_podman_checks_every_path_entry() {
        let dir = std::env::temp_dir().join(format!("podman_which_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("podman"), b"").unwrap();
        let path_env = format!("/nonexistent:{}:/also/nonexistent", dir.display());
        assert!(which_podman(&path_env));
        assert!(!which_podman("/nonexistent:/also/nonexistent"));
        assert!(!which_podman(""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_for_only_names_the_target_this_fleet_actually_has_an_image_for() {
        assert_eq!(image_for("aarch64-unknown-linux-musl"), Some("docker.io/messense/rust-musl-cross:aarch64-musl"));
        assert_eq!(image_for("aarch64-unknown-linux-gnu"), None, "has a real cross-linker (buildflags::cross_linker) -- no image needed");
        assert_eq!(image_for("x86_64-unknown-linux-gnu"), None);
    }

    #[test]
    fn parses_running_true_and_false_regardless_of_spacing() {
        assert_eq!(parse_machine_running(r#"[{"Name":"podman-machine-default","Running": true}]"#), Some(true));
        assert_eq!(parse_machine_running(r#"[{"Name":"x","Running":true}]"#), Some(true));
        assert_eq!(parse_machine_running(r#"[{"Name":"x","Running": false}]"#), Some(false));
    }

    #[test]
    fn no_running_field_at_all_is_none_not_a_guess() {
        assert_eq!(parse_machine_running("[]"), None);
        assert_eq!(parse_machine_running(""), None);
        assert_eq!(parse_machine_running("not json at all"), None);
    }

    /// A REAL fixture: actual `podman machine list --format json` shape (field
    /// order and neighbours from podman's own output), so the parser is
    /// checked against more than a hand-trimmed snippet.
    #[test]
    fn parses_a_realistic_full_machine_list_reply() {
        let json = r#"[
            {
                "Name": "podman-machine-default",
                "Default": true,
                "Created": "2026-01-01T00:00:00Z",
                "Running": true,
                "LastUp": "Currently running",
                "Stream": "default",
                "VMType": "applehv",
                "CPUs": 4,
                "Memory": "6442450944",
                "DiskSize": "107374182400",
                "Port": 12345,
                "RemoteUsername": "core",
                "IdentityPath": "/Users/x/.local/share/containers/podman/machine/machine",
                "UserModeNetworking": true
            }
        ]"#;
        assert_eq!(parse_machine_running(json), Some(true));
    }

    #[test]
    fn build_argv_mounts_the_app_dir_and_reuses_the_cargo_argv() {
        // Mounting the REPOSITORY: tideshell's path dependencies live outside its
        // directory (../crates/tideshell-core, ../../../shared/...).
        let dir = Path::new("/repo/apps/tideshell/egui-frontend");
        let argv = build_argv_at("tideshell", dir, Path::new("/repo"), "aarch64-unknown-linux-musl",
                               &["wayland".to_string(), "layer".to_string()], true,
                               "docker.io/messense/rust-musl-cross:aarch64-musl");
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--rm".to_string()));
        let v = argv.iter().position(|a| a == "-v").unwrap();
        assert_eq!(argv[v + 1], "/repo:/home/rust/src", "the repository, not just the app");
        let w = argv.iter().position(|a| a == "-w").unwrap();
        assert_eq!(argv[w + 1], "/home/rust/src/apps/tideshell/egui-frontend");
        let so = argv.iter().position(|a| a == "--security-opt").unwrap();
        assert_eq!(argv[so + 1], "label=disable", "no recursive :z relabel of the checkout");
        assert!(!argv.iter().any(|a| a.ends_with(":z")));
        assert!(argv.contains(&"docker.io/messense/rust-musl-cross:aarch64-musl".to_string()));
        // The same argv cargo_build_argv would produce, appended after the image.
        let image_pos = argv.iter().position(|a| a == "docker.io/messense/rust-musl-cross:aarch64-musl").unwrap();
        let cargo_part = &argv[image_pos + 1..];
        assert_eq!(cargo_part, &crate::buildflags::cargo_build_argv("tideshell", "aarch64-unknown-linux-musl",
                                                                     &["wayland".to_string(), "layer".to_string()], true));
    }

    /// `wait_until_ready` must not hang forever, and must not treat "podman
    /// has no machine subcommand at all" (a plain error running the command,
    /// e.g. podman is not installed) as a reason to retry -- there is nothing
    /// to wait for.
    #[test]
    fn wait_until_ready_does_not_hang_when_podman_itself_is_absent() {
        // On this machine `podman` genuinely does not exist (checked: `podman
        // --version` -> "not found"), so `Command::new("podman")` errors
        // immediately and this exercises the real "no such command" path,
        // not a fixture standing in for it.
        let start = std::time::Instant::now();
        let r = wait_until_ready(Duration::from_secs(5));
        assert!(r.is_ok(), "{r:?}");
        assert!(start.elapsed() < Duration::from_secs(1), "must return immediately, not wait out the timeout");
    }

    #[test]
    fn image_names_are_fully_qualified_and_a_local_build_is_preferred() {
        let up = image_for("aarch64-unknown-linux-musl").unwrap();
        // Enforcing short-name mode (Fedora) rejects a bare "name:tag" without a TTY.
        assert!(up.starts_with("docker.io/"), "{up}");
        assert_eq!(local_override(up), "localhost/messense/rust-musl-cross:aarch64-musl");
        assert_eq!(choose_image(up, false), up);
        assert_eq!(choose_image(up, true), "localhost/messense/rust-musl-cross:aarch64-musl");
    }

    #[test]
    fn the_mount_is_the_repository_the_app_lives_in() {
        let base = std::env::temp_dir().join(format!("cockpit-mount-{}", std::process::id()));
        let app = base.join("repo/apps/tideshell/egui-frontend");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(mount_root(&app), app, "no .git anywhere: the app's own directory");
        std::fs::create_dir_all(base.join("repo/.git")).unwrap();
        assert_eq!(mount_root(&app), base.join("repo"));
        let argv = build_argv("tideshell", &app, "aarch64-unknown-linux-musl", &[], true, "img");
        let w = argv.iter().position(|a| a == "-w").unwrap();
        assert_eq!(argv[w + 1], "/home/rust/src/apps/tideshell/egui-frontend");
        let _ = std::fs::remove_dir_all(&base);
    }
}
