//! Build-flag customisation for the app distributor: scan an app's *available*
//! knobs (cargo `[features]`, CMake `option(...)`), work out the repo's default set
//! **per target architecture**, and build the cross-compile command from a chosen
//! set. The client gets the flags with the per-arch defaults pre-enabled; it can
//! toggle from there. All of this is pure text/parse work and is unit-tested; the
//! `cross build` it produces runs on the server (on-device).
//!
//! Per-arch default convention (documented in maintenance/DISTRIBUTION.md):
//! a repo may add, in the app's Cargo.toml,
//!   [package.metadata.gitmind]
//!   default-features-musl = ["small", "no-x11"]
//!   default-features-gnu  = ["full"]
//! Whichever matches the target's libc wins; otherwise the plain `[features]
//! default` set is used.

/// Target triples the fleet builds for (both devices are aarch64).
pub const BUILD_TARGETS: [&str; 2] = [
    "aarch64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
];

/// The cargo features an app exposes, plus its `default` set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CargoFeatures {
    pub all: Vec<String>,     // every feature except `default`
    pub default: Vec<String>, // the `default = [...]` set
}

/// A CMake `option(NAME "help" DEFAULT)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmakeOption {
    pub name: String,
    pub default_on: bool,
    pub help: String,
}

/// Pull quoted/bare items out of an inline array fragment like `["a", "b"]`.
fn parse_inline_array(s: &str) -> Vec<String> {
    let inside = match (s.find('['), s.rfind(']')) {
        (Some(a), Some(b)) if b > a => &s[a + 1..b],
        _ => return Vec::new(),
    };
    inside
        .split(',')
        .map(|t| t.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Scan a Cargo.toml's `[features]` table. Pure — unit-tested. (Single-line feature
/// definitions, which is the overwhelmingly common form.)
pub fn scan_cargo_features(toml: &str) -> CargoFeatures {
    let mut out = CargoFeatures::default();
    let mut in_features = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_features = t == "[features]";
            continue;
        }
        if !in_features || t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((name, rhs)) = t.split_once('=') {
            let name = name.trim().trim_matches('"');
            if name.is_empty() {
                continue;
            }
            if name == "default" {
                out.default = parse_inline_array(rhs);
            } else {
                out.all.push(name.to_string());
            }
        }
    }
    out
}

/// Read a scalar string `key = "value"` from a named section header. Pure.
fn read_section_string(toml: &str, header: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, rhs)) = t.split_once('=') {
            if k.trim().trim_matches('"') == key {
                return Some(rhs.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    None
}

/// The crate's own name from `[package] name` — the app's real name, not its folder.
/// Pure — unit-tested.
pub fn crate_name(toml: &str) -> Option<String> {
    read_section_string(toml, "[package]", "name").filter(|s| !s.is_empty())
}

/// The actual cargo package/binary identifier to build with — `app`'s real name can differ from
/// the manifest/display id it's known by (e.g. voiceterm's crate is still named "web2qml" from
/// before the app was renamed, so `-p voiceterm` / a `target/release/voiceterm` binary lookup both
/// fail even though the app itself builds fine). Reads the REAL `[package] name` from `toml` when
/// present; falls back to `app` when there's no `[package]` section to read at all (the workspace-
/// root case, where `app` already names the real binary via `--bin`). Pure — unit-tested.
pub fn build_id(app: &str, toml: &str) -> String {
    crate_name(toml).unwrap_or_else(|| app.to_string())
}

/// The crate's `[package] description`, if any. Pure — unit-tested.
pub fn package_description(toml: &str) -> Option<String> {
    read_section_string(toml, "[package]", "description").filter(|s| !s.is_empty())
}

/// Read `key = [...]` from a named section header (e.g. `[package.metadata.gitmind]`).
fn read_section_array(toml: &str, header: &str, key: &str) -> Option<Vec<String>> {
    let mut in_section = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, rhs)) = t.split_once('=') {
            if k.trim().trim_matches('"') == key {
                return Some(parse_inline_array(rhs));
            }
        }
    }
    None
}

/// The repo's default feature set **for a given target**: a per-arch override from
/// `[package.metadata.gitmind]` if present, else the plain `[features] default`.
/// Pure — unit-tested.
pub fn arch_default_features(toml: &str, triple: &str) -> Vec<String> {
    let key = if triple.contains("musl") {
        Some("default-features-musl")
    } else if triple.contains("gnu") {
        Some("default-features-gnu")
    } else {
        None
    };
    if let Some(k) = key {
        if let Some(v) = read_section_array(toml, "[package.metadata.gitmind]", k) {
            return v;
        }
    }
    scan_cargo_features(toml).default
}

/// Every feature paired with whether it should start checked for `triple` — the exact shape a
/// checkbox list needs (name + on), computed once so Qt's Fleet.qml and gitmind-egui's fleet page
/// don't each re-derive "which features are on by default" independently. Order matches
/// `scan_cargo_features`'s `all` (declaration order in Cargo.toml). Pure — unit-tested.
pub fn feature_toggles(toml: &str, triple: &str) -> Vec<(String, bool)> {
    let feats = scan_cargo_features(toml);
    let defaults = arch_default_features(toml, triple);
    feats.all.into_iter().map(|name| { let on = defaults.contains(&name); (name, on) }).collect()
}

/// Scan CMakeLists.txt for `option(...)`. Pure — unit-tested. (One option per line,
/// the common form.)
pub fn scan_cmake_options(text: &str) -> Vec<CmakeOption> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if !lower.starts_with("option(") && !lower.starts_with("option (") {
            continue;
        }
        let inner = match (t.find('('), t.rfind(')')) {
            (Some(a), Some(b)) if b > a => &t[a + 1..b],
            _ => continue,
        };
        // NAME "help text" DEFAULT
        let name = inner.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let help = inner
            .find('"')
            .and_then(|a| inner[a + 1..].find('"').map(|b| inner[a + 1..a + 1 + b].to_string()))
            .unwrap_or_default();
        let last = inner.split_whitespace().last().unwrap_or("");
        let default_on = last.eq_ignore_ascii_case("on") || last == "1" || last.eq_ignore_ascii_case("true");
        out.push(CmakeOption { name, default_on, help });
    }
    out
}

/// `cargo build` argv for `app`/`triple` with a chosen feature set. `no_default`
/// adds `--no-default-features` (so the chosen set is exactly what's built). Uses
/// plain `cargo` with `--target` (no container needed); the target's std must be
/// installed (`rustup target add <triple>`). Pure — unit-tested.
/// `cargo build` argv. An empty `triple` means a **native** build (no `--target`, so
/// no rustup target/linker is needed — the host toolchain builds for itself); a
/// non-empty triple cross-builds for that target. Pure — unit-tested.
pub fn cargo_build_argv(app: &str, triple: &str, features: &[String], no_default: bool) -> Vec<String> {
    cargo_build_argv_kind(app, triple, features, no_default, false)
}

/// Does this Cargo.toml declare a `[workspace]`? A workspace root with no `[package]` can't be built
/// with `-p <dir-name>` (that names a package, which doesn't exist) — its app is a **binary** across
/// members, built with `--bin`.
pub fn declares_workspace(cargo_toml: &str) -> bool {
    cargo_toml.lines().any(|l| l.trim_start().starts_with("[workspace]"))
}

/// Like [`cargo_build_argv`], but `as_bin` selects `--bin <app>` instead of `-p <app>` — used for a
/// workspace-root app whose runnable binary is named after it (e.g. gitmind: package `gitmind-cli`,
/// binary `gitmind`), where `-p gitmind` would fail with "did not match any packages".
pub fn cargo_build_argv_kind(app: &str, triple: &str, features: &[String], no_default: bool, as_bin: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["cargo".into(), "build".into(), "--release".into()];
    if !triple.trim().is_empty() {
        a.push("--target".into());
        a.push(triple.to_string());
    }
    a.push(if as_bin { "--bin".into() } else { "-p".into() });
    a.push(app.to_string());
    if no_default {
        a.push("--no-default-features".into());
    }
    if !features.is_empty() {
        a.push("--features".into());
        a.push(features.join(","));
    }
    a
}

/// The libc family of a target triple: "musl" or "gnu". Pure.
pub fn target_libc(triple: &str) -> &'static str {
    if triple.contains("musl") { "musl" } else { "gnu" }
}

/// Normalize a rustc host triple to the GUI's target vocabulary, mapping vendor
/// triples like `aarch64-alpine-linux-musl` to `aarch64-unknown-linux-musl` so the
/// "this device" pill matches and the libc is unambiguous. Pure — unit-tested.
pub fn normalize_native(host_triple: &str) -> String {
    let arch = host_triple.split('-').next().unwrap_or("aarch64");
    format!("{arch}-unknown-linux-{}", target_libc(host_triple))
}

/// True when building `requested` on a host whose native triple is `native` needs no
/// cross toolchain (same libc family). Pure — unit-tested.
pub fn is_native_build(requested: &str, native: &str) -> bool {
    target_libc(requested) == target_libc(native)
}

/// The architecture of a triple (the part before the first `-`), e.g. `aarch64`.
pub fn arch_of(triple: &str) -> &str {
    triple.split('-').next().unwrap_or("")
}

/// How gitmind should build for a given target device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployStrategy {
    /// Build locally with no cross toolchain (same arch + libc as this host). Fastest.
    LocalNative,
    /// Build locally with `--target` + a cross-linker, then copy the binary over.
    CrossCompile,
    /// Build on the target device itself (native there) — the fallback when we can't cross-compile
    /// (e.g. no musl linker on the desktop) but can reach the device over ssh.
    RemoteNative,
}

/// A chosen strategy plus the reasoning, so the UI can explain *why* a device deploys the way it
/// does instead of silently picking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyChoice {
    pub strategy: DeployStrategy,
    pub reason: String,
    /// Whether this path needs a cross-linker to be present (a precondition to check/install).
    pub needs_cross_linker: bool,
}

/// Recommend a deploy strategy for a target device given this host's native triple, whether a
/// cross-linker for the target is available, and whether the device is reachable over ssh.
///
/// The fleet's common case is same-arch (aarch64) but different libc (gnu desktop → musl phone):
/// with a musl linker present we cross-compile and copy; without one we build on the phone itself
/// (native there), which is slower but needs no cross toolchain. Pure — unit-tested.
pub fn recommend_strategy(
    host_native: &str,
    target: &str,
    have_cross_linker: bool,
    remote_reachable: bool,
) -> StrategyChoice {
    let same_arch = arch_of(host_native) == arch_of(target);
    let same_libc = target_libc(host_native) == target_libc(target);
    if same_arch && same_libc {
        return StrategyChoice {
            strategy: DeployStrategy::LocalNative,
            reason: "same arch + libc as this host — no cross toolchain needed".to_string(),
            needs_cross_linker: false,
        };
    }
    let mismatch = if !same_arch {
        format!("{} vs {}", arch_of(host_native), arch_of(target))
    } else {
        format!("{} vs {} libc", target_libc(host_native), target_libc(target))
    };
    if have_cross_linker {
        StrategyChoice {
            strategy: DeployStrategy::CrossCompile,
            reason: format!("differs from host ({mismatch}); cross-linker present → cross-compile + copy"),
            needs_cross_linker: true,
        }
    } else if remote_reachable {
        StrategyChoice {
            strategy: DeployStrategy::RemoteNative,
            reason: format!("differs from host ({mismatch}) and no cross-linker → build on the device (native there)"),
            needs_cross_linker: false,
        }
    } else {
        // can't cross, can't reach — surface the blocker; cross-compile is the only path once a
        // linker is installed.
        StrategyChoice {
            strategy: DeployStrategy::CrossCompile,
            reason: format!("differs from host ({mismatch}), device unreachable, no cross-linker — install a {} linker to cross-compile", target_libc(target)),
            needs_cross_linker: true,
        }
    }
}

/// `cmake -S . -B <build_dir> -D…` argv for configuring a CMake app for a target.
/// A toolchain file (per-arch) is how CMake cross-compiles; it's optional here so
/// the command is well-formed even before one exists. Pure — unit-tested.
pub fn cmake_configure_argv(build_dir: &str, opts: &[(String, bool)], toolchain: Option<&str>) -> Vec<String> {
    let mut a = vec![
        "cmake".to_string(),
        "-S".into(), ".".into(),
        "-B".into(), build_dir.into(),
        "-DCMAKE_BUILD_TYPE=Release".into(),
    ];
    if let Some(tc) = toolchain {
        a.push(format!("-DCMAKE_TOOLCHAIN_FILE={tc}"));
    }
    for (name, on) in opts {
        a.push(format!("-D{name}={}", if *on { "ON" } else { "OFF" }));
    }
    a
}

/// Path of the freshly built cargo binary inside the build tree. Pure — tested.
pub fn built_binary_path(app: &str, triple: &str) -> String {
    format!("target/{triple}/release/{app}")
}

/// Extract a top-level JSON string value by key from a one-line JSON blob (e.g. the
/// `target_directory` from `cargo metadata`). Minimal + std-only; handles `\"`/`\\`
/// escapes, returns None for null/absent. Pure — unit-tested.
pub fn json_string_field(blob: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut i = blob.find(&pat)? + pat.len();
    let b = blob.as_bytes();
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') { i += 1; }
    if i >= b.len() || b[i] != b':' { return None; }
    i += 1;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') { i += 1; }
    let rest = &blob[i..];
    if rest.starts_with("null") { return None; }
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => { if let Some(n) = chars.next() { out.push(n); } }
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

/// The host triple from `rustc -vV` output (the `host:` line) — the authoritative
/// native target, instead of guessing libc from loader files. Pure — unit-tested.
pub fn host_triple_from_rustc(vv: &str) -> Option<String> {
    vv.lines()
        .find_map(|l| l.trim().strip_prefix("host:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a Cargo.toml declares an explicit `[[bin]]` target. (A crate is also a
/// binary if it has src/main.rs, checked separately on disk.) Pure — unit-tested.
pub fn declares_binary(cargo_toml: &str) -> bool {
    cargo_toml.lines().any(|l| l.trim() == "[[bin]]")
}

/// The cargo target directory (absolute) from `cargo metadata --format-version 1`.
/// Correct for both standalone crates and workspace members, so the built binary is
/// found wherever cargo actually writes it in a monorepo. Pure — unit-tested.
pub fn parse_target_directory(metadata: &str) -> Option<String> {
    json_string_field(metadata, "target_directory")
}

/// Where a built binary lands on the receiving device. Pure — tested.
pub fn install_dir(home: &str) -> String {
    format!("{}/.local/bin", home.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_root_builds_bin_not_package() {
        use super::{cargo_build_argv_kind, declares_workspace, crate_name};
        let ws = "[workspace]\nmembers = [\"crates/cli\"]\n";
        // gitmind case: workspace root, no [package] → build --bin, not -p
        assert!(declares_workspace(ws) && crate_name(ws).is_none());
        let argv = cargo_build_argv_kind("gitmind", "", &[], false, true);
        assert_eq!(argv, vec!["cargo", "build", "--release", "--bin", "gitmind"]);
        // normal package case stays -p
        let pkg = "[package]\nname = \"beatspace\"\n";
        assert!(crate_name(pkg).is_some());
        let argv2 = cargo_build_argv_kind("beatspace", "", &[], false, false);
        assert_eq!(argv2, vec!["cargo", "build", "--release", "-p", "beatspace"]);
    }

    use super::*;

    #[test]
    fn build_id_uses_the_real_package_name_when_it_differs_from_the_app_id() {
        // the exact bug report: voiceterm's crate is still named "web2qml" (kept from before the
        // app was renamed) — `-p voiceterm` / a `target/release/voiceterm` binary lookup both fail
        // with cargo's "package ID specification did not match any packages" even though the app
        // itself builds fine. build_id must resolve to the REAL name, not the manifest/display id.
        let voiceterm_toml = "[package]\nname = \"web2qml\"\nversion = \"0.1.0\"\n\n[features]\ndefault = []\nvoice = [\"whisper-rs\"]\n";
        assert_eq!(build_id("voiceterm", voiceterm_toml), "web2qml");
        // the common case: package name matches the app id — no surprise remapping
        let normal_toml = "[package]\nname = \"logscope\"\n";
        assert_eq!(build_id("logscope", normal_toml), "logscope");
        // no [package] at all (a bare workspace root, e.g. gitmind's `gitmind-cli` crate under the
        // `gitmind` app id) — falls back to the app id itself, matching the existing --bin path
        assert_eq!(build_id("gitmind", "[workspace]\nmembers = [\"crates/*\"]\n"), "gitmind");
    }

    #[test]
    fn feature_toggles_matches_defaults_and_preserves_declaration_order() {
        let toml = "[package]\nname = \"x\"\n[features]\ndefault = [\"b\"]\na = []\nb = []\nc = []\n\n[package.metadata.gitmind]\ndefault-features-musl = [\"a\", \"c\"]\n";
        // native (no triple match) falls through to the plain [features] default = [\"b\"]
        let native = feature_toggles(toml, "");
        assert_eq!(native.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"], "order matches declaration, not the default set");
        assert_eq!(native, vec![("a".into(), false), ("b".into(), true), ("c".into(), false)]);
        // musl triple picks up the per-arch override instead
        let musl = feature_toggles(toml, "aarch64-unknown-linux-musl");
        assert_eq!(musl, vec![("a".into(), true), ("b".into(), false), ("c".into(), true)]);
    }

    #[test]
    fn native_build_omits_target() {
        // native → no --target (works on Alpine/musl and Fedora/gnu without rustup targets)
        let n = cargo_build_argv("gitmind", "", &[], false);
        assert!(!n.contains(&"--target".to_string()), "native build has no --target: {n:?}");
        assert_eq!(n[n.iter().position(|x| x == "-p").unwrap() + 1], "gitmind");
        // cross → --target present
        let c = cargo_build_argv("gitmind", "aarch64-unknown-linux-musl", &[], false);
        assert!(c.windows(2).any(|w| w[0] == "--target" && w[1] == "aarch64-unknown-linux-musl"));
    }

    #[test]
    fn triple_normalization_and_native_check() {
        assert_eq!(normalize_native("aarch64-alpine-linux-musl"), "aarch64-unknown-linux-musl");
        assert_eq!(normalize_native("aarch64-unknown-linux-gnu"), "aarch64-unknown-linux-gnu");
        assert_eq!(target_libc("aarch64-alpine-linux-musl"), "musl");
        // phone (musl native) building musl = native; building gnu = cross
        assert!(is_native_build("aarch64-unknown-linux-musl", "aarch64-alpine-linux-musl"));
        assert!(!is_native_build("aarch64-unknown-linux-gnu", "aarch64-alpine-linux-musl"));
    }

    #[test]
    fn host_triple_and_binary_detection() {
        let vv = "rustc 1.96.0\nbinary: rustc\nhost: aarch64-unknown-linux-gnu\nrelease: 1.96.0\n";
        assert_eq!(host_triple_from_rustc(vv).as_deref(), Some("aarch64-unknown-linux-gnu"));
        assert_eq!(host_triple_from_rustc("no host line here"), None);
        // a lib-only crate declares no [[bin]]
        assert!(!declares_binary("[package]\nname = \"share-kit\"\n[lib]\n"));
        assert!(declares_binary("[package]\nname = \"x\"\n[[bin]]\nname = \"x\"\npath = \"src/m.rs\"\n"));
    }

    #[test]
    fn target_directory_from_metadata() {
        // workspace member → workspace target; standalone subdir crate → its own target
        let ws = r#"{"packages":[{"name":"gitmind"}],"target_directory":"/tmp11/target","version":1}"#;
        assert_eq!(parse_target_directory(ws).as_deref(), Some("/tmp11/target"));
        let sub = r#"{"workspace_root":"/tmp11/apps/av1tuner","target_directory":"/tmp11/apps/av1tuner/target"}"#;
        assert_eq!(parse_target_directory(sub).as_deref(), Some("/tmp11/apps/av1tuner/target"));
        assert_eq!(parse_target_directory(r#"{"target_directory":null}"#), None);
        assert_eq!(parse_target_directory(r#"{"other":1}"#), None);
    }

    const CARGO: &str = r#"
[package]
name = "av1tuner"

[package.metadata.gitmind]
default-features-musl = ["small"]
default-features-gnu  = ["full", "gpu"]

[features]
default = ["small", "logging"]
small = []
full = ["gpu", "simd"]
gpu = []
simd = []
logging = []
"#;

    #[test]
    fn reads_crate_name_and_description() {
        let toml = "[package]\nname = \"av1-tuner\"\nversion = \"0.2.0\"\ndescription = \"Tunes AV1 encodes\"\n\n[features]\ndefault = []\n";
        assert_eq!(crate_name(toml).as_deref(), Some("av1-tuner"));
        assert_eq!(package_description(toml).as_deref(), Some("Tunes AV1 encodes"));
        // a virtual workspace (no [package]) has no crate name
        assert_eq!(crate_name("[workspace]\nmembers = [\"a\"]\n"), None);
        assert_eq!(package_description("[package]\nname = \"x\"\n"), None);
    }

    #[test]
    fn scans_cargo_features_and_default() {
        let f = scan_cargo_features(CARGO);
        assert_eq!(f.default, vec!["small", "logging"]);
        for want in ["small", "full", "gpu", "simd", "logging"] {
            assert!(f.all.contains(&want.to_string()), "missing {want}");
        }
        assert!(!f.all.contains(&"default".to_string()), "default is not a togglable feature");
    }

    #[test]
    fn per_arch_defaults_override_else_fall_back() {
        assert_eq!(arch_default_features(CARGO, "aarch64-unknown-linux-musl"), vec!["small"]);
        assert_eq!(arch_default_features(CARGO, "aarch64-unknown-linux-gnu"), vec!["full", "gpu"]);
        // a triple with no metadata + no libc match falls back to [features] default
        assert_eq!(arch_default_features(CARGO, "riscv64gc-unknown-none"), vec!["small", "logging"]);
        // no metadata table at all -> default
        assert_eq!(
            arch_default_features("[features]\ndefault = [\"a\"]\na = []\n", "x-musl"),
            vec!["a"]
        );
    }

    #[test]
    fn scans_cmake_options() {
        let cm = "cmake_minimum_required(VERSION 3.20)\n\
                  option(WITH_QT \"build the Qt GUI\" ON)\n\
                  option(WITH_TESTS \"unit tests\" OFF)\n\
                  add_subdirectory(src)\n";
        let o = scan_cmake_options(cm);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0], CmakeOption { name: "WITH_QT".into(), default_on: true, help: "build the Qt GUI".into() });
        assert!(!o[1].default_on);
    }

    #[test]
    fn cargo_build_argv_honours_features() {
        let a = cargo_build_argv("av1tuner", "aarch64-unknown-linux-musl", &["small".into()], true);
        assert_eq!(a[0], "cargo", "plain cargo, no container");
        assert!(a.contains(&"--no-default-features".into()));
        assert!(a.windows(2).any(|w| w[0] == "--features" && w[1] == "small"));
        assert_eq!(a[a.iter().position(|x| x == "-p").unwrap() + 1], "av1tuner");
        // no features, keep defaults -> neither flag
        let b = cargo_build_argv("x", "t", &[], false);
        assert!(!b.contains(&"--no-default-features".into()) && !b.contains(&"--features".into()));
    }

    #[test]
    fn cmake_configure_argv_emits_flags_and_toolchain() {
        let a = cmake_configure_argv("build-musl", &[("WITH_QT".into(), true), ("WITH_TESTS".into(), false)], Some("/tc/aarch64-musl.cmake"));
        assert!(a.contains(&"-DWITH_QT=ON".into()));
        assert!(a.contains(&"-DWITH_TESTS=OFF".into()));
        assert!(a.iter().any(|x| x.starts_with("-DCMAKE_TOOLCHAIN_FILE=")));
    }

    #[test]
    fn paths() {
        assert_eq!(built_binary_path("v", "aarch64-unknown-linux-musl"), "target/aarch64-unknown-linux-musl/release/v");
        assert_eq!(install_dir("/home/kesslerd/"), "/home/kesslerd/.local/bin");
    }

    #[test]
    fn strategy_for_fleet_scenarios() {
        let desktop = "aarch64-unknown-linux-gnu"; // Asahi Fedora
        let phone = "aarch64-unknown-linux-musl"; // postmarketOS/Alpine
        // desktop -> desktop: local native
        assert_eq!(recommend_strategy(desktop, desktop, false, true).strategy, DeployStrategy::LocalNative);
        // desktop -> phone with a musl linker: cross-compile + copy
        let cc = recommend_strategy(desktop, phone, true, true);
        assert_eq!(cc.strategy, DeployStrategy::CrossCompile);
        assert!(cc.needs_cross_linker);
        // desktop -> phone, no linker but reachable: build on the phone (native there)
        assert_eq!(recommend_strategy(desktop, phone, false, true).strategy, DeployStrategy::RemoteNative);
        // desktop -> phone, no linker AND unreachable: surface the blocker (cross is the only path)
        let blocked = recommend_strategy(desktop, phone, false, false);
        assert_eq!(blocked.strategy, DeployStrategy::CrossCompile);
        assert!(blocked.reason.contains("musl"));
        // cross-arch (x86 desktop -> aarch64 phone) with a linker: cross-compile, reason names the arch gap
        let xarch = recommend_strategy("x86_64-unknown-linux-gnu", phone, true, true);
        assert_eq!(xarch.strategy, DeployStrategy::CrossCompile);
        assert!(xarch.reason.contains("x86_64") && xarch.reason.contains("aarch64"));
    }
}
