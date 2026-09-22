//! Alpine (apk) repositories: the fifth projection.
//!
//! ```text
//! pub/alpine/<branch>/<repo>/<arch>/APKINDEX.tar.gz   Alpine's index, signed by Alpine
//! pub/alpine/<branch>/<repo>/<arch>/<name>-<ver>.apk  each package, signed by Alpine
//! ```
//!
//! Like nix, depot does not rebuild or re-sign anything: it serves the
//! index and packages byte-for-byte, so every client keeps verifying
//! ALPINE's signatures (`apk` checks the index signature and each package's
//! checksum against it). A compromised depot therefore cannot inject a
//! package. Consumers: postmarketOS phones, and the Alpine userland gitmind
//! keeps under /home on reMarkable tablets (`gitmind eink pkg ... --mirror
//! http://depot.lan:8000/alpine`).
//!
//! Ingest takes a directory laid out like the mirror (`<branch>/<repo>/<arch>/`
//! holding `APKINDEX.tar.gz` and any subset of its `.apk` files), filled on
//! the online side with `curl` for the index and `apk fetch` / `curl` for the
//! packages the fleet needs. A PARTIAL mirror is normal: the report says how
//! many packages the index names that were not ingested (an install of one
//! of those gets a 404 from depot and says so).
//!
//! Each ingested `.apk` is checked against the index's `S:` (size) field,
//! which catches truncated downloads. Its `C:` checksum is left to `apk`,
//! which verifies it against the signed index at install time.

use crate::catalog::{AlpineRepo, ApkEntry};
use crate::{invalid, Result, Store};
use std::io::Read;
use std::path::Path;

/// One package as the index describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexPkg {
    pub name: String,
    pub version: String,
    pub size: u64,
    /// `D:` dependencies, version constraints stripped (`so:libc.musl...`,
    /// `cmd:sh`, plain names). Conflicts (`!name`) are left out.
    pub depends: Vec<String>,
    /// `p:` what it provides besides its own name, versions stripped.
    pub provides: Vec<String>,
}

/// A dependency or provides token without its version constraint
/// (`so:libfoo.so.1=1.2.3` -> `so:libfoo.so.1`, `busybox>=1.36` -> `busybox`).
pub fn strip_version(tok: &str) -> &str {
    let end = tok.find(|c: char| "<>=~".contains(c)).unwrap_or(tok.len());
    &tok[..end]
}

impl IndexPkg {
    pub fn filename(&self) -> String {
        format!("{}-{}.apk", self.name, self.version)
    }
}

/// The packages listed in an `APKINDEX.tar.gz` (signature segment, then a
/// tar with `DESCRIPTION` and `APKINDEX`, each gzipped: read as one stream
/// of concatenated members, tar end-markers ignored between segments).
pub fn parse_index(tgz: &[u8]) -> Result<Vec<IndexPkg>> {
    let gz = flate2::read::MultiGzDecoder::new(tgz);
    let mut ar = tar::Archive::new(gz);
    ar.set_ignore_zeros(true);
    for e in ar.entries()? {
        let mut e = e?;
        if e.path()?.as_os_str() == "APKINDEX" {
            let mut text = String::new();
            e.read_to_string(&mut text)?;
            return Ok(parse_index_text(&text));
        }
    }
    Err(invalid("no APKINDEX inside the index archive"))
}

pub fn parse_index_text(text: &str) -> Vec<IndexPkg> {
    text.split("\n\n")
        .filter_map(|block| {
            let (mut n, mut v, mut s) = (None, None, 0u64);
            let (mut d, mut p) = (Vec::new(), Vec::new());
            let toks = |x: &str| x.split_whitespace().filter(|t| !t.starts_with('!')).map(|t| strip_version(t).to_string()).collect::<Vec<_>>();
            for l in block.lines() {
                match l.split_once(':') {
                    Some(("P", x)) => n = Some(x.to_string()),
                    Some(("V", x)) => v = Some(x.to_string()),
                    Some(("S", x)) => s = x.parse().unwrap_or(0),
                    Some(("D", x)) => d = toks(x),
                    Some(("p", x)) => p = toks(x),
                    _ => {}
                }
            }
            Some(IndexPkg { name: n?, version: v?, size: s, depends: d, provides: p })
        })
        .collect()
}

/// What installing `wanted` needs from `index`: every package in the
/// dependency closure (by name or by a `p:` provide, the way apk resolves
/// `so:` and `cmd:` dependencies), and the dependencies nothing provides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Closure {
    pub needed: Vec<IndexPkg>,
    pub unresolved: Vec<String>,
}

pub fn closure(index: &[IndexPkg], wanted: &[&str]) -> Closure {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let by_name: BTreeMap<&str, &IndexPkg> = index.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut provider: BTreeMap<&str, &str> = BTreeMap::new();
    for p in index {
        for t in &p.provides {
            provider.entry(t.as_str()).or_insert(p.name.as_str());
        }
    }
    let (mut seen, mut unresolved) = (BTreeSet::new(), BTreeSet::new());
    let mut queue: VecDeque<String> = wanted.iter().map(|w| w.to_string()).collect();
    while let Some(dep) = queue.pop_front() {
        let name = by_name.get(dep.as_str()).map(|p| p.name.as_str()).or_else(|| provider.get(dep.as_str()).copied());
        let Some(name) = name else {
            unresolved.insert(dep);
            continue;
        };
        if seen.insert(name.to_string()) {
            for d in &by_name[name].depends {
                queue.push_back(d.clone());
            }
        }
    }
    Closure {
        needed: seen.iter().map(|n| by_name[n.as_str()].clone()).collect(),
        unresolved: unresolved.into_iter().collect(),
    }
}

/// `branch/repo/arch`, each segment a plain name.
pub fn check_key(key: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('/').collect();
    let ok = parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)));
    if ok { Ok(()) } else { Err(invalid(format!("{key}: expected <branch>/<repo>/<arch>"))) }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub repos: usize,
    pub packages: usize,
    pub new_packages: usize,
    /// Named by an index but not present in the directory.
    pub not_ingested: usize,
    pub errors: Vec<String>,
}

/// Every `<branch>/<repo>/<arch>/APKINDEX.tar.gz` under `dir`, with the
/// packages beside it that the index names.
pub fn ingest_dir(store: &Store, dir: &Path) -> Result<(Vec<(String, AlpineRepo)>, IngestReport)> {
    let mut rep = IngestReport::default();
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut indexes = Vec::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "APKINDEX.tar.gz") {
                indexes.push(p);
            }
        }
    }
    indexes.sort();
    for idx in indexes {
        let parent = idx.parent().unwrap();
        let key = match parent.strip_prefix(dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if let Err(e) = check_key(&key) {
            rep.errors.push(e.to_string());
            continue;
        }
        let bytes = std::fs::read(&idx)?;
        let listed = match parse_index(&bytes) {
            Ok(l) => l,
            Err(e) => {
                rep.errors.push(format!("{key}: {e}"));
                continue;
            }
        };
        let index = store.put_bytes(&bytes)?;
        let mut repo = AlpineRepo { index: index.hash, index_size: index.size, packages: Vec::new() };
        for p in &listed {
            let f = parent.join(p.filename());
            let Ok(meta) = std::fs::metadata(&f) else {
                rep.not_ingested += 1;
                continue;
            };
            if p.size != 0 && meta.len() != p.size {
                rep.errors.push(format!("{key}/{}: {} bytes, the index says {} (truncated download?)", p.filename(), meta.len(), p.size));
                continue;
            }
            let s = store.put_file(&f, None)?;
            rep.packages += 1;
            if s.fresh {
                rep.new_packages += 1;
            }
            repo.packages.push(ApkEntry { name: p.name.clone(), version: p.version.clone(), filename: p.filename(), hash: s.hash, size: s.size, added: crate::now() });
        }
        rep.repos += 1;
        out.push((key, repo));
    }
    Ok((out, rep))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn gz_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        let tar = b.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// An APKINDEX.tar.gz the way Alpine builds one: a signature segment,
    /// then the index segment, each its own gzip member.
    pub fn apkindex(text: &str) -> Vec<u8> {
        let mut v = gz_tar(&[(".SIGN.RSA.alpine-devel@lists.alpinelinux.org-6165ee59.rsa.pub", b"sig")]);
        v.extend(gz_tar(&[("DESCRIPTION", b"v3.22"), ("APKINDEX", text.as_bytes())]));
        v
    }

    #[test]
    fn the_index_is_read_past_the_signature_segment() {
        let t = "C:Q1abc\nP:bluez-alsa\nV:4.3.1-r0\nS:123\n\nC:Q1def\nP:busybox\nV:1.37.0-r18\nS:456\n\n";
        let l = parse_index(&apkindex(t)).unwrap();
        assert_eq!(l.iter().map(|p| (p.name.as_str(), p.version.as_str(), p.size)).collect::<Vec<_>>(),
            vec![("bluez-alsa", "4.3.1-r0", 123), ("busybox", "1.37.0-r18", 456)]);
        assert_eq!(l[0].filename(), "bluez-alsa-4.3.1-r0.apk");
    }

    #[test]
    fn the_closure_follows_names_and_provides_like_apk() {
        let idx = parse_index_text(concat!(
            "P:bluez-alsa\nV:4-r0\nD:so:libasound.so.2 so:libsbc.so.1 dbus-libs>=1.14 !pulseaudio-bluez\n\n",
            "P:alsa-lib\nV:1-r0\np:so:libasound.so.2=2.0.0\nD:so:libc.musl-aarch64.so.1\n\n",
            "P:sbc\nV:2-r0\np:so:libsbc.so.1=1.3\n\n",
            "P:dbus-libs\nV:1.14-r0\n\n",
            "P:musl\nV:1.2-r0\np:so:libc.musl-aarch64.so.1=1\n\n",
            "P:unrelated\nV:1-r0\n\n",
        ));
        assert_eq!(idx[0].depends, vec!["so:libasound.so.2", "so:libsbc.so.1", "dbus-libs"], "constraints stripped, conflicts dropped");
        let c = closure(&idx, &["bluez-alsa"]);
        let names: Vec<&str> = c.needed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alsa-lib", "bluez-alsa", "dbus-libs", "musl", "sbc"]);
        assert!(c.unresolved.is_empty());
        let c = closure(&idx, &["bluez-alsa", "nope"]);
        assert_eq!(c.unresolved, vec!["nope"]);
    }

    #[test]
    fn keys_are_branch_repo_arch_and_nothing_sneaky() {
        assert!(check_key("v3.22/main/aarch64").is_ok());
        assert!(check_key("latest-stable/community/x86_64").is_ok());
        assert!(check_key("v3.22/main").is_err());
        assert!(check_key("../etc/aarch64/x").is_err());
        assert!(check_key("v3.22/../aarch64").is_err());
    }
}

#[cfg(test)]
mod depot_tests {
    use super::tests::{apkindex, gz_tar};
    use crate::depot::Depot;

    /// A partial mirror of one repo, as the online side would fetch it.
    fn mirror(dir: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
        let r = dir.join("v3.22/main/aarch64");
        std::fs::create_dir_all(&r).unwrap();
        let pkg_a = gz_tar(&[(".PKGINFO", b"pkgname = bluez-alsa\n")]);
        let pkg_b = gz_tar(&[(".PKGINFO", b"pkgname = busybox\n")]);
        let text = format!(
            "P:bluez-alsa\nV:4.3.1-r0\nS:{}\n\nP:busybox\nV:1.37.0-r18\nS:{}\n\nP:not-fetched\nV:1-r0\nS:10\n\n",
            pkg_a.len(), pkg_b.len() + 5 // busybox's file will be SHORT: a truncated download
        );
        let idx = apkindex(&text);
        std::fs::write(r.join("APKINDEX.tar.gz"), &idx).unwrap();
        std::fs::write(r.join("bluez-alsa-4.3.1-r0.apk"), &pkg_a).unwrap();
        std::fs::write(r.join("busybox-1.37.0-r18.apk"), &pkg_b).unwrap();
        (idx, pkg_a)
    }

    #[test]
    fn an_alpine_tree_is_ingested_verified_and_served_byte_for_byte() {
        let base = std::env::temp_dir().join(format!("depot_alpine_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root) = (base.join("fetched"), base.join("depot"));
        let (idx, pkg_a) = mirror(&src);
        let mut d = Depot::init(&root).unwrap();
        let rep = d.ingest_alpine_dir(&src).unwrap();
        assert_eq!((rep.repos, rep.packages, rep.not_ingested), (1, 1, 1), "{rep:?}");
        assert_eq!(rep.errors.len(), 1, "the truncated package is refused: {:?}", rep.errors);
        assert!(rep.errors[0].contains("truncated"));
        let served = root.join("pub/alpine/v3.22/main/aarch64");
        assert_eq!(std::fs::read(served.join("APKINDEX.tar.gz")).unwrap(), idx, "Alpine's signed index, unchanged");
        assert_eq!(std::fs::read(served.join("bluez-alsa-4.3.1-r0.apk")).unwrap(), pkg_a);
        assert!(!served.join("busybox-1.37.0-r18.apk").exists());
        let c = d.catalog.counts();
        assert_eq!((c.alpine_repos, c.alpine_packages), (1, 1));
        // referenced objects include both, so gc and packs keep them
        let refs = d.catalog.referenced_hashes();
        let repo = &d.catalog.alpine["v3.22/main/aarch64"];
        assert!(refs.contains(&repo.index) && refs.contains(&repo.packages[0].hash));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_pack_carries_alpine_repos_to_another_depot() {
        let base = std::env::temp_dir().join(format!("depot_alpine_pack_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("fetched");
        let (idx, pkg_a) = mirror(&src);
        let mut online = Depot::init(&base.join("online")).unwrap();
        online.ingest_alpine_dir(&src).unwrap();
        let pack = base.join("alpine.pack");
        let sel = crate::pack::Select { alpine: true, ..Default::default() };
        online.pack(&sel, &Default::default(), &pack, true).unwrap();
        let mut offline = Depot::init(&base.join("offline")).unwrap();
        let rep = offline.unpack(&pack, true).unwrap();
        assert_eq!(rep.alpine_new, 1);
        let served = base.join("offline/pub/alpine/v3.22/main/aarch64");
        assert_eq!(std::fs::read(served.join("APKINDEX.tar.gz")).unwrap(), idx);
        assert_eq!(std::fs::read(served.join("bluez-alsa-4.3.1-r0.apk")).unwrap(), pkg_a);
        let _ = std::fs::remove_dir_all(&base);
    }
}
