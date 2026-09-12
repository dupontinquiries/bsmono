//! Packs: how content moves from the depot that can see the internet to the
//! one that cannot. A pack is one file — a USB stick, an `scp`, a `curl` —
//! and it is:
//!
//! ```text
//! <name>.depot                     tar, gzip level 1
//!   depot-pack/manifest.json       format, created, signer, catalog SUBSET, object list
//!   depot-pack/manifest.sig        hex ed25519 over the manifest bytes
//!   depot-pack/objects/<sha256>    each object the receiver did not have
//! ```
//!
//! Three things make it fit the job:
//!
//! * **Deduplicated by content.** `depot have` on the offline side prints its
//!   object hashes; carry that list back and `depot pack --have have.txt`
//!   ships only what is missing. A weekly Fedora-updates delta is the updates,
//!   not the distribution.
//! * **Streamed both ways.** Objects are appended from and written to disk
//!   through 64 KiB buffers; the manifest is the only thing held in memory.
//!   The manifest comes FIRST, so unpack can verify the signature and refuse
//!   before reading a single object.
//! * **Compressed for the right reason.** gzip level 1, not 9 — .crate, .rpm
//!   and .nar.xz are already at entropy, so a stronger level buys ~1% for a
//!   lot of CPU on a phone. The metadata (index lines, XML fragments) is what
//!   compresses, and level 1 already gets most of that. See DECISIONS.md §6.
//!
//! On unpack every object is re-hashed as it is written and refused if it
//! does not match its name; a corrupt stick fails on the bad object, not
//! silently later inside dnf.

use crate::catalog::Catalog;
use crate::sig;
use crate::{invalid, Error, Result, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;

pub const PACK_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub created: u64,
    /// Producer's public key (hex) or None for an unsigned pack.
    pub signer: Option<String>,
    /// The catalog entries this pack carries. Merged on unpack by upsert.
    pub catalog: Catalog,
    /// Objects included, in archive order. Objects referenced by `catalog`
    /// but absent here were on the receiver's `have` list.
    pub objects: Vec<PackObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackObject {
    pub hash: String,
    pub size: u64,
}

/// Which parts of the catalog to ship.
#[derive(Debug, Clone, Default)]
pub struct Select {
    pub crates: bool,
    /// None = every repo; Some(list) = only those ids.
    pub rpm: Option<Vec<String>>,
    pub nix: bool,
    pub toolchains: bool,
}

impl Select {
    pub fn all() -> Select {
        Select { crates: true, rpm: Some(Vec::new()), nix: true, toolchains: true }
    }
    fn wants_repo(&self, id: &str) -> bool {
        match &self.rpm {
            None => false,
            Some(v) if v.is_empty() => true,
            Some(v) => v.iter().any(|x| x == id),
        }
    }
}

/// The subset of `full` selected by `sel`.
pub fn subset(full: &Catalog, sel: &Select) -> Catalog {
    let mut c = Catalog { version: full.version, updated: full.updated, ..Default::default() };
    if sel.crates {
        c.crates = full.crates.clone();
    }
    c.rpm = full.rpm.iter().filter(|(id, _)| sel.wants_repo(id)).map(|(k, v)| (k.clone(), v.clone())).collect::<BTreeMap<_, _>>();
    if sel.nix {
        c.nix = full.nix.clone();
    }
    if sel.toolchains {
        c.toolchains = full.toolchains.clone();
    }
    c
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub objects: usize,
    pub bytes: u64,
    pub skipped_have: usize,
}

/// Write a pack. `have` = hashes the receiver already holds. `key` = None
/// produces an unsigned pack, which `read` refuses unless told otherwise.
pub fn write(store: &Store, full: &Catalog, sel: &Select, have: &HashSet<String>, key: Option<&sig::SigningKey>, out: &Path) -> Result<WriteReport> {
    let cat = subset(full, sel);
    let mut objects = Vec::new();
    let mut rep = WriteReport::default();
    for h in cat.referenced_hashes() {
        if have.contains(&h) {
            rep.skipped_have += 1;
            continue;
        }
        let size = store.size_of(&h).ok_or_else(|| invalid(format!("catalog references object {h} which is not in the store")))?;
        objects.push(PackObject { hash: h, size });
    }
    let manifest = Manifest { format: PACK_FORMAT, created: crate::now(), signer: key.map(|k| k.public_hex.clone()), catalog: cat, objects: objects.clone() };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let sig_hex = key.map(|k| k.sign(&manifest_bytes));

    let tmp = out.with_extension("depot.partial");
    {
        let f = std::fs::File::create(&tmp)?;
        let gz = flate2::write::GzEncoder::new(std::io::BufWriter::new(f), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        tar.mode(tar::HeaderMode::Deterministic);
        append_bytes(&mut tar, "depot-pack/manifest.json", &manifest_bytes)?;
        if let Some(s) = &sig_hex {
            append_bytes(&mut tar, "depot-pack/manifest.sig", s.as_bytes())?;
        }
        for o in &objects {
            let mut f = std::fs::File::open(store.object_path(&o.hash))?;
            let mut h = tar::Header::new_gnu();
            h.set_size(o.size);
            h.set_mode(0o444);
            h.set_mtime(0);
            h.set_cksum();
            tar.append_data(&mut h, format!("depot-pack/objects/{}", o.hash), &mut f)?;
            rep.objects += 1;
            rep.bytes += o.size;
        }
        let gz = tar.into_inner()?;
        let mut w = gz.finish()?;
        w.flush()?;
        w.get_ref().sync_all()?;
    }
    std::fs::rename(&tmp, out)?;
    Ok(rep)
}

fn append_bytes<W: Write>(tar: &mut tar::Builder<W>, path: &str, data: &[u8]) -> Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o444);
    h.set_mtime(0);
    h.set_cksum();
    tar.append_data(&mut h, path, data)?;
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    pub signer: Option<String>,
    pub objects_new: usize,
    pub objects_present: usize,
    pub bytes: u64,
    pub crates_new: usize,
    pub rpm_new: usize,
    pub nix_new: usize,
    pub toolchain_new: usize,
}

/// Read a pack into `store`, merging its catalog into `catalog`. The manifest
/// signature is verified against `trust/` under `root` before any object is
/// stored; `allow_unsigned` is the operator saying "I carried this stick
/// myself" and is logged as such.
pub fn read(root: &Path, store: &Store, catalog: &mut Catalog, pack: &Path, allow_unsigned: bool) -> Result<ReadReport> {
    let f = std::fs::File::open(pack)?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(f));
    let mut ar = tar::Archive::new(gz);
    let mut entries = ar.entries()?;

    // 1. manifest
    let mut first = entries.next().ok_or_else(|| invalid("empty pack"))??;
    if first.path()?.to_string_lossy() != "depot-pack/manifest.json" {
        return Err(invalid("first entry is not depot-pack/manifest.json"));
    }
    let mut manifest_bytes = Vec::new();
    first.read_to_end(&mut manifest_bytes)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.format != PACK_FORMAT {
        return Err(invalid(format!("pack format {} (this depot reads {PACK_FORMAT})", manifest.format)));
    }

    // 2. signature — decide before touching the store
    let mut rep = ReadReport::default();
    let mut next = entries.next();
    let mut sig_hex: Option<String> = None;
    if let Some(Ok(e)) = &mut next {
        if e.path()?.to_string_lossy() == "depot-pack/manifest.sig" {
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            sig_hex = Some(s.trim().to_string());
            next = entries.next();
        }
    }
    match sig_hex {
        Some(s) => rep.signer = Some(sig::verify_trusted(root, &manifest_bytes, &s)?),
        None if allow_unsigned => rep.signer = None,
        None => return Err(Error::Untrusted("pack is unsigned; pass --allow-unsigned only if you produced it yourself".into())),
    }

    // 3. objects, streamed, each verified against its name
    let expected: HashSet<&str> = manifest.objects.iter().map(|o| o.hash.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(entry) = next {
        let mut e = entry?;
        let p = e.path()?.to_string_lossy().into_owned();
        let Some(hash) = p.strip_prefix("depot-pack/objects/") else {
            return Err(invalid(format!("unexpected entry {p}")));
        };
        if !expected.contains(hash) {
            return Err(invalid(format!("object {hash} is not in the manifest")));
        }
        let size = e.size();
        let stored = store.put_reader(&mut e, Some(hash))?;
        if stored.fresh {
            rep.objects_new += 1;
        } else {
            rep.objects_present += 1;
        }
        rep.bytes += size;
        seen.insert(hash.to_string());
        next = entries.next();
    }
    // Every object the manifest promised must have arrived (a truncated
    // stick shows up here, before the catalog claims things it cannot serve).
    for o in &manifest.objects {
        if !seen.contains(&o.hash) {
            return Err(invalid(format!("pack ended before object {} arrived", o.hash)));
        }
    }
    // And every object the catalog references must now exist locally.
    for h in manifest.catalog.referenced_hashes() {
        if !store.has(&h) {
            return Err(invalid(format!("catalog references {h}, which is neither in this pack nor already here (was it packed against the wrong have-list?)")));
        }
    }

    // 4. merge
    for (name, versions) in &manifest.catalog.crates {
        for v in versions {
            if catalog.upsert_crate(name, v.clone()) {
                rep.crates_new += 1;
            }
        }
    }
    for (id, repo) in &manifest.catalog.rpm {
        let dst = catalog.rpm.entry(id.clone()).or_default();
        if repo.gpg_key.is_some() {
            dst.gpg_key = repo.gpg_key.clone();
        }
        if !repo.description.is_empty() {
            dst.description = repo.description.clone();
        }
        for p in &repo.packages {
            if catalog.upsert_rpm(id, p.clone()) {
                rep.rpm_new += 1;
            }
        }
    }
    for n in &manifest.catalog.nix {
        if catalog.upsert_nix(n.clone()) {
            rep.nix_new += 1;
        }
    }
    for t in &manifest.catalog.toolchains {
        if catalog.upsert_toolchain(t.clone()) {
            rep.toolchain_new += 1;
        }
    }
    Ok(rep)
}

/// Peek at a pack's manifest without storing anything (for `depot inspect`).
pub fn inspect(pack: &Path) -> Result<Manifest> {
    let f = std::fs::File::open(pack)?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(f));
    let mut ar = tar::Archive::new(gz);
    let mut first = ar.entries()?.next().ok_or_else(|| invalid("empty pack"))??;
    let mut b = Vec::new();
    first.read_to_end(&mut b)?;
    Ok(serde_json::from_slice(&b)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CrateEntry, ToolchainEntry};

    fn seeded(tag: &str) -> (std::path::PathBuf, Store, Catalog) {
        let d = crate::tmpdir(tag);
        let st = Store::open(&d);
        st.init().unwrap();
        let mut cat = Catalog::default();
        let a = st.put_bytes(b"crate a").unwrap();
        let b = st.put_bytes(b"toolchain b").unwrap();
        cat.upsert_crate("a", CrateEntry { vers: "0.1.0".into(), hash: a.hash, size: a.size, index_line: serde_json::json!({"name":"a"}), yanked: false, added: 0 });
        cat.upsert_toolchain(ToolchainEntry { version: "1.96.0".into(), triple: "x".into(), component: "rustc".into(), filename: "rustc-1.96.0-x.tar.xz".into(), hash: b.hash, size: b.size, added: 0 });
        (d, st, cat)
    }

    #[test]
    fn signed_pack_roundtrips_dedups_and_verifies() {
        let (src, sst, scat) = seeded("pack-src");
        let (dst, dst_store, mut dcat) = seeded("pack-dst");
        // dst already has "crate a" (seeded identically) → have-list dedup
        let pk = sig::keygen(&src).unwrap();
        let key = sig::SigningKey::load(&src).unwrap();
        let have: HashSet<String> = dst_store.list().unwrap().into_iter().collect();
        let out = src.join("sync.depot");
        let rep = write(&sst, &scat, &Select::all(), &have, Some(&key), &out).unwrap();
        assert_eq!(rep.objects, 0, "receiver had everything");
        assert_eq!(rep.skipped_have, 2);

        // Now add something the receiver lacks and pack against the same have-list.
        let mut scat2 = scat.clone();
        let c = sst.put_bytes(b"crate c new").unwrap();
        scat2.upsert_crate("c", CrateEntry { vers: "2.0.0".into(), hash: c.hash.clone(), size: c.size, index_line: serde_json::json!({"name":"c"}), yanked: false, added: 0 });
        let rep = write(&sst, &scat2, &Select::all(), &have, Some(&key), &out).unwrap();
        assert_eq!(rep.objects, 1);
        assert_eq!(rep.bytes, 11);
        assert!(!out.with_extension("depot.partial").exists());

        let m = inspect(&out).unwrap();
        assert_eq!(m.signer.as_deref(), Some(pk.as_str()));
        assert_eq!(m.objects.len(), 1);

        // Untrusted receiver refuses before storing anything.
        let before = dst_store.list().unwrap().len();
        let err = read(&dst, &dst_store, &mut dcat, &out, false).unwrap_err();
        assert!(matches!(err, Error::Untrusted(_)), "{err}");
        assert_eq!(dst_store.list().unwrap().len(), before);

        sig::trust(&dst, "src", &pk).unwrap();
        let r = read(&dst, &dst_store, &mut dcat, &out, false).unwrap();
        assert_eq!(r.signer.as_deref(), Some("src"));
        assert_eq!((r.objects_new, r.objects_present, r.crates_new, r.toolchain_new), (1, 0, 1, 0));
        assert!(dst_store.has(&c.hash));
        assert_eq!(dcat.crates["c"][0].vers, "2.0.0");
        // Reading the same pack again is a no-op for objects and catalog.
        let r2 = read(&dst, &dst_store, &mut dcat, &out, false).unwrap();
        assert_eq!((r2.objects_new, r2.objects_present, r2.crates_new), (0, 1, 0));

        std::fs::remove_dir_all(src).unwrap();
        std::fs::remove_dir_all(dst).unwrap();
    }

    #[test]
    fn unsigned_pack_needs_explicit_permission_and_tampering_is_caught() {
        let (src, sst, scat) = seeded("pack-u-src");
        let (dst, dst_store, mut dcat) = seeded("pack-u-dst");
        let out = src.join("u.depot");
        write(&sst, &scat, &Select::all(), &HashSet::new(), None, &out).unwrap();
        assert!(matches!(read(&dst, &dst_store, &mut dcat, &out, false), Err(Error::Untrusted(_))));
        let r = read(&dst, &dst_store, &mut dcat, &out, true).unwrap();
        assert_eq!(r.signer, None);
        assert_eq!(r.objects_present, 2);

        // Tamper: flip a byte inside an object in a re-packed archive whose
        // manifest still promises the original hash.
        let raw = {
            let f = std::fs::File::open(&out).unwrap();
            let mut gz = flate2::read::GzDecoder::new(f);
            let mut v = Vec::new();
            gz.read_to_end(&mut v).unwrap();
            v
        };
        let pos = raw.windows(7).position(|w| w == b"crate a").unwrap();
        let mut bad = raw.clone();
        bad[pos] = b'X';
        let bad_path = src.join("bad.depot");
        {
            let f = std::fs::File::create(&bad_path).unwrap();
            let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            gz.write_all(&bad).unwrap();
            gz.finish().unwrap();
        }
        let (dst2, dst2_store, mut dcat2) = (crate::tmpdir("pack-u-dst2"), Store::open(&crate::tmpdir("pack-u-dst2s")), Catalog::default());
        dst2_store.init().unwrap();
        let err = read(&dst2, &dst2_store, &mut dcat2, &bad_path, true).unwrap_err();
        assert!(err.to_string().contains("sha256"), "{err}");
        assert!(dcat2.crates.is_empty(), "catalog untouched after a bad object");

        // Selection: toolchains only.
        let only = Select { toolchains: true, ..Default::default() };
        let sub = subset(&scat, &only);
        assert!(sub.crates.is_empty() && sub.toolchains.len() == 1);
        let sel_repo = Select { rpm: Some(vec!["f42".into()]), ..Default::default() };
        assert!(sel_repo.wants_repo("f42") && !sel_repo.wants_repo("f41"));
        assert!(Select::all().wants_repo("anything"));

        std::fs::remove_dir_all(src).unwrap();
        std::fs::remove_dir_all(dst).unwrap();
        let _ = std::fs::remove_dir_all(dst2);
    }
}
