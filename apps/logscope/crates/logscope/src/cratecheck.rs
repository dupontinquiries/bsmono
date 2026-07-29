//! cratecheck.rs — does this crate exist? Answered locally first, network last.
//!
//! Same layering as the rest of the validation stack: free and offline before slow and remote.
//!
//! 1. **The manifest** — a `use` with no dependency cannot compile. Free, exact, no lookup.
//! 2. **`Cargo.lock`** — every crate this workspace actually resolved, with versions. Free.
//! 3. **The local cargo registry** — `~/.cargo/registry/cache` holds every crate ever downloaded on this
//!    machine. Free, and on a working machine it is a large sample of real crate names.
//! 4. **crates.io** — authoritative, and the only step that can fail, hang, or leak what you are working
//!    on to a third party.
//!
//! The ordering is not just about speed. `autonomy::Reach` records that no core capability may REQUIRE
//! the network, so every answer here has to be useful offline — the remote step upgrades a verdict from
//! "not found locally" to "does not exist", and its absence must never turn a working check into a
//! broken one.
//!
//! ## The distinction that keeps this honest
//!
//! **Not found locally is not the same as does not exist.** A machine that has never built a TUI has no
//! `ratatui` in its cache, and rejecting it would be wrong. So local absence yields `Unknown` — a prompt
//! to check, never a verdict — and only the manifest can produce a hard failure offline.

use std::collections::BTreeSet;
use std::path::Path;

/// How well-attested a crate name is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Known {
    /// Declared in the manifest being built. The strongest local signal.
    Declared,
    /// Present in Cargo.lock — this workspace has resolved it.
    Locked,
    /// Downloaded on this machine at some point.
    Cached,
    /// Nothing local says anything. NOT a verdict that it is fake.
    Unknown,
}

impl Known {
    /// Is this enough to stop asking? Only a manifest entry settles it offline.
    pub fn is_settled(self) -> bool { !matches!(self, Known::Unknown) }

    /// What a reader should conclude — worded so `Unknown` cannot be read as "fake".
    pub fn explain(self, name: &str) -> String {
        match self {
            Known::Declared => format!("`{name}` is declared in the manifest"),
            Known::Locked => format!("`{name}` is in Cargo.lock — this workspace resolves it"),
            Known::Cached => format!("`{name}` has been downloaded on this machine"),
            Known::Unknown => format!("`{name}` is not known LOCALLY, which is not evidence it does not \
exist — this machine may simply never have used it. Check crates.io to be sure."),
        }
    }

    /// Should the caller spend a network lookup on this?
    pub fn needs_network(self) -> bool { matches!(self, Known::Unknown) }
}

/// Crate names in `~/.cargo/registry/cache/*/`, from the `.crate` filenames.
///
/// A filename is `<name>-<version>.crate`, and a name may itself contain hyphens — so the version is
/// stripped from the RIGHT at the last hyphen followed by a digit. Splitting on the first hyphen would
/// turn `accesskit-0.24.0` into `accesskit` correctly but `serde-json-1.0.0` into `serde`, which then
/// silently vouches for a crate that was never there.
pub fn scan_cache(cargo_home: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let cache = cargo_home.join("registry").join("cache");
    let Ok(dirs) = std::fs::read_dir(&cache) else { return out };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let n = f.file_name().to_string_lossy().into_owned();
            let Some(stem) = n.strip_suffix(".crate") else { continue };
            if let Some(name) = crate_name_of(stem) { out.insert(name); }
        }
    }
    out
}

/// `serde_json-1.0.0` -> `serde_json`. Strips at the LAST hyphen that starts a version.
pub fn crate_name_of(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    let mut cut = None;
    for (i, w) in bytes.windows(2).enumerate() {
        if w[0] == b'-' && w[1].is_ascii_digit() { cut = Some(i); }
    }
    cut.map(|i| stem[..i].to_string()).filter(|s| !s.is_empty())
}

/// Crate names from a Cargo.lock.
pub fn scan_lock(lock_text: &str) -> BTreeSet<String> {
    lock_text.lines()
        .filter_map(|l| l.trim().strip_prefix("name = "))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve one crate name against the local sources, cheapest first.
pub fn check_local(name: &str, manifest: Option<&str>, locked: &BTreeSet<String>,
                   cached: &BTreeSet<String>) -> Known {
    if let Some(m) = manifest {
        let declared = m.lines().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && t.starts_with(name)
                && t[name.len()..].trim_start().starts_with(['=', '.', '{'])
        });
        if declared { return Known::Declared; }
    }
    if locked.contains(name) { return Known::Locked; }
    if cached.contains(name) { return Known::Cached; }
    Known::Unknown
}

/// The crates.io sparse-index URL for a name, for the caller to fetch when it chooses to.
///
/// Returns the URL rather than performing the request: this crate does no I/O to the network, so the
/// decision to reach out — and the ability to decline — stays with the caller. `serde` lives at
/// `se/rd/serde`; names shorter than four characters use length-prefixed directories.
pub fn sparse_index_url(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let path = match n.len() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[0..1]),
        _ => format!("{}/{}/{n}", &n[0..2], &n[2..4]),
    };
    Some(format!("https://index.crates.io/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> BTreeSet<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn local_absence_is_NOT_evidence_that_a_crate_is_fake() {
        // The distinction that keeps this honest. A machine that has never built a TUI has no ratatui in
        // its cache, and rejecting it on that basis would be wrong — and would be wrong most often for
        // exactly the niche crates a person needs help with.
        let k = check_local("ratatui", None, &set(&[]), &set(&[]));
        assert_eq!(k, Known::Unknown);
        assert!(!k.is_settled());
        assert!(k.explain("ratatui").contains("not evidence it does not exist"));
        assert!(k.needs_network(), "this is the case worth a lookup");
    }

    #[test]
    fn the_manifest_settles_it_without_any_lookup() {
        let m = "[dependencies]\nratatui = \"0.29\"\n";
        let k = check_local("ratatui", Some(m), &set(&[]), &set(&[]));
        assert_eq!(k, Known::Declared);
        assert!(!k.needs_network(), "nothing to gain from asking crates.io");
    }

    #[test]
    fn sources_are_consulted_cheapest_first() {
        let m = "[dependencies]\nserde = \"1\"\n";
        assert_eq!(check_local("serde", Some(m), &set(&["serde"]), &set(&["serde"])), Known::Declared);
        assert_eq!(check_local("serde", None, &set(&["serde"]), &set(&["serde"])), Known::Locked);
        assert_eq!(check_local("serde", None, &set(&[]), &set(&["serde"])), Known::Cached);
    }

    #[test]
    fn a_version_is_stripped_from_the_RIGHT_so_hyphenated_names_survive() {
        // Splitting on the FIRST hyphen turns serde-json-1.0.0 into "serde", which then silently vouches
        // for a crate that was never in the cache — the worst kind of wrong here, since it manufactures
        // a false positive out of a real file.
        assert_eq!(crate_name_of("serde_json-1.0.0").as_deref(), Some("serde_json"));
        assert_eq!(crate_name_of("accesskit_atspi_common-0.18.1").as_deref(), Some("accesskit_atspi_common"));
        assert_eq!(crate_name_of("wasm-bindgen-futures-0.4.50").as_deref(), Some("wasm-bindgen-futures"));
        assert_eq!(crate_name_of("no-version-here"), None, "not a versioned crate file");
    }

    #[test]
    fn a_commented_out_dependency_does_not_count_as_declared() {
        let m = "[dependencies]\n# ratatui = \"0.29\"\n";
        assert_eq!(check_local("ratatui", Some(m), &set(&[]), &set(&[])), Known::Unknown);
    }

    #[test]
    fn the_index_url_follows_the_sparse_layout_including_short_names() {
        assert_eq!(sparse_index_url("serde").as_deref(), Some("https://index.crates.io/se/rd/serde"));
        assert_eq!(sparse_index_url("ratatui").as_deref(), Some("https://index.crates.io/ra/ta/ratatui"));
        assert_eq!(sparse_index_url("gcd").as_deref(), Some("https://index.crates.io/3/g/gcd"));
        assert_eq!(sparse_index_url("if").as_deref(), Some("https://index.crates.io/2/if"));
        // and a name that cannot be a crate yields no URL rather than a malformed request
        assert!(sparse_index_url("../../etc/passwd").is_none());
        assert!(sparse_index_url("").is_none());
    }

    #[test]
    fn scanning_a_missing_cargo_home_is_empty_not_a_panic() {
        // An offline or minimal machine must degrade to "no local knowledge", not fail.
        assert!(scan_cache(Path::new("/nonexistent/cargo")).is_empty());
    }

    #[test]
    fn lock_parsing_takes_names_and_ignores_everything_else() {
        let lock = "[[package]]\nname = \"ratatui\"\nversion = \"0.29.0\"\n\n[[package]]\nname = \"serde\"\n";
        let s = scan_lock(lock);
        assert!(s.contains("ratatui") && s.contains("serde"));
        assert!(!s.contains("0.29.0"), "versions are not crate names");
        assert_eq!(s.len(), 2);
    }
}
