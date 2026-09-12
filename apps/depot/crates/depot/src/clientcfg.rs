//! What a device has to be told to use the depot. Each snippet is the
//! smallest change to a stock Fedora / postmarketOS / NixOS box, and each
//! comment in the output explains the setting it touches, because these
//! files get pasted by people who have never opened them before.

use depot_core::Depot;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    pub kind: &'static str,
    /// Where it goes on the device.
    pub path: String,
    pub contents: String,
    /// What to run after writing it.
    pub after: String,
}

#[derive(Debug, Clone, Default)]
pub struct Want {
    pub crates: bool,
    pub rpm: Vec<String>,
    pub nix: bool,
    pub toolchain: bool,
}

pub fn snippets(depot: &Depot, server: &str, want: &Want) -> Vec<Snippet> {
    let server = server.trim_end_matches('/');
    let mut out = Vec::new();

    if want.crates {
        let reg = &depot.config.crates.registry_name;
        out.push(Snippet {
            kind: "cargo",
            path: "~/.cargo/config.toml  (or <project>/.cargo/config.toml)".into(),
            contents: format!(
                r#"# Source replacement: cargo keeps calling the registry "crates-io" in
# Cargo.lock (so lock files stay identical to the ones made online) but
# fetches every index line and .crate from the depot instead.
[source.crates-io]
replace-with = "{reg}"

[source.{reg}]
registry = "sparse+{server}/crates/index/"

# Do NOT set net.offline = true here. Offline mode forbids ALL fetching,
# including from the depot. The depot is your network now.
"#
            ),
            after: "cargo fetch   # in any project; every dep must already be in the depot".into(),
        });
    }

    for repo in &want.rpm {
        let has_key = depot.catalog.rpm.get(repo).map(|r| r.gpg_key.is_some()).unwrap_or(false);
        let (gpgcheck, gpgkey, note) = if has_key {
            (1, format!("gpgkey={server}/rpm/{repo}/RPM-GPG-KEY\n"), "")
        } else {
            (0, String::new(), "# gpgcheck=0 because this repo has no key attached in the depot.\n# Attach one with `depot rpm gpg-key --repo <id> RPM-GPG-KEY-fedora-42-primary`\n# and regenerate this file; dnf then verifies every package signature.\n")
        };
        out.push(Snippet {
            kind: "dnf",
            path: format!("/etc/yum.repos.d/depot-{repo}.repo"),
            contents: format!(
                r#"[depot-{repo}]
name=depot mirror: {repo}
baseurl={server}/rpm/{repo}/
enabled=1
{note}gpgcheck={gpgcheck}
{gpgkey}# How long dnf trusts its cached repomd.xml before asking again. Short,
# because a depot updates when a pack lands, not on Fedora's schedule.
metadata_expire=1h
# Fail loudly if the depot is down rather than silently using stale data.
skip_if_unavailable=False
"#
            ),
            after: format!("sudo dnf config-manager setopt fedora.enabled=0 updates.enabled=0   # or --disablerepo='*' --enablerepo='depot-*' per command\nsudo dnf makecache --repo depot-{repo}"),
        });
    }

    if want.nix {
        let sig_keys: Vec<String> = {
            let mut names: Vec<String> = Vec::new();
            for e in &depot.catalog.nix {
                if let Ok(t) = std::fs::read_to_string(depot.store.object_path(&e.narinfo)) {
                    for l in t.lines() {
                        if let Some(sig) = l.strip_prefix("Sig: ") {
                            if let Some((name, _)) = sig.split_once(':') {
                                if !names.iter().any(|n| n == name) {
                                    names.push(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
            names
        };
        let key_line = if sig_keys.is_empty() {
            "# No Sig: lines were found in the mirrored narinfos, so nix will refuse them\n# unless you either sign the cache (`nix store sign --key-file ...` before\n# `nix copy`) or set `require-sigs = false` (only on a network you own).".to_string()
        } else {
            format!("# The mirrored narinfos are signed by: {}\n# Put each key's PUBLIC half here, as it appears in the origin's nix.conf.\ntrusted-public-keys = {}", sig_keys.join(", "), sig_keys.iter().map(|k| format!("{k}:<public-key>")).collect::<Vec<_>>().join(" "))
        };
        out.push(Snippet {
            kind: "nix",
            path: "/etc/nix/nix.conf  (NixOS: nix.settings in configuration.nix)".into(),
            contents: format!(
                r#"# The depot first (priority {} beats cache.nixos.org's 40); the public cache
# stays listed so a box that CAN reach it still falls through.
substituters = {server}/nix https://cache.nixos.org
{key_line}
"#,
                depot.config.nix.priority
            ),
            after: "sudo systemctl restart nix-daemon   # then: nix build ... --option substituters ...".into(),
        });
    }

    if want.toolchain {
        let latest = depot.catalog.toolchains.iter().map(|t| t.version.as_str()).max().unwrap_or("<version>");
        out.push(Snippet {
            kind: "toolchain",
            path: "(run once per device)".into(),
            contents: format!(
                r#"curl -fsSL {server}/toolchains/install.sh -o depot-rust-install.sh
# Read it — it is short. Then, as root for /usr/local, or with a prefix you own:
sh depot-rust-install.sh {latest} "$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
"#
            ),
            after: "rustc --version && cargo --version".into(),
        });
    }
    out
}

/// A self-contained POSIX script that writes every snippet in `want` to its
/// place. Served at /bootstrap.sh so a fresh box needs one command; it prints
/// each file before writing and stops on the first failure.
pub fn bootstrap_script(depot: &Depot, server: &str, want: &Want) -> String {
    let mut s = String::from("#!/bin/sh\n# depot bootstrap — generated. Writes client configuration for this depot.\n# Every file it writes is printed first. Run with DRY=1 to only print.\nset -eu\nDRY=\"${DRY:-0}\"\nwrite() { # path <<contents on stdin>>\n  echo \"==> $1\"; cat > \"/tmp/depot-bootstrap.$$\"; cat \"/tmp/depot-bootstrap.$$\"\n  if [ \"$DRY\" = 0 ]; then mkdir -p \"$(dirname \"$1\")\"; cp \"/tmp/depot-bootstrap.$$\" \"$1\"; fi\n  rm -f \"/tmp/depot-bootstrap.$$\"\n}\n\n");
    for sn in snippets(depot, server, want) {
        match sn.kind {
            "cargo" => {
                s.push_str("write \"${CARGO_HOME:-$HOME/.cargo}/config.toml\" <<'EOF'\n");
                s.push_str(&sn.contents);
                s.push_str("EOF\n\n");
            }
            "dnf" => {
                s.push_str(&format!("write \"{}\" <<'EOF'\n", sn.path));
                s.push_str(&sn.contents);
                s.push_str("EOF\n\n");
            }
            "nix" => {
                s.push_str("write \"${NIX_CONF:-/etc/nix/nix.conf}\" <<'EOF'\n");
                s.push_str(&sn.contents);
                s.push_str("EOF\n\n");
            }
            "toolchain" => {
                s.push_str("# Rust toolchain: fetched and installed only when TOOLCHAIN=1.\nif [ \"${TOOLCHAIN:-0}\" = 1 ]; then\n");
                for l in sn.contents.lines() {
                    s.push_str("  ");
                    s.push_str(l);
                    s.push('\n');
                }
                s.push_str("fi\n\n");
            }
            _ => {}
        }
    }
    s.push_str("echo done\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use depot_core::Depot;

    #[test]
    fn snippets_cover_every_kind_and_gpg_state_changes_the_repo_file() {
        let root = std::env::temp_dir().join(format!("depot-cc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut d = Depot::init(&root).unwrap();
        d.catalog.rpm.entry("f42".into()).or_default();
        let want = Want { crates: true, rpm: vec!["f42".into()], nix: true, toolchain: true };
        let s = snippets(&d, "http://depot.lan:8000/", &want);
        assert_eq!(s.iter().map(|x| x.kind).collect::<Vec<_>>(), vec!["cargo", "dnf", "nix", "toolchain"]);
        assert!(s[0].contents.contains("registry = \"sparse+http://depot.lan:8000/crates/index/\""), "{}", s[0].contents);
        assert!(s[1].contents.contains("gpgcheck=0") && s[1].contents.contains("no key attached"));
        d.catalog.rpm.get_mut("f42").unwrap().gpg_key = Some("x".repeat(64));
        let s = snippets(&d, "http://depot.lan:8000", &want);
        assert!(s[1].contents.contains("gpgcheck=1") && s[1].contents.contains("gpgkey=http://depot.lan:8000/rpm/f42/RPM-GPG-KEY"));
        assert!(s[2].contents.contains("substituters = http://depot.lan:8000/nix https://cache.nixos.org"));
        let b = bootstrap_script(&d, "http://depot.lan:8000", &want);
        assert!(b.starts_with("#!/bin/sh") && b.contains("/etc/yum.repos.d/depot-f42.repo") && b.contains("TOOLCHAIN"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
