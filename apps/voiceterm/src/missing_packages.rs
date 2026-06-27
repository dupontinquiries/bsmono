// src/missing_packages.rs
//
// Tracks packages reported as missing across command output blocks.
// Surfaces them as a sidebar list so the user can see at-a-glance
// what's blocking their workflow.
//
// Recognized patterns:
//   - "command not found: <name>"           (zsh / fish)
//   - "<name>: command not found"           (bash)
//   - "ModuleNotFoundError: No module named '<name>'"   (python)
//   - "ImportError: No module named <name>"             (python 2)
//   - "Error: Cannot find module '<name>'"              (node)
//   - "package <name> is not installed"                 (apt)
//   - "<crate>: not found in registry"                  (cargo)
//   - "fatal: <pkg> not installed"                      (various)
//
// Each detected package gets:
//   - The name as reported
//   - The likely package manager(s) that would install it
//     (we don't run package managers — just suggest)
//   - First-seen and last-seen block IDs
//   - Hit count (how many blocks have referenced this missing thing)
//
// Persisted in-memory only; clears on restart. Sidebar shows the
// most-recent + most-frequent. User can dismiss entries.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// One missing-package record.
#[derive(Clone, Debug)]
pub struct MissingPackage {
    /// Name as reported in the error message.
    pub name: String,
    /// Detected language / ecosystem ("shell", "python", "node",
    /// "rust", "system"). Drives the suggested install command.
    pub ecosystem: Ecosystem,
    /// Suggested install command (best guess — user should verify
    /// before running). Format: "apt install <name>" / "pip install
    /// <name>" / "npm install <name>" etc.
    pub install_hint: String,
    /// Block IDs that referenced this. Used for "where did this
    /// come up?" lookups.
    pub block_ids: Vec<u32>,
    /// First time we saw it (epoch seconds).
    pub first_seen: u64,
    /// Last time we saw it (epoch seconds). For sidebar sort order.
    pub last_seen: u64,
    /// How many times we've seen it.
    pub hit_count: u32,
    /// User dismissed → don't surface in sidebar but keep in record
    /// (so a subsequent hit re-surfaces it).
    pub dismissed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ecosystem {
    Shell,    // command-not-found
    Python,   // ModuleNotFoundError
    Node,     // Cannot find module
    Rust,     // not found in registry
    System,   // generic / debian-style "package X is not installed"
}

/// v0.1.470: the system package manager, detected from the host.
/// Used to turn a vague Shell command-not-found into the RIGHT
/// install command for THIS machine, instead of the old punt
/// ("# try: apt OR dnf OR pacman"). Detection order (in
/// detect_sys_pkg_manager): binary presence first (authoritative —
/// the tool is literally installed), then /etc/os-release ID as a
/// fallback hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysPkgManager {
    Apt,      // debian/ubuntu
    Dnf,      // fedora/rhel
    Pacman,   // arch
    Apk,      // alpine
    Zypper,   // opensuse
    Brew,     // macos/linuxbrew
    Unknown,  // couldn't tell — fall back to the multi-suggestion punt
}

impl SysPkgManager {
    /// The install command for `name` under this package manager.
    /// Empty string for Unknown (caller handles the fallback).
    /// `name` is assumed already sanitized by the caller.
    pub fn install_command(&self, name: &str) -> String {
        match self {
            SysPkgManager::Apt => format!("sudo apt install {}", name),
            SysPkgManager::Dnf => format!("sudo dnf install {}", name),
            SysPkgManager::Pacman => format!("sudo pacman -S {}", name),
            SysPkgManager::Apk => format!("sudo apk add {}", name),
            SysPkgManager::Zypper => format!("sudo zypper install {}", name),
            // brew must NOT be run with sudo (it refuses, by design).
            SysPkgManager::Brew => format!("brew install {}", name),
            SysPkgManager::Unknown => String::new(),
        }
    }
}

/// Pure package-manager picker. Separated from IO so it can be
/// unit-tested: takes the /etc/os-release contents and a predicate
/// that answers "is this binary on PATH?". Binary presence wins
/// (the manager is literally installed); os-release ID is the
/// fallback. This is the testable core of detect_sys_pkg_manager.
///
/// `os_release` is the raw text of /etc/os-release (may be empty).
/// `has_bin` returns true if the named binary exists on PATH.
pub fn pick_pkg_manager(
    os_release: &str,
    has_bin: impl Fn(&str) -> bool,
) -> SysPkgManager {
    // 1. Binary presence — authoritative. Order matters only when a
    //    box has more than one (rare); we prefer the "native" manager
    //    for the distro family, but since we can't know that here
    //    without os-release, we check in rough popularity order and
    //    let os-release (step 2) override ties in practice. In the
    //    common single-manager case this is unambiguous.
    //    brew is checked LAST among binaries because a linuxbrew
    //    install commonly coexists with the distro's native manager,
    //    and the user almost always wants the native one for a
    //    system tool.
    if has_bin("apt") || has_bin("apt-get") { return SysPkgManager::Apt; }
    if has_bin("dnf") { return SysPkgManager::Dnf; }
    if has_bin("pacman") { return SysPkgManager::Pacman; }
    if has_bin("apk") { return SysPkgManager::Apk; }
    if has_bin("zypper") { return SysPkgManager::Zypper; }
    if has_bin("brew") { return SysPkgManager::Brew; }

    // 2. Fallback: parse ID and ID_LIKE from /etc/os-release.
    //    Used when binary detection failed (e.g. PATH not yet set in
    //    a fresh shell, or a minimal probe environment).
    //    v0.1.470: collect tokens from BOTH ID and ID_LIKE (not
    //    ID-or-ID_LIKE) and scan all of them. Real distros set
    //    ID=opensuse-leap / ID=opensuse-tumbleweed (hyphenated, with
    //    a generic family in ID_LIKE), and ID=pop with
    //    ID_LIKE="ubuntu debian" — stopping at ID would miss the
    //    family. We also prefix-match the suse family so
    //    "opensuse-leap" maps to Zypper.
    let mut tokens: Vec<String> = Vec::new();
    if let Some(id) = os_release_field(os_release, "ID") {
        tokens.extend(id.split_whitespace().map(|s| s.to_string()));
    }
    if let Some(id_like) = os_release_field(os_release, "ID_LIKE") {
        tokens.extend(id_like.split_whitespace().map(|s| s.to_string()));
    }
    for token in &tokens {
        let t = token.as_str();
        match t {
            "debian" | "ubuntu" | "linuxmint" | "pop" | "raspbian"
                => return SysPkgManager::Apt,
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux"
                => return SysPkgManager::Dnf,
            "arch" | "manjaro" | "endeavouros"
                => return SysPkgManager::Pacman,
            "alpine" => return SysPkgManager::Apk,
            _ => {
                // suse family ships hyphenated IDs (opensuse-leap,
                // opensuse-tumbleweed); prefix-match them.
                if t.starts_with("opensuse") || t == "suse" || t == "sles" {
                    return SysPkgManager::Zypper;
                }
            }
        }
    }
    SysPkgManager::Unknown
}

/// Extract a field value from /etc/os-release content. Lines look
/// like `ID=fedora` or `ID_LIKE="ubuntu debian"` (quotes optional).
/// Returns the unquoted, lowercased value. None if the key is absent.
fn os_release_field(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            // Must be exactly `KEY=...` — guard against KEY being a
            // prefix of another key (e.g. ID vs ID_LIKE).
            if let Some(val) = rest.strip_prefix('=') {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                if val.is_empty() { return None; }
                return Some(val.to_ascii_lowercase());
            }
        }
    }
    None
}

/// Detect the host's system package manager (IO wrapper around
/// pick_pkg_manager). Reads /etc/os-release and probes PATH via
/// `which`. Cached after first call — the host doesn't change
/// mid-session.
#[cfg(not(test))]
pub fn detect_sys_pkg_manager() -> SysPkgManager {
    use std::sync::OnceLock;
    static CACHE: OnceLock<SysPkgManager> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let os_release = std::fs::read_to_string("/etc/os-release")
            .unwrap_or_default();
        pick_pkg_manager(&os_release, |bin| {
            std::process::Command::new("which")
                .arg(bin)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    })
}

impl Ecosystem {
    /// Best-guess install command per ecosystem. Heuristic; user
    /// should verify before running on their system.
    pub fn install_hint_for(&self, name: &str) -> String {
        // Sanitize name — only alphanumeric + dash/underscore allowed.
        // Anything weirder is left empty so we don't suggest running
        // something dangerous.
        let safe: String = name.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .collect();
        if safe.is_empty() { return String::new(); }
        match self {
            Ecosystem::Shell => {
                // v0.1.470: use the DETECTED system package manager so
                // the hint is correct for THIS machine, instead of the
                // old punt that listed apt/dnf/pacman and made the user
                // figure out which. detect_sys_pkg_manager is
                // #[cfg(not(test))] (it does real IO), so under test we
                // fall straight to the multi-suggestion form — the
                // detection logic itself is tested via pick_pkg_manager.
                #[cfg(not(test))]
                {
                    let mgr = detect_sys_pkg_manager();
                    let cmd = mgr.install_command(&safe);
                    if !cmd.is_empty() {
                        return cmd;
                    }
                    // Unknown manager → fall through to the punt below.
                }
                // Fallback (Unknown manager, or test build): list the
                // common options and let the user adapt.
                format!("# try: sudo apt install {} OR sudo dnf install {} OR sudo pacman -S {}",
                    safe, safe, safe)
            }
            Ecosystem::Python => format!("pip install {}", safe),
            Ecosystem::Node => format!("npm install {}", safe),
            Ecosystem::Rust => format!("cargo add {}", safe),
            Ecosystem::System => format!("apt install {}", safe),
        }
    }
}

static REGISTRY: OnceLock<Mutex<HashMap<String, MissingPackage>>> = OnceLock::new();
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn registry() -> &'static Mutex<HashMap<String, MissingPackage>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Scan a block's output for missing-package signals. Updates the
/// registry. Called once per closed block from term_blocks.
pub fn scan_block(block_id: u32, output: &str) {
    let detected = detect_in_output(output);
    if detected.is_empty() { return; }
    let now = epoch_secs();
    let mut g = match registry().lock() { Ok(g) => g, Err(_) => return };
    let mut new_entries = 0u32;
    for (eco, name) in detected {
        let key = format!("{:?}:{}", eco, name);
        match g.get_mut(&key) {
            Some(existing) => {
                existing.hit_count += 1;
                existing.last_seen = now;
                if !existing.block_ids.contains(&block_id) {
                    existing.block_ids.push(block_id);
                }
                // Re-surface a dismissed package if it appears again.
                // Per design: dismiss is "I don't want to see this NOW",
                // not "I don't want to see this ever."
                if existing.dismissed {
                    existing.dismissed = false;
                }
            }
            None => {
                let install_hint = eco.install_hint_for(&name);
                g.insert(key.clone(), MissingPackage {
                    name: name.clone(),
                    ecosystem: eco,
                    install_hint,
                    block_ids: vec![block_id],
                    first_seen: now,
                    last_seen: now,
                    hit_count: 1,
                    dismissed: false,
                });
                new_entries += 1;
            }
        }
    }
    if new_entries > 0 {
        crate::clog!("missing-pkg", "{} new in block {} (total: {})",
            new_entries, block_id, g.len());
    }
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Pull all current entries (most-recent first), excluding dismissed.
pub fn list_active() -> Vec<MissingPackage> {
    let g = match registry().lock() { Ok(g) => g, Err(_) => return Vec::new() };
    let mut out: Vec<MissingPackage> = g.values()
        .filter(|p| !p.dismissed)
        .cloned()
        .collect();
    // Sort by recency, ties broken by hit_count.
    out.sort_by(|a, b|
        b.last_seen.cmp(&a.last_seen).then(b.hit_count.cmp(&a.hit_count)));
    out
}

/// Mark an entry dismissed. Idempotent — looks up by name+ecosystem.
pub fn dismiss(ecosystem: &str, name: &str) {
    let key = format!("{}:{}", ecosystem, name);
    let mut g = match registry().lock() { Ok(g) => g, Err(_) => return };
    // Try exact key first, fall back to scan because key format is
    // "Debug-formatted ecosystem", which the QML caller might not
    // get exactly right.
    if let Some(p) = g.get_mut(&key) {
        p.dismissed = true;
        crate::clog!("missing-pkg", "dismissed {}", key);
    } else {
        for (k, v) in g.iter_mut() {
            if v.name == name && k.starts_with(ecosystem) {
                v.dismissed = true;
                crate::clog!("missing-pkg", "dismissed {} (loose match)", k);
                break;
            }
        }
    }
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Monotonic counter — QML polls to detect new entries.
pub fn revision() -> u64 {
    REVISION.load(std::sync::atomic::Ordering::Relaxed)
}

fn epoch_secs() -> u64 { core_kit::time::now_secs() }

/// Pattern-match the output for known missing-package signals.
/// Returns (ecosystem, name) pairs. Conservative — false positives
/// here mean cluttering the sidebar with noise, so we use specific
/// patterns rather than generic "X not found" matching.
pub fn detect_in_output(output: &str) -> Vec<(Ecosystem, String)> {
    let mut out: Vec<(Ecosystem, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(Ecosystem, String)> = std::collections::HashSet::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }

        // Shell command-not-found. Two layouts:
        //   bash:  "<name>: command not found"
        //   zsh:   "zsh: command not found: <name>"
        //   fish:  "fish: Unknown command: <name>"
        if let Some(name) = parse_shell_cnf(t) {
            let key = (Ecosystem::Shell, name.clone());
            if seen.insert(key.clone()) {
                out.push(key);
            }
            continue;
        }

        // Python ModuleNotFoundError / ImportError
        if let Some(name) = parse_python_module(t) {
            let key = (Ecosystem::Python, name);
            if seen.insert(key.clone()) {
                out.push(key);
            }
            continue;
        }

        // Node "Cannot find module"
        if let Some(name) = parse_node_module(t) {
            let key = (Ecosystem::Node, name);
            if seen.insert(key.clone()) {
                out.push(key);
            }
            continue;
        }

        // Cargo / Rust
        if let Some(name) = parse_rust_crate(t) {
            let key = (Ecosystem::Rust, name);
            if seen.insert(key.clone()) {
                out.push(key);
            }
            continue;
        }

        // Generic "package X is not installed" (apt etc.)
        if let Some(name) = parse_apt_missing(t) {
            let key = (Ecosystem::System, name);
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
    }
    out
}

fn parse_shell_cnf(line: &str) -> Option<String> {
    // bash: "<name>: command not found"
    if let Some(idx) = line.find(": command not found") {
        let name = line[..idx].trim();
        // Reject prefix-y forms: "zsh: command not found: foo" — the
        // ": command not found" we matched was actually after "zsh".
        if name == "zsh" || name == "bash" || name == "fish" || name == "sh" {
            // Extract from after the colon.
            let after = &line[idx + ": command not found".len()..];
            let after = after.trim().trim_start_matches(':').trim();
            if after.is_empty() { return None; }
            return clean_command_name(after);
        }
        return clean_command_name(name);
    }
    // zsh: "zsh: command not found: <name>"
    if let Some(idx) = line.find("command not found:") {
        let name = line[idx + "command not found:".len()..].trim();
        if !name.is_empty() {
            return clean_command_name(name);
        }
    }
    // fish: "fish: Unknown command: <name>"
    if let Some(idx) = line.find("Unknown command:") {
        let name = line[idx + "Unknown command:".len()..].trim();
        if !name.is_empty() {
            return clean_command_name(name);
        }
    }
    // v0.1.635: nushell — its CNF format is entirely different from the
    // bash/zsh/fish family (it doesn't say "command not found"). The error
    // line reads: Command `<name>` not found  (inside nu's box-drawing
    // error display, often followed by a "help: Did you mean `x`?" line).
    // The command name is in BACKTICKS after the word "Command". Verified
    // against nushell docs/issues. Without this, missing-package
    // suggestions silently never fired for nu users.
    if let Some(idx) = line.find("Command `") {
        let after = &line[idx + "Command `".len()..];
        if let Some(end) = after.find('`') {
            let name = after[..end].trim();
            // guard: only treat as CNF if "not found" appears after the
            // closing backtick (avoids matching unrelated "Command `x`"
            // prose). nu puts "not found" right after.
            let tail = &after[end..];
            if tail.contains("not found") && !name.is_empty() {
                return clean_command_name(name);
            }
        }
    }
    None
}

fn parse_python_module(line: &str) -> Option<String> {
    // ModuleNotFoundError: No module named 'foo'
    // ImportError: No module named foo
    let needle = "No module named ";
    let idx = line.find(needle)?;
    let rest = line[idx + needle.len()..].trim();
    let unquoted = rest.trim_matches(|c: char| c == '\'' || c == '"');
    // Take only up to the first whitespace / ;  / newline-equivalent
    let name: String = unquoted.chars().take_while(|c|
        c.is_ascii_alphanumeric() || *c == '_' || *c == '.' || *c == '-'
    ).collect();
    if name.is_empty() { return None; }
    // Strip submodule suffix — "foo.bar" → "foo" (the package
    // typically owns the top-level name)
    let top = name.split('.').next().unwrap_or(&name).to_string();
    if top.is_empty() { return None; }
    Some(top)
}

fn parse_node_module(line: &str) -> Option<String> {
    // Error: Cannot find module 'foo'
    // Cannot find module 'foo'
    let needle = "Cannot find module ";
    let idx = line.find(needle)?;
    let rest = line[idx + needle.len()..].trim();
    let unquoted = rest.trim_matches(|c: char| c == '\'' || c == '"');
    let name: String = unquoted.chars().take_while(|c|
        c.is_ascii_alphanumeric() || *c == '@' || *c == '/' || *c == '-' || *c == '_' || *c == '.'
    ).collect();
    if name.is_empty() { return None; }
    Some(name)
}

fn parse_rust_crate(line: &str) -> Option<String> {
    // "error: no matching package named `foo` found"  (cargo add)
    if let Some(idx) = line.find("no matching package named `") {
        let rest = &line[idx + "no matching package named `".len()..];
        if let Some(end) = rest.find('`') {
            let name = &rest[..end];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    // "use of undeclared crate `foo`"  (rustc)
    if let Some(idx) = line.find("use of undeclared crate `") {
        let rest = &line[idx + "use of undeclared crate `".len()..];
        if let Some(end) = rest.find('`') {
            let name = &rest[..end];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn parse_apt_missing(line: &str) -> Option<String> {
    // "Package 'foo' is not installed"
    // "E: Unable to locate package foo"
    if let Some(idx) = line.find("Unable to locate package ") {
        let rest = &line[idx + "Unable to locate package ".len()..];
        let name: String = rest.chars().take_while(|c|
            c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.'
        ).collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn clean_command_name(s: &str) -> Option<String> {
    let trimmed = s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if trimmed.is_empty() || trimmed.len() > 60 { return None; }
    // Reject obvious noise — single-char names are usually parser
    // artifacts.
    if trimmed.len() < 2 { return None; }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_zsh_cnf() {
        let out = "zsh: command not found: ripgrep";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, Ecosystem::Shell);
        assert_eq!(r[0].1, "ripgrep");
    }

    #[test]
    fn detects_bash_cnf() {
        let out = "fzf: command not found";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "fzf");
    }

    #[test]
    fn detects_nushell_cnf() {
        // v0.1.635: nushell's distinct format. The name is in backticks
        // after "Command", with "not found" following.
        let out = "  ╰── Command `ripgrep` not found";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1, "should detect the nushell CNF");
        assert_eq!(r[0].0, Ecosystem::Shell);
        assert_eq!(r[0].1, "ripgrep");
    }

    #[test]
    fn nushell_cnf_plain_line() {
        let out = "Command `htop` not found";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "htop");
    }

    #[test]
    fn nushell_did_you_mean_not_parsed_as_command() {
        // the help line "help: Did you mean `du`?" must NOT be mistaken
        // for the missing command — it has no "not found" after the
        // backtick, so our guard rejects it.
        let out = "help: Did you mean `du`?";
        let r = detect_in_output(out);
        assert!(r.is_empty(), "the suggestion line must not be parsed as the missing cmd");
    }

    #[test]
    fn detects_python_module() {
        let out = "ModuleNotFoundError: No module named 'requests'";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, Ecosystem::Python);
        assert_eq!(r[0].1, "requests");
    }

    #[test]
    fn detects_python_module_submodule() {
        let out = "ModuleNotFoundError: No module named 'foo.bar.baz'";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "foo");  // top-level package
    }

    #[test]
    fn detects_node_module() {
        let out = "Error: Cannot find module 'lodash'";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, Ecosystem::Node);
        assert_eq!(r[0].1, "lodash");
    }

    #[test]
    fn dedupes_within_block() {
        let out = "foo: command not found\nfoo: command not found";
        let r = detect_in_output(out);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn empty_on_clean_output() {
        let out = "Hello world\n42 lines processed";
        assert_eq!(detect_in_output(out).len(), 0);
    }

    // ── v0.1.470: system package-manager detection ──

    #[test]
    fn pkg_binary_presence_wins() {
        assert_eq!(pick_pkg_manager("", |b| b == "dnf"), SysPkgManager::Dnf);
        assert_eq!(pick_pkg_manager("", |b| b == "pacman"), SysPkgManager::Pacman);
        assert_eq!(pick_pkg_manager("", |b| b == "apt-get"), SysPkgManager::Apt);
        assert_eq!(pick_pkg_manager("", |b| b == "apk"), SysPkgManager::Apk);
        assert_eq!(pick_pkg_manager("", |b| b == "zypper"), SysPkgManager::Zypper);
        assert_eq!(pick_pkg_manager("", |b| b == "brew"), SysPkgManager::Brew);
    }

    #[test]
    fn pkg_os_release_fallback() {
        let none = |_: &str| false;
        assert_eq!(pick_pkg_manager("ID=fedora\n", none), SysPkgManager::Dnf);
        assert_eq!(pick_pkg_manager("ID=arch\n", none), SysPkgManager::Pacman);
        assert_eq!(pick_pkg_manager("ID=ubuntu\n", none), SysPkgManager::Apt);
        assert_eq!(pick_pkg_manager("ID=alpine\n", none), SysPkgManager::Apk);
        assert_eq!(pick_pkg_manager("ID=opensuse-leap\nID_LIKE=\"suse opensuse\"\n", none),
            SysPkgManager::Zypper);
    }

    #[test]
    fn pkg_id_like_multitoken_and_quotes() {
        let none = |_: &str| false;
        assert_eq!(pick_pkg_manager("ID=pop\nID_LIKE=\"ubuntu debian\"\n", none),
            SysPkgManager::Apt);
        assert_eq!(pick_pkg_manager("ID=rocky\nID_LIKE=\"rhel centos fedora\"\n", none),
            SysPkgManager::Dnf);
    }

    #[test]
    fn pkg_id_prefix_guard() {
        // "ID_LIKE" must not be read as key "ID".
        assert_eq!(os_release_field("ID_LIKE=debian\n", "ID"), None);
        assert_eq!(os_release_field("ID=fedora\n", "ID"), Some("fedora".to_string()));
    }

    #[test]
    fn pkg_binary_beats_os_release() {
        // Box claims ubuntu but has dnf installed → trust the binary.
        assert_eq!(pick_pkg_manager("ID=ubuntu\n", |b| b == "dnf"),
            SysPkgManager::Dnf);
    }

    #[test]
    fn pkg_unknown_when_nothing_matches() {
        let none = |_: &str| false;
        assert_eq!(pick_pkg_manager("ID=plan9\n", none), SysPkgManager::Unknown);
        assert_eq!(pick_pkg_manager("", none), SysPkgManager::Unknown);
    }

    #[test]
    fn pkg_install_commands() {
        assert_eq!(SysPkgManager::Dnf.install_command("ripgrep"),
            "sudo dnf install ripgrep");
        assert_eq!(SysPkgManager::Pacman.install_command("fd"),
            "sudo pacman -S fd");
        // brew never takes sudo.
        assert_eq!(SysPkgManager::Brew.install_command("bat"), "brew install bat");
        // Unknown yields empty (caller falls back to the multi-punt).
        assert_eq!(SysPkgManager::Unknown.install_command("x"), "");
    }
}
