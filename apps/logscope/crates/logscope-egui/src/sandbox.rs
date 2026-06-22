//! sandbox.rs — verify a generated workspace by building/testing it in an ISOLATED throwaway copy.
//!
//! Why this exists (from the harness-design + sandboxing notes): for VERIFIABLE code, an LLM evaluator
//! is unreliable (models praise their own work; a small local generator can't QA itself), but the
//! toolchain is a deterministic, taste-free oracle. So the "evaluator" here is `cargo build`/`test`
//! (or any configured command) run over a /tmp MIRROR of the workspace under bubblewrap — filesystem
//! AND network isolation — so untrusted generated code can run autonomously in a loop without
//! touching the real tree or the network. Returns {passed, output}; on failure the compiler error is
//! carried verbatim so it can be fed back to the generator for the next iteration.
//!
//! UX tie-in: the aggressive coordinator fans out K solutions (tree branches); verify() grades each,
//! passing branches become "proven" (pinned), failing ones feed their error back. The sandbox is what
//! makes best-of-K meaningful — otherwise it's K unverified guesses.
//!
//! Scope: pure orchestration around std::process + a recursive copy. The isolation is bubblewrap on
//! Linux (the user's Asahi M2). When bwrap is absent, verify() still runs the command in the mirror
//! and reports sandboxed=false, so the loop degrades to "built in a throwaway copy, not isolated".

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub output: String,  // combined stdout+stderr — the compiler error on failure
    pub sandboxed: bool, // whether OS-level isolation was actually applied (bwrap present)
    pub mirror: PathBuf, // the /tmp copy that was built (inspect/reuse/delete)
}

/// What a non-pass actually means — so a network hiccup isn't reported as broken code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict { Pass, CompileError, DepsUnavailable, ToolError }

/// Classify a verify outcome from the toolchain output. Checks dependency/network failures FIRST
/// (those carry registry/DNS text, never an `error[E…]`), then genuine rustc diagnostics.
pub fn classify(passed: bool, output: &str) -> Verdict {
    if passed { return Verdict::Pass; }
    let o = output.to_lowercase();
    if o.contains("could not resolve host") || o.contains("failed to download")
        || o.contains("spurious network error") || o.contains("failed to get ")
        || o.contains("no matching package") || o.contains("unable to get packages")
        || o.contains("failed to query replaced source") {
        return Verdict::DepsUnavailable;
    }
    if o.contains("error[e") || o.contains("aborting due to") || o.contains("cannot find")
        || o.contains("mismatched types") || o.contains("expected ") || o.contains("error: ") {
        return Verdict::CompileError;
    }
    Verdict::ToolError
}

#[derive(Clone, Debug)]
pub struct VerifySpec {
    pub workspace: PathBuf,   // the source dir to verify
    pub command: Vec<String>, // e.g. ["cargo","build","--offline"]
    pub net: bool,            // allow network (default false: offline, hermetic)
    pub mirror_root: PathBuf, // where the /tmp copy goes
}

impl VerifySpec {
    /// Default: an offline `cargo build` in a /tmp mirror (no network, so the build is hermetic).
    pub fn cargo_build(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            command: vec!["cargo".into(), "build".into(), "--offline".into()],
            net: false,
            mirror_root: std::env::temp_dir().join("logscope-verify"),
        }
    }
    pub fn cargo_test(workspace: impl Into<PathBuf>) -> Self {
        let mut s = Self::cargo_build(workspace);
        s.command = vec!["cargo".into(), "test".into(), "--offline".into()];
        s
    }
    /// `cargo check` — faster than build (no codegen) and enough to catch type/borrow/syntax errors.
    pub fn cargo_check(workspace: impl Into<PathBuf>) -> Self {
        let mut s = Self::cargo_build(workspace);
        s.command = vec!["cargo".into(), "check".into(), "--offline".into()];
        s
    }
    pub fn with_command(mut self, cmd: Vec<String>) -> Self { self.command = cmd; self }
    pub fn with_net(mut self, net: bool) -> Self { self.net = net; self }
}

/// Non-std extern crates referenced via `use <crate>…;` or `extern crate <crate>;`.
pub fn detect_deps(files: &[(String, String)]) -> Vec<String> {
    let std_like = ["std", "core", "alloc", "crate", "self", "super", "proc_macro", "test"];
    let mut deps: Vec<String> = Vec::new();
    for (_, content) in files {
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("use ").or_else(|| t.strip_prefix("extern crate ")) {
                let name: String = rest.trim_start_matches("::")
                    .chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() && !std_like.contains(&name.as_str()) && !deps.contains(&name) {
                    deps.push(name);
                }
            }
        }
    }
    deps
}

fn sanitize_pkg(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let n = n.trim_matches('_').to_string();
    match n.chars().next() { Some(c) if c.is_ascii_digit() => format!("c_{n}"), None => "scratch".into(), _ => n }
}

/// Scaffold a throwaway cargo project from generated Rust file(s). One file with `fn main` → a binary
/// (src/main.rs); otherwise a lib (entry → src/lib.rs, the rest as `mod` siblings). Detected extern
/// crates are listed as `"*"` deps (verify with net so cargo can resolve them).
pub fn scaffold_cargo(name: &str, files: &[(String, String)], root: &Path) -> std::io::Result<PathBuf> {
    let dir = root.join(format!("{}-proj", sanitize_pkg(name)));
    if dir.exists() { let _ = std::fs::remove_dir_all(&dir); }
    std::fs::create_dir_all(dir.join("src"))?;
    let deps = detect_deps(files);
    let mut toml = format!("[package]\nname = \"{}\"\nedition = \"2021\"\n\n[dependencies]\n", sanitize_pkg(name));
    for d in &deps { toml.push_str(&format!("{d} = \"*\"\n")); }
    std::fs::write(dir.join("Cargo.toml"), toml)?;

    let entry_idx = files.iter().position(|(f, c)| c.contains("fn main") || f.contains("main"));
    let is_bin = entry_idx.is_some();
    let entry_idx = entry_idx.unwrap_or(0);
    if files.len() <= 1 {
        let entry = if is_bin { "src/main.rs" } else { "src/lib.rs" };
        std::fs::write(dir.join(entry), files.get(0).map(|f| f.1.as_str()).unwrap_or(""))?;
    } else {
        let mut mods = String::new();
        for (i, (fname, content)) in files.iter().enumerate() {
            if i == entry_idx { continue; }
            let stem = sanitize_pkg(Path::new(fname).file_stem().and_then(|s| s.to_str()).unwrap_or("m"));
            std::fs::write(dir.join("src").join(format!("{stem}.rs")), content)?;
            mods.push_str(&format!("pub mod {stem};\n"));
        }
        let entry = if is_bin { "src/main.rs" } else { "src/lib.rs" };
        std::fs::write(dir.join(entry), format!("{mods}\n{}", files[entry_idx].1))?;
    }
    Ok(dir)
}

/// Offline-first oracle. ALWAYS tries `cargo check --offline` first — hermetic, no network, and it
/// already works for std-only code and for any deps in the local cargo cache (`~/.cargo`, which on a
/// real machine is usually warm). Only if that fails specifically because deps aren't cached
/// (`Verdict::DepsUnavailable`) AND `allow_net` is set does it retry with registry access — which also
/// populates the cache, so the next run for that dep is offline again. The common path touches no
/// network; the reach-out is explicit (opt-in) and rare (first sighting of a crate). Returns the
/// networked result when it retried, otherwise the offline result (honest DepsUnavailable if deps are
/// missing and network wasn't permitted).
pub fn verify_files(name: &str, files: &[(String, String)], root: &Path, allow_net: bool) -> std::io::Result<VerifyResult> {
    let dir = scaffold_cargo(name, files, root)?;
    let mirror_root = root.join("build");
    let offline = verify(&VerifySpec {
        workspace: dir.clone(),
        command: vec!["cargo".into(), "check".into(), "--offline".into()],
        net: false,
        mirror_root: mirror_root.clone(),
    })?;
    if allow_net && classify(offline.passed, &offline.output) == Verdict::DepsUnavailable {
        return verify(&VerifySpec {
            workspace: dir,
            command: vec!["cargo".into(), "check".into()], // networked: cargo resolves + caches
            net: true,
            mirror_root,
        });
    }
    Ok(offline)
}

/// Build the bubblewrap argv: the OS is bound read-only, the toolchain (CARGO_HOME) read-only, the
/// workspace mirror is the ONLY writable real path, /tmp is a fresh tmpfs, HOME points at the mirror,
/// and the network is unshared unless `net`. `--die-with-parent` so a killed UI tears the build down.
/// Pure string assembly (no process spawn) so it's unit-testable without bwrap installed.
pub fn bwrap_argv(mirror: &Path, command: &[String], net: bool, cargo_home: &Path) -> Vec<String> {
    let m = mirror.display().to_string();
    let mut a: Vec<String> = vec!["bwrap".into()];
    // Guaranteed paths — hard binds.
    for path in ["/usr", "/bin", "/lib", "/etc"] {
        a.push("--ro-bind".into()); a.push(path.into()); a.push(path.into());
    }
    // Optional paths — try-binds so bwrap doesn't abort when they're absent:
    //  • /lib64 doesn't exist on aarch64 (Asahi).
    //  • DNS: /etc/resolv.conf is often a symlink into /run on systemd machines, so the resolver
    //    config is unreadable inside the fresh mount namespace unless the TARGET is also bound —
    //    that's why a networked `cargo` fails with "could not resolve host" even though the network
    //    namespace is the host's. Bind the systemd-resolved stub + common resolver files.
    for path in ["/lib64", "/run/systemd/resolve", "/etc/resolv.conf", "/run/resolvconf", "/run/NetworkManager"] {
        a.push("--ro-bind-try".into()); a.push(path.into()); a.push(path.into());
    }
    a.push("--proc".into()); a.push("/proc".into());
    a.push("--dev".into());  a.push("/dev".into());
    a.push("--tmpfs".into()); a.push("/tmp".into());
    // toolchain (registry cache, rustc, std): read-only for an offline build; writable for a networked
    // fetch so downloaded crates persist in the cache and the NEXT offline run finds them.
    let ch = cargo_home.display().to_string();
    a.push(if net { "--bind".into() } else { "--ro-bind".into() });
    a.push(ch.clone()); a.push(ch);
    // the mirror: the single writable real path; cwd + HOME live here
    a.push("--bind".into()); a.push(m.clone()); a.push(m.clone());
    a.push("--chdir".into()); a.push(m.clone());
    a.push("--setenv".into()); a.push("HOME".into()); a.push(m);
    if !net { a.push("--unshare-net".into()); }
    a.push("--die-with-parent".into());
    a.push("--".into());
    a.extend(command.iter().cloned());
    a
}

/// Copy `src` into a fresh dir under `root`, skipping `target/` and `.git/` (never mirror build
/// output or vcs). Returns the mirror path.
pub fn mirror_workspace(src: &Path, root: &Path) -> std::io::Result<PathBuf> {
    let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "ws".into());
    let dst = root.join(format!("{name}-{}", short_id(src)));
    if dst.exists() { let _ = std::fs::remove_dir_all(&dst); }
    std::fs::create_dir_all(&dst)?;
    copy_tree(src, &dst)?;
    Ok(dst)
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        let ns = name.to_string_lossy();
        if ns == "target" || ns == ".git" { continue; } // build output + vcs: never mirror
        let from = e.path();
        let to = dst.join(&name);
        let ft = e.file_type()?;
        if ft.is_dir() { std::fs::create_dir_all(&to)?; copy_tree(&from, &to)?; }
        else if ft.is_file() { std::fs::copy(&from, &to)?; }
        // symlinks/devices skipped deliberately — generated workspaces shouldn't carry them
    }
    Ok(())
}

fn short_id(p: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    format!("{:08x}", h.finish() & 0xffff_ffff)
}

/// Mirror the workspace, then run the command — under bwrap if available, else directly in the mirror.
pub fn verify(spec: &VerifySpec) -> std::io::Result<VerifyResult> {
    let mirror = mirror_workspace(&spec.workspace, &spec.mirror_root)?;
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".cargo"));
    let (prog, args, sandboxed) = if which("bwrap").is_some() {
        let argv = bwrap_argv(&mirror, &spec.command, spec.net, &cargo_home);
        (argv[0].clone(), argv[1..].to_vec(), true)
    } else {
        (spec.command[0].clone(), spec.command[1..].to_vec(), false)
    };
    let mut cmd = Command::new(&prog);
    cmd.args(&args);
    if !sandboxed { cmd.current_dir(&mirror); } // bwrap's --chdir handles this when sandboxed
    let out = cmd.output()?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() { combined.push_str(&err); }
    Ok(VerifyResult { passed: out.status.success(), output: combined, sandboxed, mirror })
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths)
        .map(|d| d.join(bin)).find(|p| p.is_file()))
}
fn home_dir() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")) }

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ls-sbx-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mirror_skips_target_and_git() {
        let src = scratch("src");
        std::fs::create_dir_all(src.join("src")).unwrap();
        std::fs::create_dir_all(src.join("target/debug")).unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(src.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(src.join("target/debug/junk"), "x").unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref").unwrap();

        let root = scratch("root");
        let m = mirror_workspace(&src, &root).unwrap();
        assert!(m.join("Cargo.toml").exists(), "manifest copied");
        assert!(m.join("src/main.rs").exists(), "source copied");
        assert!(!m.join("target").exists(), "target/ skipped");
        assert!(!m.join(".git").exists(), ".git/ skipped");
    }

    #[test]
    fn bwrap_argv_isolates_network_by_default() {
        let argv = bwrap_argv(Path::new("/tmp/mir"), &["cargo".into(), "build".into()], false, Path::new("/root/.cargo"));
        assert_eq!(argv[0], "bwrap");
        assert!(argv.iter().any(|s| s == "--unshare-net"), "network unshared when net=false");
        assert!(argv.windows(3).any(|w| w[0] == "--bind" && w[1] == "/tmp/mir" && w[2] == "/tmp/mir"), "mirror is the writable bind");
        assert!(argv.windows(2).any(|w| w[0] == "--chdir" && w[1] == "/tmp/mir"), "cwd is the mirror");
        assert!(argv.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == "/root/.cargo"), "toolchain bound read-only");
        // command sits after the -- separator, in order
        let sep = argv.iter().position(|s| s == "--").unwrap();
        assert_eq!(&argv[sep + 1..], &["cargo".to_string(), "build".to_string()]);
    }

    #[test]
    fn bwrap_argv_allows_network_when_requested() {
        let argv = bwrap_argv(Path::new("/tmp/m"), &["cargo".into()], true, Path::new("/h/.cargo"));
        assert!(!argv.iter().any(|s| s == "--unshare-net"), "no net unshare when net=true");
    }

    #[test]
    fn bwrap_binds_dns_and_optional_paths() {
        let argv = bwrap_argv(Path::new("/tmp/m"), &["cargo".into()], true, Path::new("/h/.cargo"));
        assert!(argv.windows(2).any(|w| w[0] == "--ro-bind-try" && w[1] == "/run/systemd/resolve"),
                "systemd-resolved stub bound so DNS resolves inside the mount namespace");
        assert!(argv.windows(2).any(|w| w[0] == "--ro-bind-try" && w[1] == "/lib64"),
                "/lib64 is a try-bind (absent on aarch64 Asahi, must not abort bwrap)");
        // cargo cache writable during a networked fetch so it persists
        assert!(argv.windows(3).any(|w| w[0] == "--bind" && w[1] == "/h/.cargo"), "cargo cache rw when net");
    }

    #[test]
    fn detect_deps_finds_extern_crates_not_std() {
        let files = [("m.rs".to_string(), "use rand::Rng;\nuse std::cmp;\nuse serde::Serialize;\nfn main(){}".to_string())];
        let d = detect_deps(&files);
        assert!(d.contains(&"rand".to_string()) && d.contains(&"serde".to_string()), "extern crates detected: {d:?}");
        assert!(!d.contains(&"std".to_string()), "std is not a dependency");
    }

    #[test]
    fn scaffold_picks_bin_vs_lib() {
        let root = scratch("scaf");
        let bin = scaffold_cargo("x", &[("a.rs".to_string(), "fn main(){}".to_string())], &root).unwrap();
        assert!(bin.join("src/main.rs").exists(), "fn main ⇒ binary");
        let lib = scaffold_cargo("y", &[("a.rs".to_string(), "pub fn f(){}".to_string())], &root).unwrap();
        assert!(lib.join("src/lib.rs").exists(), "no main ⇒ lib");
    }

    #[test]
    fn verify_files_catches_a_real_model_bug_offline() {
        // The kind of thing the 1B emitted: an infinitely-sized recursive type (E0072), std-only.
        let broken = "struct Node { value: i32, next: Option<Node> }\nfn main() { let _ = Node { value: 1, next: None }; }";
        let root = scratch("vf");
        let res = verify_files("sortcrate", &[("main.rs".to_string(), broken.to_string())], &root, false).unwrap();
        assert!(!res.passed, "the compiler rejects the broken code — the oracle works");
        assert_eq!(classify(res.passed, &res.output), Verdict::CompileError, "classified as a real compile error, not a dep/tool issue");
    }

    #[test]
    fn offline_first_reports_deps_unavailable_without_network() {
        // A crate that cannot resolve offline (not in any cache) + network NOT permitted ⇒ honest
        // DepsUnavailable, not a build failure. (A *cached* dep like rand would instead resolve offline
        // with no network — the offline-first win — so we use a guaranteed-uncached name here.)
        let code = "use zzz_uncached_crate_4242::Thing;\nfn main() { let _ = Thing; }";
        let root = scratch("vf-offline");
        let res = verify_files("needsdep", &[("main.rs".to_string(), code.to_string())], &root, false).unwrap();
        assert!(!res.passed);
        assert_eq!(classify(res.passed, &res.output), Verdict::DepsUnavailable,
                   "uncached dep offline ⇒ DepsUnavailable, awaiting opt-in: {}", res.output.chars().take(200).collect::<String>());
    }

    #[test]
    fn classify_distinguishes_network_from_broken_code() {
        assert_eq!(classify(true, ""), Verdict::Pass);
        // the exact M2 failure: a dependency download couldn't reach the registry
        let net = "Updating crates.io index\nwarning: spurious network error\nerror: failed to get `rand` as a dependency\nCaused by:\n  Could not resolve host: index.crates.io";
        assert_eq!(classify(false, net), Verdict::DepsUnavailable, "network failure is NOT a build failure");
        // a real compile error
        assert_eq!(classify(false, "error[E0072]: recursive type `Node` has infinite size\n --> src/main.rs:1:1"), Verdict::CompileError);
        assert_eq!(classify(false, "error: cannot find value `rght` in this scope"), Verdict::CompileError);
    }
}
