//! `Depot`: root + store + catalog + config, and the operations the CLI and
//! the web shell expose. Every mutation ends with `catalog.save` and a
//! targeted view rebuild, so the served tree is never behind the catalog.

use crate::catalog::{Catalog, CrateEntry, RpmEntry};
use crate::config::Config;
use crate::pack::{self, ReadReport, Select, WriteReport};
use crate::views::Views;
use crate::{crates, invalid, nix, rpm, sig, toolchain, Result, Store};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Depot {
    pub root: PathBuf,
    pub store: Store,
    pub catalog: Catalog,
    pub config: Config,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Ingested {
    pub kind: &'static str,
    pub id: String,
    pub hash: String,
    pub size: u64,
    /// false = byte-identical object was already present.
    pub fresh: bool,
}

impl Depot {
    /// Create the skeleton, config and (if absent) a signing key. Idempotent
    /// except for the key, which is only generated when missing.
    pub fn init(root: &Path) -> Result<Depot> {
        let store = Store::open(root);
        store.init()?;
        if !root.join(crate::config::CONFIG_FILE).exists() {
            crate::write_atomic(&root.join(crate::config::CONFIG_FILE), Config::annotated_default().as_bytes())?;
        }
        if !root.join(sig::KEY_FILE).exists() {
            sig::keygen(root)?;
        }
        let mut d = Depot::open(root)?;
        d.catalog.save(root)?;
        d.rebuild_all()?;
        Ok(d)
    }

    pub fn open(root: &Path) -> Result<Depot> {
        if !root.join("objects").is_dir() {
            return Err(invalid(format!("{} is not a depot (no objects/); run `depot init --root {}`", root.display(), root.display())));
        }
        Ok(Depot { root: root.to_path_buf(), store: Store::open(root), catalog: Catalog::load(root)?, config: Config::load(root)? })
    }

    fn views(&self) -> Views<'_> {
        Views { store: &self.store, catalog: &self.catalog, config: &self.config }
    }

    pub fn rebuild_all(&self) -> Result<()> {
        self.views().rebuild_all()
    }

    // ── ingest ───────────────────────────────────────────────────────────────

    pub fn ingest_crate(&mut self, path: &Path) -> Result<Ingested> {
        let m = crates::read_crate(path)?;
        let s = self.store.put_file(path, Some(&m.cksum))?;
        self.finish_crate(m, s)
    }

    /// Ingest a `.crate` that is already an object in the store (an HTTP
    /// upload streamed there without holding this lock). Only the
    /// Cargo.toml scan happens here; no hashing, no copy.
    pub fn ingest_crate_stored(&mut self, hash: &str) -> Result<Ingested> {
        let size = self.store.size_of(hash).ok_or_else(|| invalid(format!("object {hash} is not in the store")))?;
        let m = crates::read_crate_known(&self.store.object_path(hash), hash, size)?;
        self.finish_crate(m, crate::store::Stored { hash: hash.to_string(), size, fresh: true })
    }

    fn finish_crate(&mut self, m: crates::CrateMeta, s: crate::store::Stored) -> Result<Ingested> {
        self.catalog.upsert_crate(&m.name, CrateEntry { vers: m.vers.clone(), hash: s.hash.clone(), size: s.size, index_line: m.index_line, yanked: false, added: crate::now() });
        self.catalog.save(&self.root)?;
        self.views().rebuild_crate(&m.name)?;
        Ok(Ingested { kind: "crate", id: format!("{}@{}", m.name, m.vers), hash: s.hash, size: s.size, fresh: s.fresh })
    }

    /// Ingest an RPM into `repo`, deriving its three repodata fragments. The
    /// repo's repodata is rebuilt unless `defer_views` (batch ingest calls
    /// `rebuild_rpm_repo` once at the end).
    pub fn ingest_rpm(&mut self, repo: &str, path: &Path, defer_views: bool) -> Result<Ingested> {
        check_repo_id(repo)?;
        let info = rpm::read_rpm(path)?;
        let s = self.store.put_file(path, Some(&info.sha256))?;
        self.finish_rpm(repo, info, s, defer_views)
    }

    /// `ingest_rpm` for an object already in the store: reads the headers
    /// only (the pkgid IS the object name).
    pub fn ingest_rpm_stored(&mut self, repo: &str, hash: &str, defer_views: bool) -> Result<Ingested> {
        check_repo_id(repo)?;
        let size = self.store.size_of(hash).ok_or_else(|| invalid(format!("object {hash} is not in the store")))?;
        let info = rpm::read_rpm_known(&self.store.object_path(hash), hash, size)?;
        self.finish_rpm(repo, info, crate::store::Stored { hash: hash.to_string(), size, fresh: true }, defer_views)
    }

    fn finish_rpm(&mut self, repo: &str, info: rpm::RpmInfo, s: crate::store::Stored, defer_views: bool) -> Result<Ingested> {
        let filename = info.filename();
        let href = crate::views::rpm_href(&info.name, &filename);
        let now = crate::now();
        let p = self.store.put_bytes(rpm::primary_fragment(&info, &href, now).as_bytes())?;
        let f = self.store.put_bytes(rpm::filelists_fragment(&info).as_bytes())?;
        let o = self.store.put_bytes(rpm::other_fragment(&info).as_bytes())?;
        self.catalog.upsert_rpm(
            repo,
            RpmEntry {
                name: info.name.clone(),
                epoch: info.epoch,
                version: info.version.clone(),
                release: info.release.clone(),
                arch: if info.is_source { "src".into() } else { info.arch.clone() },
                filename,
                hash: s.hash.clone(),
                size: s.size,
                primary_xml: p.hash,
                filelists_xml: f.hash,
                other_xml: o.hash,
                added: now,
            },
        );
        self.catalog.save(&self.root)?;
        if !defer_views {
            self.views().rebuild_rpm_repo(repo)?;
        }
        Ok(Ingested { kind: "rpm", id: format!("{repo}/{}", info.nevra()), hash: s.hash, size: s.size, fresh: s.fresh })
    }

    pub fn rebuild_rpm_repo(&self, repo: &str) -> Result<()> {
        self.views().rebuild_rpm_repo(repo)
    }

    /// Attach a GPG public key (ASCII-armored) to a repo; served as RPM-GPG-KEY.
    pub fn set_rpm_gpg_key(&mut self, repo: &str, key_file: &Path) -> Result<Ingested> {
        check_repo_id(repo)?;
        let text = std::fs::read(key_file)?;
        if !text.starts_with(b"-----BEGIN PGP PUBLIC KEY BLOCK-----") {
            return Err(invalid(format!("{}: not an ASCII-armored PGP public key", key_file.display())));
        }
        let s = self.store.put_bytes(&text)?;
        self.catalog.rpm.entry(repo.to_string()).or_default().gpg_key = Some(s.hash.clone());
        self.catalog.save(&self.root)?;
        self.views().rebuild_rpm_repo(repo)?;
        Ok(Ingested { kind: "gpg-key", id: repo.into(), hash: s.hash, size: s.size, fresh: s.fresh })
    }

    pub fn set_rpm_description(&mut self, repo: &str, desc: &str) -> Result<()> {
        check_repo_id(repo)?;
        self.catalog.rpm.entry(repo.to_string()).or_default().description = desc.to_string();
        self.catalog.save(&self.root)
    }

    pub fn ingest_nix_dir(&mut self, dir: &Path) -> Result<nix::IngestReport> {
        let (entries, rep) = nix::ingest_dir(&self.store, dir)?;
        for e in entries {
            self.catalog.upsert_nix(e);
        }
        self.catalog.save(&self.root)?;
        self.views().rebuild_nix()?;
        Ok(rep)
    }

    /// An Alpine repository tree (`<branch>/<repo>/<arch>/APKINDEX.tar.gz`
    /// plus any subset of its packages); see alpine.rs.
    pub fn ingest_alpine_dir(&mut self, dir: &Path) -> Result<crate::alpine::IngestReport> {
        let (repos, rep) = crate::alpine::ingest_dir(&self.store, dir)?;
        for (key, repo) in repos {
            self.catalog.upsert_alpine(&key, repo);
        }
        self.catalog.save(&self.root)?;
        self.views().rebuild_alpine()?;
        Ok(rep)
    }

    pub fn ingest_toolchain(&mut self, path: &Path) -> Result<(Vec<Ingested>, Vec<String>)> {
        let mut out = Vec::new();
        let errors;
        if path.is_dir() {
            let (entries, errs) = toolchain::ingest_dir(&self.store, path)?;
            errors = errs;
            for e in entries {
                out.push(Ingested { kind: "toolchain", id: format!("{}/{}/{}", e.version, e.triple, e.component), hash: e.hash.clone(), size: e.size, fresh: true });
                self.catalog.upsert_toolchain(e);
            }
        } else {
            let (e, fresh) = toolchain::ingest_file(&self.store, path)?;
            errors = Vec::new();
            out.push(Ingested { kind: "toolchain", id: format!("{}/{}/{}", e.version, e.triple, e.component), hash: e.hash.clone(), size: e.size, fresh });
            self.catalog.upsert_toolchain(e);
        }
        self.catalog.save(&self.root)?;
        self.views().rebuild_toolchains()?;
        Ok((out, errors))
    }

    /// A toolchain tarball already in the store: the filename carries the
    /// metadata, the object carries the bytes.
    pub fn ingest_toolchain_stored(&mut self, filename: &str, hash: &str) -> Result<Ingested> {
        let size = self.store.size_of(hash).ok_or_else(|| invalid(format!("object {hash} is not in the store")))?;
        let (component, version, triple) = toolchain::parse_component_filename(filename).ok_or_else(|| invalid(format!("{filename}: not <component>-<version>-<triple>.tar.xz")))?;
        toolchain::check_magic(&self.store.object_path(hash), filename)?;
        let e = crate::catalog::ToolchainEntry { version, triple, component, filename: filename.to_string(), hash: hash.to_string(), size, added: crate::now() };
        let id = format!("{}/{}/{}", e.version, e.triple, e.component);
        self.catalog.upsert_toolchain(e);
        self.catalog.save(&self.root)?;
        self.views().rebuild_toolchains()?;
        Ok(Ingested { kind: "toolchain", id, hash: hash.to_string(), size, fresh: true })
    }

    pub fn yank_crate(&mut self, name: &str, vers: &str, yanked: bool) -> Result<()> {
        let v = self.catalog.crates.get_mut(name).and_then(|vs| vs.iter_mut().find(|e| e.vers == vers)).ok_or_else(|| invalid(format!("{name}@{vers} not in catalog")))?;
        v.yanked = yanked;
        self.catalog.save(&self.root)?;
        self.views().rebuild_crate(name)
    }

    // ── sync ─────────────────────────────────────────────────────────────────

    /// Sorted list of every object hash — the `have` list.
    pub fn have(&self) -> Result<Vec<String>> {
        Ok(self.store.list()?)
    }

    pub fn pack(&self, sel: &Select, have: &HashSet<String>, out: &Path, unsigned: bool) -> Result<WriteReport> {
        let key = if unsigned { None } else { Some(sig::SigningKey::load(&self.root)?) };
        pack::write(&self.store, &self.catalog, sel, have, key.as_ref(), out)
    }

    pub fn unpack(&mut self, pack_file: &Path, allow_unsigned: bool) -> Result<ReadReport> {
        let rep = pack::read(&self.root, &self.store, &mut self.catalog, pack_file, allow_unsigned)?;
        self.catalog.save(&self.root)?;
        self.rebuild_all()?;
        Ok(rep)
    }

    // ── maintenance ──────────────────────────────────────────────────────────

    /// Objects nothing in the catalog references. Returned, not deleted:
    /// `depot gc --delete` is the explicit step.
    pub fn orphans(&self) -> Result<Vec<String>> {
        let referenced: HashSet<String> = self.catalog.referenced_hashes().into_iter().collect();
        Ok(self.store.list()?.into_iter().filter(|h| !referenced.contains(h)).collect())
    }

    pub fn status(&self) -> Result<Status> {
        Ok(Status {
            root: self.root.display().to_string(),
            counts: self.catalog.counts(),
            objects: self.store.list()?.len(),
            bytes: self.store.disk_usage()?,
            updated: self.catalog.updated,
            signing_key: std::fs::read_to_string(self.root.join(sig::PUB_FILE)).ok().map(|s| s.trim().to_string()),
            trusted: sig::trusted_keys(&self.root)?.into_iter().map(|(n, _)| n).collect(),
            bind: self.config.server.bind.clone(),
            writes_enabled: self.root.join(&self.config.server.admin_token_file).is_file(),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub root: String,
    pub counts: crate::catalog::Counts,
    pub objects: usize,
    pub bytes: u64,
    pub updated: u64,
    pub signing_key: Option<String>,
    pub trusted: Vec<String>,
    pub bind: String,
    pub writes_enabled: bool,
}

/// Repo ids become directory names and URL segments.
pub fn check_repo_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') || id.starts_with('.') {
        return Err(invalid(format!("repo id {id:?}: use [A-Za-z0-9._-], e.g. fedora-42-x86_64")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(f: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(f)
    }

    #[test]
    fn end_to_end_ingest_pack_unpack_between_two_depots() {
        let a = crate::tmpdir("depot-a");
        let b = crate::tmpdir("depot-b");
        let mut da = Depot::init(&a).unwrap();
        assert!(a.join("depot.toml").is_file() && a.join("keys/depot.key").is_file());

        let r = da.ingest_crate(&testdata("cfg-if-1.0.0.crate")).unwrap();
        assert!(r.fresh && r.id == "cfg-if@1.0.0");
        assert!(!da.ingest_crate(&testdata("cfg-if-1.0.0.crate")).unwrap().fresh);
        da.ingest_crate(&testdata("unicode-ident-1.0.12.crate")).unwrap();
        for f in ["hello-1.0-1.i386.rpm", "hello-2.0-1.x86_64.rpm", "foo-1.0-1.noarch.rpm"] {
            da.ingest_rpm("test-repo", &testdata(f), true).unwrap();
        }
        da.rebuild_rpm_repo("test-repo").unwrap();
        assert!(da.ingest_rpm("bad/id", &testdata("foo-1.0-1.noarch.rpm"), false).is_err());
        assert!(da.ingest_rpm("r", &testdata("cfg-if-1.0.0.crate"), false).is_err(), "a crate is not an rpm");
        da.set_rpm_description("test-repo", "three tiny packages").unwrap();
        da.yank_crate("cfg-if", "1.0.0", true).unwrap();
        assert!(da.yank_crate("cfg-if", "9.9.9", true).is_err());

        // Served tree exists and index reflects the yank.
        let idx = std::fs::read_to_string(a.join("pub/crates/index/cf/g-/cfg-if")).unwrap();
        assert!(idx.contains("\"yanked\":true"));
        assert!(a.join("pub/rpm/test-repo/Packages/h/hello-2.0-1.x86_64.rpm").is_file());
        assert!(a.join("pub/rpm/test-repo/repodata/repomd.xml").is_file());
        let st = da.status().unwrap();
        assert_eq!((st.counts.crates, st.counts.crate_versions, st.counts.rpm_packages), (2, 2, 3));
        assert!(st.signing_key.is_some());
        assert!(da.orphans().unwrap().is_empty());

        // Sneakernet to B.
        let mut db = Depot::init(&b).unwrap();
        let have: HashSet<String> = db.have().unwrap().into_iter().collect();
        let out = a.join("sync.depot");
        let w = da.pack(&Select::all(), &have, &out, false).unwrap();
        // 2 crates + 3 rpms + 9 fragments, minus one: foo has no files and
        // no changelog, so its filelists and other fragments are byte-identical
        // and content-addressing stores them once.
        assert_eq!(w.objects, 2 + 3 + 9 - 1);
        assert!(db.unpack(&out, false).is_err(), "B does not trust A yet");
        sig::trust(&b, "a", st.signing_key.as_deref().unwrap()).unwrap();
        let rr = db.unpack(&out, false).unwrap();
        assert_eq!(rr.signer.as_deref(), Some("a"));
        assert_eq!((rr.crates_new, rr.rpm_new, rr.objects_new), (2, 3, 13));
        // B now serves the same things, from its own store.
        assert_eq!(std::fs::read_to_string(b.join("pub/crates/index/cf/g-/cfg-if")).unwrap(), idx);
        assert!(b.join("pub/rpm/test-repo/repodata/repomd.xml").is_file());
        assert_eq!(db.catalog.rpm["test-repo"].description, "three tiny packages");
        assert_eq!(db.store.verify().unwrap(), Vec::<String>::new());

        // Second pack against B's new have-list carries nothing.
        let have2: HashSet<String> = db.have().unwrap().into_iter().collect();
        let w2 = da.pack(&Select::all(), &have2, &out, false).unwrap();
        assert_eq!(w2.objects, 0);

        assert!(Depot::open(&crate::tmpdir("not-a-depot")).is_err());
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn stored_ingest_paths_match_file_ingest_and_reject_cleanly() {
        let a = crate::tmpdir("depot-stored-a");
        let b = crate::tmpdir("depot-stored-b");
        let mut file_side = Depot::init(&a).unwrap();
        let mut stream_side = Depot::init(&b).unwrap();
        // The HTTP path: bytes land in the store first (no lock), then the
        // catalog is built from the object. It must produce the same catalog
        // entries as ingesting the file directly.
        for (kind, f) in [("crate", "cfg-if-1.0.0.crate"), ("rpm", "edge-1.2.3-4.fc42.noarch.rpm")] {
            let path = testdata(f);
            let mut fh = std::fs::File::open(&path).unwrap();
            let stored = stream_side.store.put_reader(&mut fh, None).unwrap();
            assert!(stored.fresh);
            let (via_file, via_store) = if kind == "crate" {
                (file_side.ingest_crate(&path).unwrap(), stream_side.ingest_crate_stored(&stored.hash).unwrap())
            } else {
                (file_side.ingest_rpm("r", &path, false).unwrap(), stream_side.ingest_rpm_stored("r", &stored.hash, false).unwrap())
            };
            assert_eq!((via_file.id.clone(), via_file.hash.clone(), via_file.size), (via_store.id, via_store.hash, via_store.size));
        }
        assert_eq!(serde_json::to_string(&file_side.catalog.crates).unwrap(), serde_json::to_string(&stream_side.catalog.crates).unwrap());
        assert_eq!(file_side.catalog.rpm["r"].packages[0].primary_xml, stream_side.catalog.rpm["r"].packages[0].primary_xml, "same fragment object → same bytes");
        // Toolchain: name carries the metadata, object carries the bytes.
        let mut xz = vec![0xfd, b'7', b'z', b'X', b'Z', 0x00];
        xz.extend_from_slice(b"payload");
        let st = stream_side.store.put_bytes(&xz).unwrap();
        let t = stream_side.ingest_toolchain_stored("rust-std-1.96.0-aarch64-unknown-linux-musl.tar.xz", &st.hash).unwrap();
        assert_eq!(t.id, "1.96.0/aarch64-unknown-linux-musl/rust-std");
        assert!(stream_side.ingest_toolchain_stored("README.md", &st.hash).is_err());
        // A crate that is not a crate: the caller (web) removes the fresh
        // object; here we check the ingest error and Store::remove.
        let junk = stream_side.store.put_bytes(b"not a crate").unwrap();
        assert!(stream_side.ingest_crate_stored(&junk.hash).is_err());
        stream_side.store.remove(&junk.hash).unwrap();
        assert!(!stream_side.store.has(&junk.hash));
        assert!(stream_side.ingest_crate_stored(&"0".repeat(64)).is_err(), "unknown object");
        // put_file fast path: expected hash + object present → no read of src
        // (src may not even exist).
        let s = stream_side.store.put_file(Path::new("/nonexistent/file"), Some(&st.hash)).unwrap();
        assert_eq!((s.fresh, s.size), (false, xz.len() as u64));
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }
}
