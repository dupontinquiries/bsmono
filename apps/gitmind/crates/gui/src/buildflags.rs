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

/// `cross build` argv for `app`/`triple` with a chosen feature set. `no_default`
/// adds `--no-default-features` (so the chosen set is exactly what's built).
/// Pure — unit-tested.
pub fn cargo_build_argv(app: &str, triple: &str, features: &[String], no_default: bool) -> Vec<String> {
    let mut a: Vec<String> = ["cross", "build", "--release", "--target", triple, "-p", app]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if no_default {
        a.push("--no-default-features".into());
    }
    if !features.is_empty() {
        a.push("--features".into());
        a.push(features.join(","));
    }
    a
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

/// Where a built binary lands on the receiving device. Pure — tested.
pub fn install_dir(home: &str) -> String {
    format!("{}/.local/bin", home.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
