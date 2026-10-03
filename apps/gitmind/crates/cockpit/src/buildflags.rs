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
/// tideshell's `Cargo.toml` for reference: ticking only `layer` in the Push page's feature grid
/// silently builds a keyboard that shows and never hides, or a shell with no
/// window focus tracking. That exact gap shipped once already (the fleet
/// store's own tideshell/rusty-board entries had it, fixed separately) -- this
/// is the same mistake, available again through the OTHER path to a build: a
/// person hand-picking flags on the Push page.
///
/// "COSMIC + tideshell" in the UI is this pair, named for what it is for
/// rather than what it does technically -- the person picking it should not
/// need to already know these two features interact.
///
/// `ei` is in the set because the `tideshell` binary declares
/// `required-features = ["layer", "ei"]`: a build with `wayland,layer` and no `ei` compiles the
/// library, SKIPS the binary without a word, and finishes "successfully" with nothing to install
/// (the Push page's "no binary at .../target/release/tideshell", then a misleading cross-compile
/// fallback). `the_set_satisfies_tideshells_own_required_features` reads the real manifest.
pub const COSMIC_TIDESHELL_FEATURES: [&str; 3] = ["wayland", "layer", "ei"];

/// Turn a feature selection into one with `COSMIC_TIDESHELL_FEATURES` present,
/// preserving every other feature and their relative order, and never
/// duplicating one already on. Pure, so the Push page's switch is testable
/// without a Cargo.toml or a build.
pub fn with_cosmic_tideshell(mut features: Vec<String>) -> Vec<String> {
    for f in COSMIC_TIDESHELL_FEATURES {
        if !features.iter().any(|x| x == f) {
            features.push(f.to_string());
        }
    }
    features
}

/// Is a feature selection ALREADY the COSMIC + tideshell one -- both required
/// features present? Order-independent and ignores extras (dictation's
/// `voice`, say), so the switch reflects reality even after a person adds
/// something on top of it.
pub fn is_cosmic_tideshell(features: &[String]) -> bool {
    COSMIC_TIDESHELL_FEATURES.iter().all(|f| features.iter().any(|x| x == f))
}

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

/// The name of the ARTIFACT cargo will produce, which is not always the
/// package name.
///
/// `build_id` answers "what do I pass to `-p`" and is the package name. This
/// answers "what file will appear in target/release", and an explicit
/// `[[bin]] name = ...` overrides the package:
///
/// ```text
///   [package] name = "studio-app"     ->  -p studio-app
///   [[bin]]   name = "studio"         ->  target/release/studio
/// ```
///
/// Conflating the two is what produced "✓ built (0 KB)" and then, once that
/// was made an error, "cargo succeeded but no binary at .../studio-app". The
/// build was right both times; only the lookup was wrong.
///
/// Falls back to `build_id` when there is no `[[bin]]` name, which is the
/// common case where package and binary agree.
pub fn artifact_name(app: &str, toml: &str) -> String {
    let bins = bin_names(toml);
    // A bin named after the APP ID wins over position.
    //
    // This used to take the first `[[bin]]` unconditionally, on the reasoning
    // that "a crate with several bins lists the primary one first by
    // convention". `apps/settings-app` does not: it lists `bsettings-cli`
    // first and `bsettings` (the GUI, behind `required-features = ["gui"]`)
    // second. So gitmind built the store entry `bsettings` with the `gui`
    // feature on, then went looking for the artifact and found the COMMAND
    // LINE TOOL, pinned it under the name `bsettings`, and gave the run button
    // a binary that prints usage and exits. Reported as "bsettings-cli does not
    // get the launch gui treatment", which is exact: the thing being launched
    // was bsettings-cli.
    //
    // Matching the app ID rather than the package name keeps the case the
    // original comment was defending — voiceterm's crate is still named
    // `web2qml`, studio's package is `studio-app` — because in both of those
    // the bin IS named after the app, and neither has a second bin to be
    // confused by. Position remains the fallback for a crate whose single bin
    // is named after neither.
    if bins.iter().any(|b| b == app) {
        return app.to_string();
    }
    if let Some(n) = bins.into_iter().next() {
        return n;
    }
    build_id(app, toml)
}

/// Every `[[bin]] name = "..."` in a manifest, in declaration order.
fn bin_names(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_bin = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_bin = t == "[[bin]]";
            continue;
        }
        if in_bin {
            if let Some(rest) = t.strip_prefix("name") {
                if let Some(v) = rest.split('=').nth(1) {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        out.push(v.to_string());
                        in_bin = false; // one name per [[bin]]
                    }
                }
            }
        }
    }
    out
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
/// Environment a cross build needs on top of `--target`.
///
/// `cargo build --target aarch64-...` compiles fine and then fails at LINK
/// time, because cargo invokes the host `cc` for the target's objects. The
/// error is a wall of `unrecognized opcode` or "incompatible" from the host
/// linker, which does not mention cross-compilation at all — so it reads as a
/// broken toolchain rather than a missing setting.
///
/// One variable per target fixes it:
/// `CARGO_TARGET_<TRIPLE_UPPER_UNDERSCORED>_LINKER`.
///
/// Returned as pairs rather than applied, so a caller can print them for a dry
/// run and so this is testable with no toolchain present — which matters,
/// because a sandbox without `rustup` cannot install a target's std and
/// therefore cannot prove a cross build works. It can still prove the argv and
/// the environment are right.
///
/// An empty `triple` is a native build and needs nothing: the host `cc` IS the
/// right linker.
/// Does this app open a window? A GUI app has a runtime requirement a CLI does not — see
/// [`gui_env`].
///
/// Read from the app's Cargo.toml rather than a list of names: an app that depends on
/// eframe, winit or egui-winit draws a window, whatever it is called.
pub fn is_gui_app(toml: &str) -> bool {
    const WINDOWING: [&str; 4] = ["eframe", "winit", "egui-winit", "egui_winit"];
    toml.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .any(|l| WINDOWING.iter().any(|w| l.starts_with(w) || l.starts_with(&format!("{w} ")) || l.starts_with(&format!("{w}="))))
}

/// Does any manifest under `dir` declare a windowing dependency?
///
/// [`is_gui_app`] reads ONE manifest, and for two apps in this fleet that is the wrong one:
///
/// * `apps/voiceterm/Cargo.toml` is the `web2qml` package; eframe lives in
///   `apps/voiceterm/egui-frontend/Cargo.toml`.
/// * `apps/snapclone` has no top-level `Cargo.toml` at all — it is a CMake project with the
///   egui frontend in a subdirectory — so the read fails and the string is empty.
///
/// Both therefore looked like CLI apps, missed the `-crt-static` flag, and shipped a static
/// musl binary that cannot dlopen libwayland-client. winit reports that as a Wayland error
/// and it reads as a missing system library on the phone, which is the wrong thing to go
/// looking for.
///
/// Searches the root, every immediate subdirectory, and `crates/*` — the three places this
/// fleet puts a frontend. Not a full walk: a bounded search that names where it looked is
/// better than one that wanders into `target/` and takes a minute.
pub fn is_gui_app_dir(dir: &std::path::Path) -> bool {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    if is_gui_app(&read(dir.join("Cargo.toml"))) {
        return true;
    }
    let mut roots = vec![dir.to_path_buf()];
    if dir.join("crates").is_dir() {
        roots.push(dir.join("crates"));
    }
    for r in roots {
        let Ok(entries) = std::fs::read_dir(&r) else { continue };
        for e in entries.flatten() {
            if !e.path().is_dir() || e.file_name() == "target" {
                continue;
            }
            if is_gui_app(&read(e.path().join("Cargo.toml"))) {
                return true;
            }
        }
    }
    false
}

/// Extra environment a GUI app needs for a given target.
///
/// # A static musl build of a windowed app compiles and cannot start
///
/// winit does not link libwayland-client or libX11: it **dlopens** them at run time, which
/// is what lets one binary work on a Wayland session, an X11 session, or neither. rustc
/// links musl targets statically by default, and a static binary has no dynamic loader —
/// `dlopen` returns null for everything. winit then reports `NoWaylandLib`, the X11 backend
/// fails the same way, and eframe exits with "Failed to initialize any backend". The build
/// succeeded; the app cannot open a window on any machine, including one with both
/// libraries installed.
///
/// `-C target-feature=-crt-static` makes the musl build dynamic, which restores dlopen.
/// The binary then needs musl's loader on the device, which is exactly the Alpine /
/// postmarketOS case this target exists for.
///
/// Nothing for gnu targets: they are already dynamic.
pub fn gui_env(triple: &str) -> Vec<(String, String)> {
    if !triple.contains("musl") {
        return Vec::new();
    }
    vec![("RUSTFLAGS".to_string(), "-C target-feature=-crt-static".to_string())]
}

/// The whole environment for one build: cross-compilation plus, for a windowed app, what it
/// needs to be able to open a window at all.
///
/// `RUSTFLAGS` is merged rather than overwritten — an existing value from the operator's
/// environment would otherwise be silently dropped, and dropping `RUSTFLAGS` changes what
/// gets built without saying so.
pub fn build_env(triple: &str, toml: &str, inherited_rustflags: Option<&str>) -> Vec<(String, String)> {
    build_env_on_path(triple, toml, inherited_rustflags, "")
}

/// [`build_env`], probing `path_env` for the cross linker.
pub fn build_env_on_path(
    triple: &str,
    toml: &str,
    inherited_rustflags: Option<&str>,
    path_env: &str,
) -> Vec<(String, String)> {
    build_env_for(triple, is_gui_app(toml), inherited_rustflags, path_env)
}

/// [`build_env_on_path`] with the GUI answer already decided — by [`is_gui_app_dir`], when
/// the caller has a directory rather than one manifest.
/// [`build_env_for`], knowing the host triple as well as the `--target` one.
///
/// TWO DIFFERENT QUESTIONS NEED TWO DIFFERENT TRIPLES, and conflating them is the
/// bug this exists to fix.
///
/// * "Which cross LINKER?" wants the `--target` string. A native build passes
///   no `--target`, `cargo_target` is "", and `cross_env("")` correctly returns
///   nothing. Feed it the host triple instead and a native build on the phone
///   would be told to use `aarch64-linux-musl-gcc` as a CROSS linker.
/// * "Is this musl, so does a windowed app need -crt-static?" wants the
///   EFFECTIVE triple. For a native build that is the host. `gui_env` decides
///   on `triple.contains("musl")`, and "" contains nothing -- so on the phone,
///   the exact case the flag was written for, a native build never got it and
///   shipped a static binary that died with NoWaylandLib. The reported log
///   showed no environment lines at all, which is this.
pub fn build_env_for_native(
    cargo_target: &str,
    native: &str,
    gui: bool,
    inherited_rustflags: Option<&str>,
    path_env: &str,
) -> Vec<(String, String)> {
    let mut env = if path_env.is_empty() { cross_env(cargo_target) } else { cross_env_on_path(cargo_target, path_env) };
    let effective = if cargo_target.is_empty() { native } else { cargo_target };
    // `RUSTFLAGS` DOES NOT REACH BUILD SCRIPTS UNDER --target. Passing a target
    // puts cargo in cross mode, where RUSTFLAGS applies to the target only;
    // build scripts and proc-macros are built for the host without it. So a
    // cross build targeting static musl got a dynamic BINARY and a static
    // BUILD SCRIPT -- and whisper-rs-sys's build script is what dlopens
    // libclang, which is exactly where it failed.
    //
    // The per-triple variable is the one that covers both: cargo reads
    // CARGO_TARGET_<TRIPLE>_RUSTFLAGS for the target, and for a native build
    // (host == target) it applies to build scripts as well. On a genuine
    // cross build the host side is a different triple and is correctly left
    // alone -- a glibc host's build script does not need -crt-static.
    let triple_var = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        effective.to_ascii_uppercase().replace('-', "_")
    );
    if gui {
        for (k, v) in gui_env(effective) {
            let merged = match inherited_rustflags.map(str::trim).filter(|s| !s.is_empty()) {
                Some(existing) if k == "RUSTFLAGS" => format!("{existing} {v}"),
                _ => v,
            };
            // Both: RUSTFLAGS for tools that read only that, and the
            // per-triple variable so a static-musl HOST's build scripts get it
            // too. Same value, so they cannot disagree.
            if k == "RUSTFLAGS" && effective.contains("musl") {
                env.push((triple_var.clone(), merged.clone()));
            }
            env.push((k, merged));
        }
    }
    env
}

pub fn build_env_for(
    triple: &str,
    gui: bool,
    inherited_rustflags: Option<&str>,
    path_env: &str,
) -> Vec<(String, String)> {
    let mut env = if path_env.is_empty() { cross_env(triple) } else { cross_env_on_path(triple, path_env) };
    if gui {
        for (k, v) in gui_env(triple) {
            let merged = match inherited_rustflags.map(str::trim).filter(|s| !s.is_empty()) {
                Some(existing) if k == "RUSTFLAGS" => format!("{existing} {v}"),
                _ => v,
            };
            env.push((k, merged));
        }
    }
    env
}

/// Does this ELF have a dynamic loader (a `PT_INTERP` segment)?
///
/// `Some(false)` is the failure this whole path exists for: a static binary cannot dlopen,
/// so a windowed app built that way starts and cannot open a window on any machine. `None`
/// means "not an ELF64 little-endian file I can read" — an unknown format is not evidence
/// of anything, and guessing here would fail correct builds.
///
/// Only the program-header table is read: 64 bytes of header, then `e_phnum` entries.
pub fn elf_is_dynamic(bytes: &[u8]) -> Option<bool> {
    const PT_INTERP: u32 = 3;
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" {
        return None;
    }
    // class 2 = 64-bit, data 1 = little-endian. Anything else: not read.
    if bytes[4] != 2 || bytes[5] != 1 {
        return None;
    }
    let u16at = |o: usize| -> Option<usize> {
        Some(u16::from_le_bytes([*bytes.get(o)?, *bytes.get(o + 1)?]) as usize)
    };
    let phoff = u64::from_le_bytes(bytes.get(0x20..0x28)?.try_into().ok()?) as usize;
    let phentsize = u16at(0x36)?;
    let phnum = u16at(0x38)?;
    if phentsize < 4 {
        return None;
    }
    for i in 0..phnum {
        let at = phoff.checked_add(i.checked_mul(phentsize)?)?;
        let ty = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?);
        if ty == PT_INTERP {
            return Some(true);
        }
    }
    Some(false)
}

/// What to tell the operator when a pushed app dies at startup with a backend error.
///
/// The runtime message names a missing library, so the obvious reading is "install
/// libwayland on the device" — which does not help, because the library is there and the
/// binary cannot dlopen anything. Worth saying once, in the words the failure uses.
pub fn explain_no_backend(triple: &str) -> Option<&'static str> {
    triple.contains("musl").then_some(
        "the app started and could not open a window (NoWaylandLib, or \"Failed to \
         initialize any backend\"). This is a STATIC musl build: winit dlopens \
         libwayland-client and libX11 at run time, and a static binary cannot dlopen \
         anything, so both backends look absent even when they are installed. Rebuild \
         with RUSTFLAGS=-C target-feature=-crt-static, which gitmind now sets for \
         windowed apps on musl targets.",
    )
}

/// [`cross_env`], but choosing the linker by what is actually on `path_env` rather than by
/// what the distro usually calls it.
///
/// Nix's cross compilers are prefixed with the FULL triple
/// (`aarch64-unknown-linux-gnu-gcc`); Debian's are short (`aarch64-linux-gnu-gcc`). Naming
/// one and hoping is how a machine with a perfectly good cross toolchain gets told the
/// linker is missing. Falls back to [`cross_linker`]'s fixed name when nothing is found, so
/// the error still names a variable cargo understands.
pub fn cross_env_on_path(triple: &str, path_env: &str) -> Vec<(String, String)> {
    let found = crate::nixtoolchain::pick_linker(triple, |n| crate::nixtoolchain::on_path(n, path_env));
    let Some(linker) = found.or_else(|| cross_linker(triple).map(str::to_string)) else {
        return Vec::new();
    };
    vec![(linker_var(triple), linker)]
}

/// `CARGO_TARGET_<TRIPLE>_LINKER`, the variable cargo reads.
pub fn linker_var(triple: &str) -> String {
    format!("CARGO_TARGET_{}_LINKER", triple.to_uppercase().replace('-', "_"))
}

pub fn cross_env(triple: &str) -> Vec<(String, String)> {
    // No early return for the empty triple: `cross_linker("")` matches no arm
    // and returns None, so the unknown-triple path below already covers it. I
    // wrote the guard first and then could not break it — deleting it failed
    // no test, because it never did anything. `a_native_build_needs_no_cross_
    // environment` still pins the behaviour, now on the one path that provides
    // it rather than on two.
    let Some(linker) = cross_linker(triple) else {
        // An unknown triple gets no guess. A wrong linker fails later and more
        // confusingly than no linker, which at least names the variable cargo
        // wanted.
        return Vec::new();
    };
    vec![(linker_var(triple), linker.to_string())]
}

/// The linker binary for a target triple, if this fleet knows one.
///
/// musl and gnu need DIFFERENT linkers even for the same architecture —
/// `musl-gcc` is a wrapper that points gcc at musl's headers and libc, and
/// using the gnu cross-gcc for a musl target produces a binary that links
/// against glibc and dies on the device with a loader error. Both fleet
/// targets are aarch64, so architecture alone does not decide it.
pub fn cross_linker(triple: &str) -> Option<&'static str> {
    match triple {
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu-gcc"),
        // The Alpine/postmarketOS target. `musl-gcc` on a Debian host is the
        // HOST's musl wrapper and is x86_64 — the aarch64 musl target wants
        // `aarch64-linux-musl-gcc` from a musl cross toolchain, which is not
        // in Debian's archive. Named here anyway so the failure is "linker not
        // found: aarch64-linux-musl-gcc" rather than a link error nobody can
        // read.
        "aarch64-unknown-linux-musl" => Some("aarch64-linux-musl-gcc"),
        _ => None,
    }
}

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
/// argv for TESTING a crate, built from the same flags as its build.
///
/// Deliberately shares `features` and `no_default` with the build argv: a test
/// run under different features tests different code, and a green result there
/// says nothing about the binary that ships. That is the whole reason this is
/// derived from the app's SuiteApp rather than being a bare `cargo test`.
///
/// Two differences from the build, both intentional:
///
/// * `-p`, never `--bin`. `--bin` builds one target and runs NO tests — a lib's
///   unit tests live in the lib target, so a bin-scoped test run reports
///   success having executed nothing.
/// * no `--release`. Debug assertions are most of what a test suite is for, and
///   `--release` compiles them out.
pub fn cargo_test_argv(app: &str, triple: &str, features: &[String], no_default: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["cargo".into(), "test".into()];
    if !triple.trim().is_empty() {
        a.push("--target".into());
        a.push(triple.to_string());
    }
    a.push("-p".into());
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

/// How many cargo jobs to allow, given where the build runs.
///
/// # Desktop and phone want opposite answers
///
/// On a desktop, cargo's default — one job per core — is right, and capping it
/// wastes the machine. On a phone it is how a build gets killed: an FP5 has 8
/// cores and 6-8 GB, and eight concurrent rustc processes on a large crate ask
/// for more than that. The kernel's OOM killer takes the biggest one, cargo
/// reports a signal rather than a compile error, and the operator sees a build
/// that "just stops" — which reads as a toolchain problem rather than memory.
///
/// It is memory per job, not cores, that decides this. A phone has plenty of
/// cores and not enough RAM to feed them, and `unendlich` or `blackstar` link
/// steps are gigabytes each.
///
/// `None` means "say nothing and let cargo decide", which is the desktop
/// answer: a fixed number would be wrong on both a 4-core laptop and a 32-core
/// workstation.
///
/// Pure over the two facts it needs, so the policy is testable without a
/// device — which matters, because the device is exactly where it cannot be
/// experimented with cheaply.
pub fn job_limit(is_remote_device: bool, device_ram_mb: Option<u64>) -> Option<u32> {
    if !is_remote_device {
        return None;
    }
    // Roughly 1.5 GB per concurrent rustc on this fleet's larger crates,
    // measured the hard way: a full-fleet build in a 4 GB tmpfs lost rlibs
    // and produced `E0463 can't find crate` for one that had compiled
    // (C14_TODO §21). Memory pressure on a phone fails the same way and
    // reports even less.
    let ram = device_ram_mb.unwrap_or(4096);
    let by_ram = (ram / 1536).max(1) as u32;
    // Never more than 4 even on a well-provisioned device: past that the
    // limit stops being memory and starts being thermal, and a phone that
    // throttles takes longer than one that never did.
    Some(by_ram.min(4))
}

/// What this HOST can do at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostResources {
    pub ram_mb: u64,
    pub cores: u32,
}

/// How to spend a build machine: how many app builds at once, and how many cargo jobs each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildPlan {
    /// Builds that may run concurrently. Always at least 1.
    pub concurrent: u32,
    /// `-j` for each of them. `None` leaves cargo's default, which is right when only one
    /// build runs.
    pub jobs_each: Option<u32>,
}

/// Below this, one build at a time. See [`host_build_plan`].
pub const STACK_RAM_MB: u64 = 24 * 1024;
/// Memory to reserve per concurrent build, beyond the per-job estimate: a linking rustc on
/// this fleet's larger crates is gigabytes on its own.
pub const RAM_PER_BUILD_MB: u64 = 8 * 1024;

/// Whether to stack whole builds on this host, and how hard to drive each.
///
/// # Why stacking is worth it here, and only above a threshold
///
/// A fleet build is many small crates and a few enormous ones, and cargo's parallelism is
/// per-BUILD: while one app is linking, one rustc is running and the rest of the machine is
/// idle. Running several app builds side by side fills that, which is where most of the wall
/// time on a workstation goes.
///
/// It is memory that decides whether that is a good idea. Two concurrent builds of the big
/// crates want gigabytes each on top of their jobs, and a machine that starts swapping is
/// slower than one building serially — while an OOM kill reports as a signal, not a compile
/// error, and reads as a toolchain fault. So: nothing changes below [`STACK_RAM_MB`], which
/// is the "ample RAM" line, and above it the concurrency is bounded by BOTH memory and
/// cores.
///
/// The jobs are then divided among the concurrent builds. Leaving each at cargo's default —
/// one job per core — would put `cores × concurrent` rustc processes on the machine, which
/// is the thrashing this is supposed to avoid rather than cause.
///
/// Pure over the two facts it needs, so the policy can be argued about without a
/// workstation to try it on.
pub fn host_build_plan(res: HostResources) -> BuildPlan {
    let cores = res.cores.max(1);
    if res.ram_mb < STACK_RAM_MB {
        // One at a time, cargo's own job count. This is today's behaviour, unchanged.
        return BuildPlan { concurrent: 1, jobs_each: None };
    }
    let by_ram = (res.ram_mb / RAM_PER_BUILD_MB).max(1) as u32;
    // At least 2 cores per build, or the jobs division leaves each build serial and the
    // stacking buys nothing but memory pressure.
    let by_cores = (cores / 2).max(1);
    // Four is the ceiling: past it the contention is disk and the linker, which do not
    // divide, and a build machine that thrashes its page cache is slower than one that did
    // less at once.
    let concurrent = by_ram.min(by_cores).min(4);
    if concurrent <= 1 {
        return BuildPlan { concurrent: 1, jobs_each: None };
    }
    BuildPlan { concurrent, jobs_each: Some((cores / concurrent).max(1)) }
}

/// Tests are NEVER stacked, whatever the machine has.
///
/// A build is a pure function of its inputs; a test suite is not. These suites bind ports,
/// write into `/tmp` and `$HOME`, spawn a compositor under Xvfb, and read the same fixture
/// files. Two of them at once do not fail cleanly — they fail intermittently, in whichever
/// one lost the race, and the failure does not reproduce when it is run alone. That costs
/// more than the wall time it saves.
///
/// Returned as a plan rather than a bare `1` so a caller cannot accidentally reuse the build
/// plan here: the two questions look identical and have different answers.
pub fn host_test_plan(_res: HostResources) -> BuildPlan {
    BuildPlan { concurrent: 1, jobs_each: None }
}

/// This machine's RAM and cores.
///
/// `/proc/meminfo`'s `MemTotal`, which is what the machine HAS rather than what is free
/// right now — a build that starts when memory happens to be free still has to finish, and
/// planning against a momentary figure produces a different answer every run.
pub fn detect_host() -> HostResources {
    let ram_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0);
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    HostResources { ram_mb, cores }
}

/// Append `-j` to a build argv when the target warrants it.
///
/// Separate from `cargo_build_argv_kind` rather than folded in, because the
/// argv builder is used for local builds, remote builds and dry-run display,
/// and only one of those wants a cap. A caller that forgets is left with
/// cargo's default, which is the safe direction.
pub fn with_job_limit(mut argv: Vec<String>, limit: Option<u32>) -> Vec<String> {
    if let Some(n) = limit {
        argv.push("-j".into());
        argv.push(n.to_string());
    }
    argv
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
    /// Build inside a container with a NATIVE toolchain for the target's libc
    /// (musl-in-musl, not cross-gcc) -- see `podman`. The fleet's actual answer
    /// for aarch64-musl on a glibc/gnu desktop: Debian's archive has no
    /// `aarch64-linux-musl-gcc` (see `cross_linker`'s doc), so `CrossCompile`
    /// is UNREACHABLE for that pairing on a typical dev host, and this is what
    /// stands in for it -- no host cross-linker needed, no phone reachability
    /// needed, and (unlike `RemoteNative`) not limited to what a phone's CPU
    /// and RAM can compile in a reasonable time.
    PodmanCross,
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
/// cross-linker for the target is available, whether a podman-buildable NATIVE
/// image exists for it (see `podman::image_for`) and podman itself is on this
/// machine, and whether the device is reachable over ssh.
///
/// FALLBACK ORDER, cheapest and least fragile first: same arch+libc needs no
/// toolchain at all; a real cross-linker builds fastest when one exists (rare
/// for musl -- see `PodmanCross`'s doc); podman needs neither a host
/// cross-linker nor the phone being reachable RIGHT NOW; building on the
/// device itself is the fallback of last resort (slow, and asks a phone's CPU
/// to compile a GUI toolkit); with none of these, the blocker names every
/// option, since which one to unblock is the person's call. Pure -- unit-tested.
pub fn recommend_strategy(
    host_native: &str,
    target: &str,
    have_cross_linker: bool,
    have_podman: bool,
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
    let podman_ready = have_podman && crate::podman::image_for(target).is_some();
    if have_cross_linker {
        StrategyChoice {
            strategy: DeployStrategy::CrossCompile,
            reason: format!("differs from host ({mismatch}); cross-linker present → cross-compile + copy"),
            needs_cross_linker: true,
        }
    } else if podman_ready {
        StrategyChoice {
            strategy: DeployStrategy::PodmanCross,
            reason: format!("differs from host ({mismatch}), no cross-linker, podman has a native {} image → build in a container", target_libc(target)),
            needs_cross_linker: false,
        }
    } else if remote_reachable {
        StrategyChoice {
            strategy: DeployStrategy::RemoteNative,
            reason: format!("differs from host ({mismatch}), no cross-linker or podman image → build on the device (native there)"),
            needs_cross_linker: false,
        }
    } else {
        // can't cross, can't reach, no podman image — surface the blocker and name every way out.
        StrategyChoice {
            strategy: DeployStrategy::CrossCompile,
            reason: format!(
                "differs from host ({mismatch}), device unreachable, no cross-linker or podman image — install a {} linker, or podman + a native image, to unblock",
                target_libc(target)
            ),
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
    fn cosmic_tideshell_adds_both_features_without_duplicating() {
        use super::{with_cosmic_tideshell, is_cosmic_tideshell};
        assert_eq!(with_cosmic_tideshell(vec![]), vec!["wayland", "layer", "ei"]);
        // Already-present features are kept, in their original order, and not doubled.
        assert_eq!(with_cosmic_tideshell(vec!["voice".into(), "wayland".into()]),
                   vec!["voice", "wayland", "layer", "ei"]);
        assert_eq!(with_cosmic_tideshell(vec!["wayland".into(), "layer".into(), "ei".into()]),
                   vec!["wayland", "layer", "ei"], "already the set: unchanged");
    }

    /// The set must switch on every feature the `tideshell` binary REQUIRES, or cargo builds the library, skips
    /// the binary and reports success. Read from the real manifest so it cannot drift.
    #[test]
    fn the_set_satisfies_tideshells_own_required_features() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tideshell/egui-frontend/Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("{manifest:?}: {e}"));
        let at = text.find("name = \"tideshell\"\n").expect("the tideshell [[bin]]");
        let block = &text[at..text[at..].find("\n[").map(|n| at + n).unwrap_or(text.len())];
        let line = block.lines().find(|l| l.trim_start().starts_with("required-features")).expect("tideshell declares required-features");
        let required: Vec<String> = line.split('[').nth(1).unwrap().split(']').next().unwrap().split(',').map(|t| t.trim().trim_matches('"').to_string()).filter(|t| !t.is_empty()).collect();
        assert!(required.len() >= 2, "the scan found them: {required:?}");
        let set: Vec<String> = super::COSMIC_TIDESHELL_FEATURES.iter().map(|s| s.to_string()).collect();
        for r in &required {
            assert!(set.contains(r), "the preset must enable {r}, which the tideshell binary requires: {set:?}");
        }
    }

    #[test]
    fn is_cosmic_tideshell_ignores_order_and_extras() {
        use super::is_cosmic_tideshell;
        assert!(is_cosmic_tideshell(&["ei".into(), "layer".into(), "wayland".into()]), "order-independent");
        assert!(is_cosmic_tideshell(&["wayland".into(), "voice".into(), "layer".into(), "ei".into()]), "extras don't disqualify it");
        assert!(!is_cosmic_tideshell(&["layer".into(), "wayland".into()]), "missing ei: the binary would be skipped");
        assert!(!is_cosmic_tideshell(&["layer".into()]), "missing wayland");
        assert!(!is_cosmic_tideshell(&["wayland".into()]), "missing layer");
        assert!(!is_cosmic_tideshell(&[]));
    }

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

    /// THE TWO APPS THAT REPORTED A WAYLAND ERROR. `is_gui_app` reads one manifest, and for
    /// both of these it is the wrong one — so they missed the `-crt-static` flag, shipped a
    /// static musl binary that cannot dlopen libwayland-client, and the failure surfaced as
    /// a winit Wayland error that reads like a missing system library.
    ///
    /// This carried a DUPLICATE `#[test]` attribute (one above the doc comment and one
    /// below), so it was registered twice and libtest ran two copies concurrently. Both
    /// derived the same pid-keyed temp root and both began with `remove_dir_all`, so one
    /// copy deleted the other's fixtures mid-run — which is why the voiceterm assertion
    /// passed and the snapclone one, four lines later, reported "frontend was not found"
    /// for a directory that had been created and then wiped. `is_gui_app_dir` was correct
    /// throughout.
    #[test]
    fn a_frontend_in_a_subdirectory_is_still_a_gui_app() {
        // Unique per CALL, not per process: a pid is shared by every test in
        // the binary, so it is the wrong key for a directory one test wipes.
        let root = std::env::temp_dir().join(format!(
            "gm-gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);

        // voiceterm's shape: the top-level manifest is a DIFFERENT package, eframe is in a
        // subdirectory.
        let vt = root.join("voiceterm");
        std::fs::create_dir_all(vt.join("egui-frontend")).unwrap();
        std::fs::write(vt.join("Cargo.toml"), "[package]\nname = \"web2qml\"\n\n[dependencies]\nserde = \"1\"\n").unwrap();
        assert!(!is_gui_app(&std::fs::read_to_string(vt.join("Cargo.toml")).unwrap()), "the premise");
        std::fs::write(vt.join("egui-frontend/Cargo.toml"), "[dependencies]\neframe = \"0.34\"\n").unwrap();
        assert!(is_gui_app_dir(&vt), "voiceterm's frontend was not found");

        // snapclone's shape: NO top-level manifest at all (a CMake project).
        let sc = root.join("snapclone");
        std::fs::create_dir_all(sc.join("egui-frontend")).unwrap();
        std::fs::write(sc.join("CMakeLists.txt"), "project(snapclone)\n").unwrap();
        std::fs::write(sc.join("egui-frontend/Cargo.toml"), "[dependencies]\neframe = \"0.34\"\n").unwrap();
        assert!(is_gui_app_dir(&sc), "snapclone's frontend was not found");

        // A frontend under `crates/` too — gitmind's own shape.
        let gm = root.join("gitmind");
        std::fs::create_dir_all(gm.join("crates/gui-egui")).unwrap();
        std::fs::write(gm.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(gm.join("crates/gui-egui/Cargo.toml"), "[dependencies]\neframe = \"0.34\"\n").unwrap();
        assert!(is_gui_app_dir(&gm), "a crates/ frontend was not found");

        // And a CLI app is still a CLI app: this must not turn every push into a dynamic
        // musl build, which is a different binary with a different runtime requirement.
        let cli = root.join("cli");
        std::fs::create_dir_all(cli.join("src")).unwrap();
        std::fs::write(cli.join("Cargo.toml"), "[dependencies]\nclap = \"4\"\n").unwrap();
        assert!(!is_gui_app_dir(&cli));
        // A directory that does not exist is not a GUI app either.
        assert!(!is_gui_app_dir(&root.join("nope")));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// EVERY app in the fleet, not the handful I thought were affected.
    ///
    /// Thirteen of them keep their frontend outside the root manifest — `egui-frontend/`,
    /// `crates/*-egui/`, a differently-named root package — so the single-manifest check
    /// missed all thirteen, not the two that happened to be reported. This walks each app
    /// tree for ANY manifest declaring a windowing dependency and requires the bounded
    /// search to agree, which is what turns "the two I heard about" into "the ones that
    /// exist".
    #[test]
    fn no_app_with_a_frontend_is_mistaken_for_a_cli() {
        let apps = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .expect("the apps directory");

        // A full walk, for the TEST only — the shipping check stays bounded, and this is
        // what proves the bound is wide enough.
        fn any_gui_manifest(dir: &std::path::Path, depth: usize) -> bool {
            if depth > 4 {
                return false;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { return false };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if e.file_name() == "target" || e.file_name() == ".git" {
                        continue;
                    }
                    if any_gui_manifest(&p, depth + 1) {
                        return true;
                    }
                } else if e.file_name() == "Cargo.toml"
                    && super::is_gui_app(&std::fs::read_to_string(&p).unwrap_or_default())
                {
                    return true;
                }
            }
            false
        }

        let mut checked = 0;
        let mut missed: Vec<String> = Vec::new();
        for e in std::fs::read_dir(&apps).expect("apps/").flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            if !any_gui_manifest(&dir, 0) {
                continue; // genuinely a CLI app or a non-Rust project
            }
            checked += 1;
            if !is_gui_app_dir(&dir) {
                missed.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        assert!(checked >= 10, "only {checked} app trees found under {apps:?} — the path is wrong");
        assert!(
            missed.is_empty(),
            "these apps have a frontend the bounded search does not find, so they would ship a \
             static musl binary that cannot dlopen libwayland-client: {missed:?}"
        );
    }

    /// The real trees, so this cannot regress when an app moves its frontend.
    #[test]
    fn the_shipped_frontends_are_detected_where_they_actually_live() {
        let apps = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..") // crates/cockpit -> gitmind -> apps
            .canonicalize()
            .expect("the apps directory");
        let mut checked = 0;
        for name in ["voiceterm", "snapclone", "rusty-board", "logscope"] {
            let dir = apps.join(name);
            if !dir.is_dir() {
                continue; // a checkout without every app is not a failure
            }
            checked += 1;
            assert!(is_gui_app_dir(&dir), "{name} has an egui frontend and was not detected");
        }
        // A test that silently checked nothing is the failure this whole commit is about.
        assert!(checked >= 2, "found only {checked} app trees under {apps:?} — the path is wrong");
    }

    /// A windowed app on a static musl target compiles and cannot open a window on any
    /// machine. This is the flag that makes the build dynamic again.
    #[test]
    fn a_windowed_app_on_musl_is_built_dynamic() {
        let gui = "[dependencies]\neframe = \"0.34\"\nserde = \"1\"\n";
        let cli = "[dependencies]\nserde = \"1\"\nclap = \"4\"\n";
        assert!(is_gui_app(gui) && !is_gui_app(cli));
        assert!(is_gui_app("[dependencies]\nwinit=\"0.30\"\n"), "no space before the =");
        assert!(!is_gui_app("[dependencies]\n# eframe = \"0.34\"\n"), "a commented-out dep is not a dep");

        let musl = build_env("aarch64-unknown-linux-musl", gui, None);
        assert!(musl.iter().any(|(k, v)| k == "RUSTFLAGS" && v.contains("-crt-static")));
        // and the cross linker is still there
        assert!(musl.iter().any(|(k, _)| k.contains("LINKER")));

        // gnu is already dynamic; a CLI does not open a window at all
        assert!(!build_env("aarch64-unknown-linux-gnu", gui, None).iter().any(|(k, _)| k == "RUSTFLAGS"));
        assert!(!build_env("aarch64-unknown-linux-musl", cli, None).iter().any(|(k, _)| k == "RUSTFLAGS"));
    }

    /// Overwriting the operator's RUSTFLAGS would change what gets built without saying so.
    /// The linker is chosen by what exists, not by what the distro usually calls it — a Nix
    /// cross compiler is prefixed with the full triple and would otherwise look missing.
    #[test]
    fn the_linker_is_found_by_probing_the_path() {
        let dir = std::env::temp_dir().join(format!("gm-linker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let nixish = dir.join("aarch64-unknown-linux-gnu-gcc");
        std::fs::write(&nixish, b"#!/bin/sh\n").unwrap();
        let path = dir.display().to_string();
        let env = cross_env_on_path("aarch64-unknown-linux-gnu", &path);
        assert_eq!(env[0].0, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER");
        assert_eq!(env[0].1, "aarch64-unknown-linux-gnu-gcc");
        // nothing on PATH: the fixed name, so the failure still names a real variable
        let env = cross_env_on_path("aarch64-unknown-linux-gnu", "/nonexistent");
        assert_eq!(env[0].1, "aarch64-linux-gnu-gcc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inherited_rustflags_are_kept() {
        let gui = "[dependencies]\neframe = \"0.34\"\n";
        let env = build_env("aarch64-unknown-linux-musl", gui, Some("-C debuginfo=1"));
        let flags = env.iter().find(|(k, _)| k == "RUSTFLAGS").map(|(_, v)| v.clone()).unwrap();
        assert!(flags.contains("-C debuginfo=1") && flags.contains("-crt-static"), "{flags}");
        // an empty inherited value is not prepended as whitespace
        let env = build_env("aarch64-unknown-linux-musl", gui, Some("   "));
        let flags = env.iter().find(|(k, _)| k == "RUSTFLAGS").map(|(_, v)| v.clone()).unwrap();
        assert_eq!(flags, "-C target-feature=-crt-static");
    }

    /// The check that turns a runtime mystery into a build-time failure.
    #[test]
    fn a_static_elf_is_told_apart_from_a_dynamic_one() {
        // A minimal ELF64-LE header with one program header.
        let mut e = vec![0u8; 64 + 56];
        e[..4].copy_from_slice(b"\x7fELF");
        e[4] = 2; // 64-bit
        e[5] = 1; // little-endian
        e[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        e[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        e[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        // PT_LOAD only → static
        e[64..68].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(elf_is_dynamic(&e), Some(false));
        // PT_INTERP → has a loader
        e[64..68].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(elf_is_dynamic(&e), Some(true));
        // not an ELF, truncated, or a class this does not read: no opinion, not a failure
        assert_eq!(elf_is_dynamic(b"#!/bin/sh\n"), None);
        assert_eq!(elf_is_dynamic(&e[..40]), None);
        let mut e32 = e.clone();
        e32[4] = 1;
        assert_eq!(elf_is_dynamic(&e32), None);
        // a program-header count that runs off the end is not a crash. (The first entry is
        // put back to PT_LOAD first: with PT_INTERP still there the answer is Some(true)
        // before the walk ever reaches the bad offset, which is correct and tests nothing.)
        let mut over = e.clone();
        over[64..68].copy_from_slice(&1u32.to_le_bytes());
        over[0x38..0x3a].copy_from_slice(&999u16.to_le_bytes());
        assert_eq!(elf_is_dynamic(&over), None);
    }

    #[test]
    fn the_backend_failure_is_explained_only_where_it_applies() {
        let m = explain_no_backend("aarch64-unknown-linux-musl").unwrap();
        assert!(m.contains("dlopen") && m.contains("crt-static"));
        assert!(explain_no_backend("aarch64-unknown-linux-gnu").is_none());
    }

    /// "Ample RAM" is the whole condition. Below the line nothing changes — one build at a
    /// time, cargo's own job count — because a machine that starts swapping is slower than
    /// one building serially, and an OOM kill reports as a signal rather than a compile
    /// error.
    #[test]
    fn builds_stack_only_on_a_machine_with_room_for_them() {
        // A laptop: 16 GB, 8 cores. Unchanged.
        let laptop = host_build_plan(HostResources { ram_mb: 16 * 1024, cores: 8 });
        assert_eq!(laptop, BuildPlan { concurrent: 1, jobs_each: None });

        // A phone-sized host, and a machine with plenty of RAM but two cores: also
        // unchanged, because two builds of one core each is not stacking.
        assert_eq!(host_build_plan(HostResources { ram_mb: 6 * 1024, cores: 8 }).concurrent, 1);
        assert_eq!(host_build_plan(HostResources { ram_mb: 64 * 1024, cores: 2 }).concurrent, 1);

        // A workstation: 32 GB, 16 cores. Stacks, and DIVIDES the jobs — leaving each build
        // at cargo's default would put cores × concurrent rustc processes on the machine,
        // which is the thrashing this exists to avoid.
        let ws = host_build_plan(HostResources { ram_mb: 32 * 1024, cores: 16 });
        assert!(ws.concurrent > 1, "{ws:?}");
        let jobs = ws.jobs_each.expect("a stacked build must cap its jobs");
        assert!(jobs * ws.concurrent <= 16, "{} jobs × {} builds oversubscribes 16 cores", jobs, ws.concurrent);

        // Memory bounds it, not just cores: 24 GB and 64 cores is three builds, not thirty.
        let lopsided = host_build_plan(HostResources { ram_mb: 24 * 1024, cores: 64 });
        assert_eq!(lopsided.concurrent, 3, "{lopsided:?}");

        // And there is a ceiling however big the machine is: past it the contention is disk
        // and the linker, which do not divide.
        let huge = host_build_plan(HostResources { ram_mb: 512 * 1024, cores: 128 });
        assert_eq!(huge.concurrent, 4, "{huge:?}");

        // Nonsense inputs do not produce a zero: `-j 0` is an error and no builds at all is
        // a hang.
        assert!(host_build_plan(HostResources { ram_mb: 0, cores: 0 }).concurrent >= 1);
        assert!(host_build_plan(HostResources { ram_mb: 64 * 1024, cores: 0 }).concurrent >= 1);
    }

    /// TESTS ARE NEVER STACKED. A build is a pure function of its inputs; a suite binds
    /// ports, writes into /tmp and $HOME, and spawns a compositor under Xvfb. Two at once do
    /// not fail cleanly — they fail intermittently, in whichever lost the race, and do not
    /// reproduce alone.
    #[test]
    fn tests_never_stack_however_big_the_machine() {
        for res in [
            HostResources { ram_mb: 8 * 1024, cores: 4 },
            HostResources { ram_mb: 64 * 1024, cores: 32 },
            HostResources { ram_mb: 512 * 1024, cores: 128 },
        ] {
            let t = host_test_plan(res);
            assert_eq!(t.concurrent, 1, "tests stacked on {res:?}");
            assert_eq!(t.jobs_each, None);
            // The two questions look identical and have different answers, which is why
            // they are separate functions rather than a flag.
            if host_build_plan(res).concurrent > 1 {
                assert_ne!(host_build_plan(res), t);
            }
        }
    }

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
        assert_eq!(recommend_strategy(desktop, desktop, false, false, true).strategy, DeployStrategy::LocalNative);
        // desktop -> phone with a musl linker: cross-compile + copy
        let cc = recommend_strategy(desktop, phone, true, false, true);
        assert_eq!(cc.strategy, DeployStrategy::CrossCompile);
        assert!(cc.needs_cross_linker);
        // desktop -> phone, no linker but reachable: build on the phone (native there)
        assert_eq!(recommend_strategy(desktop, phone, false, false, true).strategy, DeployStrategy::RemoteNative);
        // desktop -> phone, no linker AND unreachable: surface the blocker (cross is the only path)
        let blocked = recommend_strategy(desktop, phone, false, false, false);
        assert_eq!(blocked.strategy, DeployStrategy::CrossCompile);
        assert!(blocked.reason.contains("musl"));
        // cross-arch (x86 desktop -> aarch64 phone) with a linker: cross-compile, reason names the arch gap
        let xarch = recommend_strategy("x86_64-unknown-linux-gnu", phone, true, false, true);
        assert_eq!(xarch.strategy, DeployStrategy::CrossCompile);
        assert!(xarch.reason.contains("x86_64") && xarch.reason.contains("aarch64"));
    }

    #[test]
    fn podman_is_preferred_over_building_on_the_phone() {
        let desktop = "aarch64-unknown-linux-gnu";
        let phone = "aarch64-unknown-linux-musl";
        // No cross-linker, but podman + a native image exist: use the container,
        // not RemoteNative -- faster and does not need the phone reachable now.
        let c = recommend_strategy(desktop, phone, false, true, true);
        assert_eq!(c.strategy, DeployStrategy::PodmanCross);
        assert!(!c.needs_cross_linker);
        assert!(c.reason.contains("podman"));
    }

    #[test]
    fn a_real_cross_linker_still_wins_over_podman() {
        let desktop = "aarch64-unknown-linux-gnu";
        let phone = "aarch64-unknown-linux-musl";
        let c = recommend_strategy(desktop, phone, true, true, true);
        assert_eq!(c.strategy, DeployStrategy::CrossCompile, "a real linker is faster than a container");
    }

    #[test]
    fn podman_present_but_no_image_for_this_target_falls_back_to_remote() {
        let desktop = "aarch64-unknown-linux-gnu";
        // gnu-on-gnu already HAS a real cross-linker path in this fleet
        // (buildflags::cross_linker), so podman offering no image for it must
        // not block the remote fallback.
        let c = recommend_strategy("x86_64-unknown-linux-gnu", desktop, false, true, true);
        assert_eq!(c.strategy, DeployStrategy::RemoteNative);
    }

    #[test]
    fn no_podman_falls_back_to_remote_exactly_as_before() {
        let desktop = "aarch64-unknown-linux-gnu";
        let phone = "aarch64-unknown-linux-musl";
        assert_eq!(recommend_strategy(desktop, phone, false, false, true).strategy, DeployStrategy::RemoteNative);
    }

}

#[cfg(test)]
mod test_argv_tests {
    use super::*;

    #[test]
    fn tests_run_against_the_same_features_the_build_uses() {
        // A test run under different features tests DIFFERENT CODE, and a green
        // result there says nothing about the binary that ships. bsettings is
        // the case that makes this concrete: its GUI only exists behind
        // `--features gui`.
        let feats = vec!["gui".to_string()];
        let t = cargo_test_argv("bigsilicon-settings", "", &feats, false);
        assert!(t.windows(2).any(|w| w == ["--features", "gui"]), "{t:?}");
    }

    #[test]
    fn tests_use_p_and_never_bin() {
        // `--bin` builds one target and runs NO tests — a lib's unit tests live
        // in the lib target, so a bin-scoped run reports success having
        // executed nothing. That is the worst possible green.
        let t = cargo_test_argv("voiceterm", "", &[], false);
        assert!(t.contains(&"-p".to_string()));
        assert!(!t.contains(&"--bin".to_string()));
    }

    #[test]
    fn tests_are_not_built_in_release() {
        // Debug assertions are most of what a suite is for, and --release
        // compiles them out.
        let t = cargo_test_argv("voiceterm", "", &[], false);
        assert!(!t.contains(&"--release".to_string()));
    }

    #[test]
    fn no_default_features_carries_across() {
        // rusty-board and blackstar both need it — their audio features want
        // system libraries a build box may not have.
        let t = cargo_test_argv("skydribble", "", &[], true);
        assert!(t.contains(&"--no-default-features".to_string()));
    }

    #[test]
    fn a_cross_target_is_passed_through() {
        let t = cargo_test_argv("voiceterm", "aarch64-unknown-linux-gnu", &[], false);
        assert!(t.windows(2).any(|w| w == ["--target", "aarch64-unknown-linux-gnu"]));
    }
}

#[cfg(test)]
mod artifact_name_tests {
    use super::*;

    /// The real manifest, not a fixture. The bug was a convention assumption —
    /// "a crate with several bins lists the primary one first" — that one crate
    /// in this tree breaks, and a fixture written by the same hand would have
    /// listed them in the order the assumption expects.
    #[test]
    fn the_gui_bin_wins_even_though_the_cli_is_declared_first() {
        let root = crate::ops::repo_root();
        let toml = std::fs::read_to_string(root.join("apps/settings-app/Cargo.toml"))
            .expect("apps/settings-app/Cargo.toml");
        // Guard the premise on the [[bin]] DECLARATIONS, not on any mention of
        // the names: settings-app's own description names both binaries, so a
        // plain `find` matches prose and the guard would pass whatever the
        // order became.
        let cli_at = toml.find("name = \"bsettings-cli\"").expect("bsettings-cli bin");
        let gui_at = toml.find("name = \"bsettings\"").expect("bsettings bin");
        assert!(cli_at < gui_at, "the CLI is no longer declared first — this test is moot");

        assert_eq!(artifact_name("bsettings", &toml), "bsettings",
                   "gitmind pins the command-line tool and the run button gets a binary \
                    that prints usage and exits");
        assert_eq!(artifact_name("bsettings-cli", &toml), "bsettings-cli",
                   "the CLI must still be reachable when it is what was asked for");
    }

    #[test]
    fn position_still_decides_when_no_bin_is_named_after_the_app() {
        // The case the original first-bin rule existed for. Removing the
        // fallback would break it, so both halves are pinned.
        let odd = "[package]\nname = \"pkg\"\n\n[[bin]]\nname = \"other\"\n";
        assert_eq!(artifact_name("app-id", odd), "other", "position is still the fallback");
    }

    const STUDIO: &str = "[package]\nname = \"studio-app\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"studio\"\npath = \"src/main.rs\"\n";

    #[test]
    fn an_explicit_bin_name_wins_over_the_package_name() {
        // The reported failure: cargo built studio-app and produced
        // target/release/studio, and the lookup went to .../studio-app.
        assert_eq!(build_id("studio", STUDIO), "studio-app", "-p takes the package");
        assert_eq!(artifact_name("studio", STUDIO), "studio", "the file is the bin");
    }

    #[test]
    fn without_a_bin_section_the_package_name_is_the_artifact() {
        let plain = "[package]\nname = \"krapper\"\n";
        assert_eq!(artifact_name("krapper", plain), "krapper");
    }

    #[test]
    fn a_name_key_outside_a_bin_section_is_not_a_bin_name() {
        // [package] also has `name`, and reading the first `name =` in the file
        // would return the PACKAGE every time — silently making this function
        // identical to build_id.
        let t = "[package]\nname = \"pkg\"\n\n[dependencies]\n\n[[bin]]\nname = \"realbin\"\n";
        assert_eq!(artifact_name("pkg", t), "realbin");
    }

    #[test]
    fn the_first_bin_wins_when_there_are_several() {
        // bsettings ships bsettings-cli and bsettings; the primary is listed
        // first by convention.
        let t = "[package]\nname = \"p\"\n\n[[bin]]\nname = \"first\"\n\n[[bin]]\nname = \"second\"\n";
        assert_eq!(artifact_name("p", t), "first");
    }

    #[test]
    fn an_empty_manifest_falls_back_to_the_app_name() {
        assert_eq!(artifact_name("unendlich", ""), "unendlich");
    }
    #[test]
    fn a_native_build_needs_no_cross_environment() {
        // The host `cc` IS the right linker. Setting one would override a
        // perfectly good default with a guess.
        assert!(cross_env("").is_empty());
        assert_eq!(cross_linker(""), None);
    }

    #[test]
    fn the_variable_name_is_the_one_cargo_reads() {
        // Uppercase with dashes as underscores. Get this wrong and cargo
        // silently ignores it, then fails at link with an error that does not
        // mention cross-compilation.
        let e = cross_env("aarch64-unknown-linux-gnu");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER");
        assert_eq!(e[0].1, "aarch64-linux-gnu-gcc");
    }

    #[test]
    fn musl_and_gnu_get_different_linkers() {
        // Same architecture, different libc. Using the gnu cross-gcc for a musl
        // target produces a binary linked against glibc that dies on the device
        // with a loader error — and both fleet targets are aarch64, so
        // architecture alone cannot decide it.
        let gnu = cross_linker("aarch64-unknown-linux-gnu").unwrap();
        let musl = cross_linker("aarch64-unknown-linux-musl").unwrap();
        assert_ne!(gnu, musl);
        assert!(musl.contains("musl"), "{musl}");
        assert!(!gnu.contains("musl"), "{gnu}");
    }

    #[test]
    fn every_fleet_target_has_a_linker() {
        // `BUILD_TARGETS` is what the UI offers. A target that can be selected and
        // cannot be linked is a build that fails after several minutes.
        for t in BUILD_TARGETS {
            assert!(cross_linker(t).is_some(), "{t} is offered with no linker");
            assert_eq!(cross_env(t).len(), 1, "{t} produced no environment");
        }
    }

    #[test]
    fn an_unknown_triple_gets_no_guess() {
        // A wrong linker fails later and more confusingly than no linker, which
        // at least names the variable cargo wanted.
        assert_eq!(cross_linker("riscv64gc-unknown-linux-gnu"), None);
        assert!(cross_env("riscv64gc-unknown-linux-gnu").is_empty());
    }

    #[test]
    fn a_desktop_build_is_not_capped() {
        // Cargo's default is one job per core and that is right here. A fixed
        // number would be wrong on both a 4-core laptop and a 32-core
        // workstation, which is why the answer is "say nothing".
        assert_eq!(job_limit(false, None), None);
        assert_eq!(job_limit(false, Some(65536)), None);
        let argv = with_job_limit(vec!["cargo".into(), "build".into()], job_limit(false, None));
        assert!(!argv.contains(&"-j".to_string()), "{argv:?}");
    }

    #[test]
    fn a_phone_build_is_capped_by_memory_not_cores() {
        // An FP5 has 8 cores and 6-8 GB. Eight concurrent rustc processes on a
        // large crate ask for more than that; the OOM killer takes one, cargo
        // reports a signal rather than a compile error, and the build "just
        // stops".
        assert_eq!(job_limit(true, Some(6144)), Some(4));
        assert_eq!(job_limit(true, Some(3072)), Some(2));
        assert_eq!(job_limit(true, Some(1536)), Some(1));
        // Never zero, whatever it is told — `-j 0` is an error, and a device
        // reporting a small or absent figure must still build.
        assert_eq!(job_limit(true, Some(0)), Some(1));
        assert_eq!(job_limit(true, Some(100)), Some(1));
    }

    #[test]
    fn a_large_phone_is_still_capped() {
        // Past four the limit stops being memory and starts being thermal: a
        // phone that throttles takes longer than one that never did.
        assert_eq!(job_limit(true, Some(16384)), Some(4));
        assert_eq!(job_limit(true, Some(65536)), Some(4));
    }

    #[test]
    fn an_unknown_device_ram_is_assumed_small() {
        // Guessing high on a device that cannot say is how the build dies.
        // Guessing low costs minutes.
        assert_eq!(job_limit(true, None), Some(2));
    }

    #[test]
    fn the_flag_lands_as_two_arguments_cargo_understands() {
        let argv = with_job_limit(
            cargo_build_argv("aperture", "", &[], false),
            job_limit(true, Some(6144)),
        );
        let i = argv.iter().position(|a| a == "-j").expect("no -j");
        assert_eq!(argv[i + 1], "4");
        // And it goes at the END, after `-p <app>` — cargo accepts it there
        // and a caller reading the argv sees the app name where it expects to.
        assert!(i > argv.iter().position(|a| a == "-p").unwrap());
    }


    /// UNENDLICH'S DEFAULTS, against the real manifest.
    ///
    /// The engine shipped `default = ["gpu"]`: no render module, no window, no
    /// audio. Correct for a headless CI build and wrong for anyone launching
    /// it to play, who then had to know that `render` does not imply `window`
    /// and that `audio` exists at all.
    ///
    /// `ai-engine` is deliberately NOT a default -- it pulls aperture-engine
    /// and a model -- but it must stay VISIBLE, because a feature that is
    /// neither on nor offered is one nobody finds. `all` is every feature
    /// except `default`, so it is listed and merely unticked.
    #[test]
    fn unendlich_defaults_to_a_playable_window_with_ai_offered_but_off() {
        let toml = include_str!(
            "../../../../../games/unendlich/crates/unendlich-engine/Cargo.toml");
        for triple in BUILD_TARGETS {
            let d = arch_default_features(toml, triple);
            for want in ["gpu", "render", "window", "audio"] {
                assert!(d.iter().any(|f| f == want), "{triple} is missing {want}: {d:?}");
            }
            assert!(!d.iter().any(|f| f == "ai-engine"),
                    "{triple} enables the model by default: {d:?}");
        }
        let f = scan_cargo_features(toml);
        assert!(f.all.iter().any(|x| x == "ai-engine"),
                "ai-engine must stay offered as a toggle, not hidden");
    }

#[cfg(test)]
mod native_musl_tests {
    use super::*;

    fn has(env: &[(String, String)], k: &str) -> Option<String> {
        env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone())
    }

    /// THE REPORTED BUG. A native build on the phone passes no --target, so
    /// `cargo_target` is "", and the flag decision read `"".contains("musl")`.
    /// The build log showed no environment at all and the binary was static.
    #[test]
    fn a_native_musl_gui_build_gets_the_dynamic_flag() {
        let env = build_env_for_native("", "aarch64-unknown-linux-musl", true, None, "");
        let rf = has(&env, "RUSTFLAGS").expect("a native musl GUI build must set RUSTFLAGS");
        assert!(rf.contains("-crt-static"), "{rf}");
    }

    /// ...and the BUILD SCRIPT gets it too, via the per-triple variable --
    /// whisper-rs-sys's build script is what dlopens libclang.
    #[test]
    fn a_native_musl_build_covers_build_scripts() {
        let env = build_env_for_native("", "aarch64-unknown-linux-musl", true, None, "");
        let v = has(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS")
            .expect("the per-triple variable must be set for the host's build scripts");
        assert!(v.contains("-crt-static"), "{v}");
    }

    /// THE TRAP IN THE OBVIOUS FIX. Passing the host triple to `cross_env`
    /// would tell a native build to use a CROSS linker. The linker decision
    /// must still see "" for native.
    #[test]
    fn a_native_build_is_not_given_a_cross_linker() {
        let env = build_env_for_native("", "aarch64-unknown-linux-musl", true, None, "");
        assert!(!env.iter().any(|(k, _)| k.ends_with("_LINKER")),
                "native build got a linker variable: {env:?}");
    }

    /// A glibc host needs none of this.
    #[test]
    fn a_native_glibc_build_is_left_alone() {
        let env = build_env_for_native("", "aarch64-unknown-linux-gnu", true, None, "");
        assert!(has(&env, "RUSTFLAGS").is_none(), "{env:?}");
    }

    /// A headless app never needed a window, so it keeps whatever linking it
    /// had -- the flag is about dlopening display libraries.
    #[test]
    fn a_non_gui_musl_build_is_unchanged() {
        let env = build_env_for_native("", "aarch64-unknown-linux-musl", false, None, "");
        assert!(has(&env, "RUSTFLAGS").is_none());
    }

    /// An operator's own RUSTFLAGS is merged, not dropped.
    #[test]
    fn existing_rustflags_are_kept() {
        let env = build_env_for_native("", "aarch64-unknown-linux-musl", true,
                                       Some("-C opt-level=s"), "");
        let rf = has(&env, "RUSTFLAGS").unwrap();
        assert!(rf.contains("opt-level=s") && rf.contains("-crt-static"), "{rf}");
    }
}
}
