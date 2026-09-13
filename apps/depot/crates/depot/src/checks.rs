//! Live checks for tutorial steps. voiceterm's notes wizard gates each step
//! with `can_advance(step, cfg) -> Result<(), String>`; the same shape works
//! for a tutorial that runs *inside* the thing it teaches: a step can declare
//! `%check: <key>` and the depot evaluates it against its own state on every
//! `/api/tutorials` request. `Ok(detail)` is "this depot already satisfies
//! the step — here is the evidence"; `Err(what)` is "not yet — here is what
//! is missing". Cheap by construction: every key reads the catalog or config
//! already in memory; none hashes or touches the network.

use depot_core::Depot;
use serde::Serialize;

/// Every key `run` understands. The tutorial parser test checks each
/// `%check:` against this list, so a typo in a markdown file fails the
/// build's tests rather than silently rendering no badge.
#[cfg_attr(not(test), allow(dead_code))]
pub const KEYS: &[&str] = &[
    "reachable", "initialised", "public_url_set", "public_url_usb", "writes_enabled", "signing_key", "trusted_any", "has_crates", "has_rpm", "rpm_keys", "has_toolchains", "has_nix", "has_content",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Outcome {
    pub key: String,
    pub ok: bool,
    pub detail: String,
}

pub fn run(key: &str, d: &Depot) -> Option<Outcome> {
    let r: Result<String, String> = match key {
        "reachable" => Ok("you are reading this page from the depot, so the client you are on reaches it".into()),
        "initialised" => Ok(format!("root at {}", d.root.display())),
        "public_url_set" => match d.config.server.public_url.trim() {
            "" => Err("public_url is empty in depot.toml; generated client files will use whatever Host: the browser sent".into()),
            u => Ok(format!("public_url = {u}")),
        },
        "public_url_usb" => match d.config.server.public_url.trim() {
            u if u.contains("172.16.42.") => Ok(format!("public_url = {u}")),
            "" => Err("public_url is empty; set it to http://172.16.42.1:8000 for a USB-only depot".into()),
            u => Err(format!("public_url = {u} — not the USB address; fine if this depot serves over the LAN too")),
        },
        "writes_enabled" => {
            if d.root.join(&d.config.server.admin_token_file).is_file() {
                Ok("admin token present: HTTP writes enabled".into())
            } else {
                Err("no admin token: this depot is read-only over HTTP (depot admin-token --new)".into())
            }
        }
        "signing_key" => match std::fs::read_to_string(d.root.join(depot_core::sig::PUB_FILE)) {
            Ok(k) => Ok(format!("signing key {}…", &k.trim()[..16.min(k.trim().len())])),
            Err(_) => Err("no signing key (depot keygen)".into()),
        },
        "trusted_any" => match depot_core::sig::trusted_keys(&d.root) {
            Ok(k) if !k.is_empty() => Ok(format!("trusts: {}", k.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))),
            _ => Err("trust/ is empty: this depot cannot unpack anyone's packs yet".into()),
        },
        "has_crates" => {
            let c = d.catalog.counts();
            if c.crate_versions > 0 {
                Ok(format!("{} crates, {} versions", c.crates, c.crate_versions))
            } else {
                Err("no crates yet".into())
            }
        }
        "has_rpm" => {
            let c = d.catalog.counts();
            if c.rpm_packages > 0 {
                Ok(format!("{} packages in {} repo(s)", c.rpm_packages, c.rpm_repos))
            } else {
                Err("no RPM repos yet".into())
            }
        }
        "rpm_keys" => {
            let missing: Vec<&str> = d.catalog.rpm.iter().filter(|(_, r)| r.gpg_key.is_none()).map(|(id, _)| id.as_str()).collect();
            if d.catalog.rpm.is_empty() {
                Err("no RPM repos yet".into())
            } else if missing.is_empty() {
                Ok("every repo has a GPG key attached".into())
            } else {
                Err(format!("no key on: {}", missing.join(", ")))
            }
        }
        "has_toolchains" => {
            let c = d.catalog.counts();
            if c.toolchain_files > 0 {
                Ok(format!("{} toolchain files", c.toolchain_files))
            } else {
                Err("no toolchains yet".into())
            }
        }
        "has_nix" => {
            let c = d.catalog.counts();
            if c.nix_paths > 0 {
                Ok(format!("{} store paths", c.nix_paths))
            } else {
                Err("no nix store paths yet".into())
            }
        }
        "has_content" => {
            let c = d.catalog.counts();
            let n = c.crate_versions + c.rpm_packages + c.nix_paths + c.toolchain_files;
            if n > 0 {
                Ok(format!("{n} artifacts in the catalog"))
            } else {
                Err("the catalog is empty".into())
            }
        }
        _ => return None,
    };
    Some(match r {
        Ok(detail) => Outcome { key: key.into(), ok: true, detail },
        Err(detail) => Outcome { key: key.into(), ok: false, detail },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_flip_as_the_depot_changes() {
        let root = std::env::temp_dir().join(format!("depot-checks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut d = Depot::init(&root).unwrap();
        for k in KEYS {
            assert!(run(k, &d).is_some(), "{k} is implemented");
        }
        assert!(run("nope", &d).is_none());
        assert!(run("initialised", &d).unwrap().ok && run("signing_key", &d).unwrap().ok && run("reachable", &d).unwrap().ok);
        for k in ["public_url_set", "public_url_usb", "writes_enabled", "trusted_any", "has_crates", "has_rpm", "rpm_keys", "has_toolchains", "has_nix", "has_content"] {
            assert!(!run(k, &d).unwrap().ok, "{k} is not satisfied on a fresh depot");
        }
        d.config.server.public_url = "http://172.16.42.1:8000".into();
        assert!(run("public_url_set", &d).unwrap().ok && run("public_url_usb", &d).unwrap().ok);
        d.config.server.public_url = "http://depot.lan:8000".into();
        assert!(!run("public_url_usb", &d).unwrap().ok);
        std::fs::write(root.join("keys/admin.token"), "x").unwrap();
        assert!(run("writes_enabled", &d).unwrap().ok);
        depot_core::sig::trust(&root, "online", &"a".repeat(64)).unwrap();
        assert!(run("trusted_any", &d).unwrap().detail.contains("online"));
        let td = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../depot-core/testdata");
        d.ingest_crate(&td.join("cfg-if-1.0.0.crate")).unwrap();
        d.ingest_rpm("r", &td.join("foo-1.0-1.noarch.rpm"), false).unwrap();
        assert!(run("has_crates", &d).unwrap().ok && run("has_rpm", &d).unwrap().ok && run("has_content", &d).unwrap().ok);
        assert!(run("rpm_keys", &d).unwrap().detail.contains("no key on: r"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
