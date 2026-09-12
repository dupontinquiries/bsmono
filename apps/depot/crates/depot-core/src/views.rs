//! Projections: turn the catalog into the directory trees that cargo, dnf,
//! nix and a shell script expect, under `pub/`. Every artifact is a hardlink
//! into `objects/`; only small derived files (index lines, repodata, index
//! json) are written, and those atomically.
//!
//! A rebuild is idempotent and cheap: hardlinks that already point at the
//! right inode are left alone, and repodata is regenerated only when a repo's
//! membership changed (its fragment-hash list differs from the last build,
//! recorded in `repodata/.depot-state`).

use crate::catalog::{Catalog, RpmRepo};
use crate::config::Config;
use crate::{crates, nix, rpm, sha256_hex, toolchain, write_atomic, Result, Store};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Views<'a> {
    pub store: &'a Store,
    pub catalog: &'a Catalog,
    pub config: &'a Config,
}

impl Views<'_> {
    fn pub_dir(&self) -> PathBuf {
        self.store.pub_dir()
    }

    pub fn rebuild_all(&self) -> Result<()> {
        self.rebuild_crates_all()?;
        for repo in self.catalog.rpm.keys() {
            self.rebuild_rpm_repo(repo)?;
        }
        self.rebuild_nix()?;
        self.rebuild_toolchains()?;
        Ok(())
    }

    // ── crates ───────────────────────────────────────────────────────────────

    pub fn rebuild_crates_all(&self) -> Result<()> {
        let idx = self.pub_dir().join("crates/index");
        // Names that disappeared from the catalog must disappear from the
        // index too; wiping is simpler than diffing and the files are tiny.
        if idx.is_dir() {
            std::fs::remove_dir_all(&idx)?;
        }
        std::fs::create_dir_all(&idx)?;
        for name in self.catalog.crates.keys() {
            self.rebuild_crate(name)?;
        }
        Ok(())
    }

    /// One crate: its index file and a hardlink per version.
    pub fn rebuild_crate(&self, name: &str) -> Result<()> {
        let Some(versions) = self.catalog.crates.get(name) else { return Ok(()) };
        let base = self.pub_dir().join("crates");
        write_atomic(&base.join("index").join(crates::index_path(name)), crates::render_index(versions).as_bytes())?;
        for v in versions {
            self.store.link_into(&v.hash, &base.join(crates::dl_path(name, &v.vers)))?;
        }
        Ok(())
    }

    // ── rpm ──────────────────────────────────────────────────────────────────

    /// One repository: Packages/ hardlinks and, if membership changed, fresh
    /// repodata/ with hash-named metadata files and a new repomd.xml.
    pub fn rebuild_rpm_repo(&self, repo_id: &str) -> Result<()> {
        let Some(repo) = self.catalog.rpm.get(repo_id) else { return Ok(()) };
        let base = self.pub_dir().join("rpm").join(repo_id);
        std::fs::create_dir_all(&base)?;
        for p in &repo.packages {
            self.store.link_into(&p.hash, &base.join(rpm_href(&p.name, &p.filename)))?;
        }
        if let Some(k) = &repo.gpg_key {
            self.store.link_into(k, &base.join("RPM-GPG-KEY"))?;
        }
        let rd = base.join("repodata");
        std::fs::create_dir_all(&rd)?;
        let state = repo_state(repo);
        let state_file = rd.join(".depot-state");
        if std::fs::read_to_string(&state_file).map(|s| s == state).unwrap_or(false) && rd.join("repomd.xml").is_file() {
            return Ok(()); // membership unchanged: repomd.xml still valid
        }
        self.write_repodata(repo, &rd)?;
        write_atomic(&state_file, state.as_bytes())?;
        Ok(())
    }

    fn write_repodata(&self, repo: &RpmRepo, rd: &Path) -> Result<()> {
        let n = repo.packages.len();
        let ts = crate::now();
        let mut entries = Vec::new();
        let mut keep: Vec<String> = Vec::new();
        for (kind, head, tail, pick) in [
            ("primary", rpm::primary_head(n), rpm::PRIMARY_TAIL, 0usize),
            ("filelists", rpm::filelists_head(n), rpm::FILELISTS_TAIL, 1),
            ("other", rpm::other_head(n), rpm::OTHER_TAIL, 2),
        ] {
            // Concatenate fragments straight from the object store; the XML
            // never has to exist uncompressed on disk.
            let mut open = Vec::with_capacity(n * 1024 + head.len());
            open.extend_from_slice(head.as_bytes());
            for p in &repo.packages {
                let h = match pick {
                    0 => &p.primary_xml,
                    1 => &p.filelists_xml,
                    _ => &p.other_xml,
                };
                open.extend_from_slice(&std::fs::read(self.store.object_path(h))?);
            }
            open.extend_from_slice(tail.as_bytes());
            let open_sum = sha256_hex(&open);
            let (bytes, ext) = compress(&open, &self.config.rpm.compression)?;
            let sum = sha256_hex(&bytes);
            let fname = format!("{sum}-{kind}.xml.{ext}");
            write_atomic(&rd.join(&fname), &bytes)?;
            keep.push(fname.clone());
            entries.push(rpm::RepomdEntry { kind, href: format!("repodata/{fname}"), checksum: sum, open_checksum: open_sum, size: bytes.len() as u64, open_size: open.len() as u64, timestamp: ts });
        }
        write_atomic(&rd.join("repomd.xml"), rpm::render_repomd(&entries, ts).as_bytes())?;
        // Old hash-named files are unreachable once repomd.xml no longer
        // lists them; remove them so the directory does not grow forever.
        for e in std::fs::read_dir(rd)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "repomd.xml" || keep.contains(&name) {
                continue;
            }
            let _ = std::fs::remove_file(e.path());
        }
        Ok(())
    }

    // ── nix ──────────────────────────────────────────────────────────────────

    pub fn rebuild_nix(&self) -> Result<()> {
        let base = self.pub_dir().join("nix");
        std::fs::create_dir_all(&base)?;
        write_atomic(&base.join("nix-cache-info"), nix::cache_info(&self.config.nix.store_dir, self.config.nix.priority).as_bytes())?;
        for e in &self.catalog.nix {
            // narinfo is small and text: copy it out (a hardlink would be
            // 0444, which is fine, but a plain file is what nix tooling that
            // rewrites caches expects to be able to touch).
            let info = std::fs::read(self.store.object_path(&e.narinfo))?;
            write_atomic(&base.join(format!("{}.narinfo", e.hash_part)), &info)?;
            self.store.link_into(&e.nar_hash, &base.join(&e.nar_url))?;
        }
        Ok(())
    }

    // ── toolchains ───────────────────────────────────────────────────────────

    pub fn rebuild_toolchains(&self) -> Result<()> {
        let base = self.pub_dir().join("toolchains");
        std::fs::create_dir_all(&base)?;
        for e in &self.catalog.toolchains {
            self.store.link_into(&e.hash, &base.join(toolchain::served_path(e)))?;
        }
        write_atomic(&base.join("index.json"), toolchain::render_index(&self.catalog.toolchains).as_bytes())?;
        // install.sh is served with the depot URL substituted at request time
        // by the web layer; the file on disk carries the placeholder so a
        // copy taken from disk still works with DEPOT=... set.
        write_atomic(&base.join("install.sh"), toolchain::INSTALL_SH.as_bytes())?;
        Ok(())
    }
}

/// `Packages/<first letter>/<filename>` — the href primary.xml carries.
pub fn rpm_href(name: &str, filename: &str) -> String {
    format!("Packages/{}/{}", rpm::packages_subdir(name), filename)
}

fn repo_state(repo: &RpmRepo) -> String {
    let mut v: Vec<&str> = repo.packages.iter().map(|p| p.primary_xml.as_str()).collect();
    v.sort();
    let mut s = v.join("\n");
    s.push('\n');
    s
}

fn compress(data: &[u8], kind: &str) -> Result<(Vec<u8>, &'static str)> {
    match kind {
        "zst" => {
            #[cfg(feature = "zstd")]
            {
                return Ok((zstd::encode_all(data, 19).map_err(crate::Error::Io)?, "zst"));
            }
            #[cfg(not(feature = "zstd"))]
            {
                Err(crate::invalid("rpm.compression = \"zst\" needs depot built with --features zstd"))
            }
        }
        _ => {
            let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(data.len() / 4), flate2::Compression::best());
            enc.write_all(data)?;
            Ok((enc.finish()?, "gz"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CrateEntry, RpmEntry};

    #[test]
    fn crates_and_rpm_views_are_projections_of_the_store() {
        let d = crate::tmpdir("views");
        let st = Store::open(&d);
        st.init().unwrap();
        let cfg = Config::default();
        let mut cat = Catalog::default();
        // a crate
        let c = st.put_bytes(b"crate bytes").unwrap();
        cat.upsert_crate("cfg-if", CrateEntry { vers: "1.0.0".into(), hash: c.hash.clone(), size: c.size, index_line: serde_json::json!({"name":"cfg-if","vers":"1.0.0","yanked":false}), yanked: false, added: 0 });
        // an rpm with fragments
        let r = st.put_bytes(b"rpm bytes").unwrap();
        let p = st.put_bytes(b"<package type=\"rpm\"><name>hello</name></package>\n").unwrap();
        let f = st.put_bytes(b"<package pkgid=\"x\" name=\"hello\" arch=\"noarch\"></package>\n").unwrap();
        let o = st.put_bytes(b"<package pkgid=\"x\" name=\"hello\" arch=\"noarch\"></package>\n").unwrap();
        cat.upsert_rpm("f42", RpmEntry { name: "hello".into(), epoch: 0, version: "1".into(), release: "1".into(), arch: "noarch".into(), filename: "hello-1-1.noarch.rpm".into(), hash: r.hash.clone(), size: r.size, primary_xml: p.hash, filelists_xml: f.hash, other_xml: o.hash, added: 0 });
        let v = Views { store: &st, catalog: &cat, config: &cfg };
        v.rebuild_all().unwrap();

        let idx = std::fs::read_to_string(d.join("pub/crates/index/cf/g-/cfg-if")).unwrap();
        assert!(idx.contains("\"vers\":\"1.0.0\""));
        assert_eq!(std::fs::read(d.join("pub/crates/dl/cfg-if/cfg-if-1.0.0.crate")).unwrap(), b"crate bytes");
        assert_eq!(std::fs::read(d.join("pub/rpm/f42/Packages/h/hello-1-1.noarch.rpm")).unwrap(), b"rpm bytes");
        let repomd = std::fs::read_to_string(d.join("pub/rpm/f42/repodata/repomd.xml")).unwrap();
        assert!(repomd.contains("<data type=\"primary\">") && repomd.contains("-primary.xml.gz"));
        let href = repomd.split("href=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        let gz = std::fs::read(d.join("pub/rpm/f42").join(&href)).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&gz[..]);
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut dec, &mut xml).unwrap();
        assert!(xml.starts_with("<?xml") && xml.contains("packages=\"1\"") && xml.contains("<name>hello</name>") && xml.ends_with("</metadata>"));
        // The open-checksum in repomd must be the sha256 of the decompressed XML.
        assert!(repomd.contains(&sha256_hex(xml.as_bytes())));
        // Unchanged membership: rebuild leaves repomd.xml alone.
        let before = std::fs::read(d.join("pub/rpm/f42/repodata/repomd.xml")).unwrap();
        v.rebuild_rpm_repo("f42").unwrap();
        assert_eq!(std::fs::read(d.join("pub/rpm/f42/repodata/repomd.xml")).unwrap(), before);
        assert!(d.join("pub/nix/nix-cache-info").is_file());
        assert!(d.join("pub/toolchains/index.json").is_file());
        std::fs::remove_dir_all(d).unwrap();
    }
}
