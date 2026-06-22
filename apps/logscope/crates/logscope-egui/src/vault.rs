//! vault.rs — project history as a content-addressed, branching store.
//!
//! Not git: this records the conversation tree and the files derived from it, linking every file version
//! to the TURN AND REASONING that produced it (provenance, not line-diff). Generated files are a
//! deterministic function of the transcript, so we version the (small, text) transcript and re-derive
//! files; the only thing that can be huge — external inputs (logs/attachments) — is tracked BY REFERENCE
//! (hash + size + mtime), bytes copied only under a size threshold. That is what keeps the store off the
//! 1 GB cliff. See HISTORY_DESIGN.md.
//!
//! Layout under <root>/:  blobs/<2>/<hash>  ·  nodes/<id>.json  ·  index.json
//! All content-addressed (128-bit = two seeded xxhash passes; blake3 is the export-grade upgrade). Writes
//! are atomic (tmp + rename). Individual blob files → seekable, concurrent reads, no giant file to parse.

use crate::workspace::{Artifact, Kind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub type NodeId = u64;
pub type Hash = String;

/// Inputs larger than this are referenced only (never copied into the store).
pub const INLINE_MAX: u64 = 8 * 1024 * 1024;

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// 128-bit content id of a byte slice (two seeded xxhash passes).
fn hash_bytes(b: &[u8]) -> Hash {
    use std::hash::Hasher;
    let mut h0 = twox_hash::XxHash64::with_seed(0);
    let mut h1 = twox_hash::XxHash64::with_seed(0x9E37_79B9_7F4A_7C15);
    h0.write(b);
    h1.write(b);
    format!("{:016x}{:016x}", h0.finish(), h1.finish())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MsgRef { pub role: String, pub hash: Hash }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub hash: Hash,
    pub kind: String,
    pub fragments: usize,
    /// Blob hash of the message that generated this file — its full text IS the reasoning.
    pub produced_by: Hash,
    pub turn: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InputRef { pub path: String, pub size: u64, pub hash: Hash, pub mtime: u64, pub present: bool }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeRecord {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub label: String,
    pub created: u64,
    pub cum_len: usize,          // cumulative message count up to & including this node
    pub added: Vec<MsgRef>,      // messages added vs parent (delta → prefix sharing)
    pub files: Vec<FileEntry>,   // full derived manifest at this node
    pub inputs: Vec<InputRef>,
    pub note: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Index {
    head: Option<NodeId>,
    next_id: NodeId,
    roots: Vec<NodeId>,
    branches: BTreeMap<String, NodeId>,
}

pub struct Vault {
    root: PathBuf,
    index: Index,
    inline_max: u64,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        let _ = f.sync_all();
    }
    fs::rename(&tmp, path)
}

// Some methods here are deliberate forward API for Phases 2–3 (parallel branches, KV warm-start, git
// export) and for tests; they aren't all wired into the UI yet.
#[allow(dead_code)]
impl Vault {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Vault> {
        let root = root.into();
        fs::create_dir_all(root.join("blobs"))?;
        fs::create_dir_all(root.join("nodes"))?;
        let index = match fs::read(root.join("index.json")) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => Index::default(),
        };
        Ok(Vault { root, index, inline_max: INLINE_MAX })
    }

    /// Override the inline copy threshold for inputs (mainly for tests / tuning).
    pub fn with_inline_max(mut self, n: u64) -> Self { self.inline_max = n; self }

    fn blob_path(&self, h: &Hash) -> PathBuf { self.root.join("blobs").join(&h[..2]).join(h) }

    pub fn put_blob(&self, bytes: &[u8]) -> io::Result<Hash> {
        let h = hash_bytes(bytes);
        let p = self.blob_path(&h);
        if !p.exists() {
            if let Some(d) = p.parent() { fs::create_dir_all(d)?; }
            write_atomic(&p, bytes)?;
        }
        Ok(h)
    }
    pub fn has_blob(&self, h: &Hash) -> bool { self.blob_path(h).exists() }
    pub fn get_blob(&self, h: &Hash) -> io::Result<Vec<u8>> { fs::read(self.blob_path(h)) }
    pub fn get_text(&self, h: &Hash) -> String {
        self.get_blob(h).ok().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    }

    fn save_index(&self) -> io::Result<()> {
        write_atomic(&self.root.join("index.json"), &serde_json::to_vec(&self.index)?)
    }
    fn save_node(&self, n: &NodeRecord) -> io::Result<()> {
        write_atomic(&self.root.join("nodes").join(format!("{}.json", n.id)), &serde_json::to_vec(n)?)
    }
    pub fn node(&self, id: NodeId) -> io::Result<NodeRecord> {
        let b = fs::read(self.root.join("nodes").join(format!("{id}.json")))?;
        serde_json::from_slice(&b).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn head(&self) -> Option<NodeId> { self.index.head }
    pub fn set_head(&mut self, id: NodeId) -> io::Result<()> { self.index.head = Some(id); self.save_index() }
    pub fn roots(&self) -> Vec<NodeId> { self.index.roots.clone() }
    pub fn branches(&self) -> BTreeMap<String, NodeId> { self.index.branches.clone() }
    pub fn set_branch(&mut self, label: &str, id: NodeId) -> io::Result<()> {
        self.index.branches.insert(label.into(), id);
        self.save_index()
    }
    pub fn children(&self, id: NodeId) -> Vec<NodeId> { self.node(id).map(|n| n.children).unwrap_or_default() }

    /// All node ids on disk (for a tree view). Cheap for the scale we expect (hundreds).
    pub fn all_ids(&self) -> Vec<NodeId> {
        let mut v = Vec::new();
        if let Ok(rd) = fs::read_dir(self.root.join("nodes")) {
            for e in rd.flatten() {
                if let Some(stem) = e.path().file_stem().and_then(|s| s.to_str()) {
                    if let Ok(id) = stem.parse::<NodeId>() { v.push(id); }
                }
            }
        }
        v.sort_unstable();
        v
    }

    /// Hash a (possibly huge) file in chunks — never loaded whole. Returns (hash, size).
    pub fn hash_file(path: &Path) -> io::Result<(Hash, u64)> {
        use std::hash::Hasher;
        let mut f = fs::File::open(path)?;
        let mut h0 = twox_hash::XxHash64::with_seed(0);
        let mut h1 = twox_hash::XxHash64::with_seed(0x9E37_79B9_7F4A_7C15);
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 { break; }
            h0.write(&buf[..n]);
            h1.write(&buf[..n]);
            size += n as u64;
        }
        Ok((format!("{:016x}{:016x}", h0.finish(), h1.finish()), size))
    }

    /// Record an external input by reference. Copies bytes into the store only if it is at or under the
    /// inline threshold; large inputs (logs/attachments) are referenced, never copied.
    fn input_ref(&self, path: &Path) -> io::Result<InputRef> {
        let (hash, size) = Self::hash_file(path)?;
        let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()).unwrap_or(0);
        if size <= self.inline_max {
            if let Ok(bytes) = fs::read(path) { let _ = self.put_blob(&bytes); }
        }
        Ok(InputRef { path: path.to_string_lossy().into_owned(), size, hash, mtime, present: true })
    }

    /// Commit the current conversation + derived files as a new node (child of `parent`, default = head).
    /// `produced_turn` is the message index whose reasoning produced `files` (normally the last assistant
    /// turn). Returns the new node id (which also becomes head).
    pub fn commit(&mut self, parent: Option<NodeId>, label: &str, msgs: &[(String, String)],
                  files: &[Artifact], produced_turn: usize, inputs: &[PathBuf], note: &str) -> io::Result<NodeId> {
        let parent = parent.or(self.index.head);
        let parent_cum = match parent { Some(p) => self.node(p)?.cum_len, None => 0 };

        let mut added = Vec::new();
        for (role, text) in msgs.iter().skip(parent_cum) {
            let hash = self.put_blob(text.as_bytes())?;
            added.push(MsgRef { role: role.clone(), hash });
        }

        // provenance: hash of the producing message — its text is the reasoning behind the files
        let produced_by = match msgs.get(produced_turn) {
            Some((_, t)) => self.put_blob(t.as_bytes())?,
            None => String::new(),
        };

        let mut fents = Vec::new();
        for a in files {
            let hash = self.put_blob(a.content.as_bytes())?;
            let kind = match a.kind { Kind::File => "file", Kind::Patch => "patch" }.to_string();
            fents.push(FileEntry { path: a.path.clone(), hash, kind, fragments: a.fragments,
                                   produced_by: produced_by.clone(), turn: produced_turn });
        }

        let mut irefs = Vec::new();
        for p in inputs { if let Ok(r) = self.input_ref(p) { irefs.push(r); } }

        let id = self.index.next_id;
        let node = NodeRecord {
            id, parent, children: vec![], label: label.into(), created: now_unix(),
            cum_len: msgs.len(), added, files: fents, inputs: irefs, note: note.into(),
        };
        self.save_node(&node)?;
        match parent {
            Some(p) => { let mut pn = self.node(p)?; pn.children.push(id); self.save_node(&pn)?; }
            None => self.index.roots.push(id),
        }
        self.index.next_id = id + 1;
        self.index.head = Some(id);
        self.save_index()?;
        Ok(id)
    }

    /// Full ordered transcript at a node: walk parent chain to root, concatenating each node's `added`.
    pub fn transcript(&self, id: NodeId) -> io::Result<Vec<(String, String)>> {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let n = self.node(c)?;
            cur = n.parent;
            chain.push(n);
        }
        chain.reverse();
        let mut out = Vec::new();
        for n in &chain {
            for m in &n.added { out.push((m.role.clone(), self.get_text(&m.hash))); }
        }
        Ok(out)
    }

    /// Version history of one file along root→id, each linked to the turn/reasoning that changed it
    /// (only where the content actually changed). The provenance chain git can't express.
    pub fn file_history(&self, id: NodeId, path: &str) -> io::Result<Vec<(NodeId, FileEntry)>> {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let n = self.node(c)?;
            cur = n.parent;
            chain.push(n);
        }
        chain.reverse();
        let mut out: Vec<(NodeId, FileEntry)> = Vec::new();
        for n in &chain {
            if let Some(fe) = n.files.iter().find(|f| f.path == path) {
                if out.last().map(|(_, p)| p.hash != fe.hash).unwrap_or(true) {
                    out.push((n.id, fe.clone()));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);
    fn tmp() -> PathBuf {
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("lsvault_{}_{}_{}", std::process::id(), now_unix(), n));
        let _ = fs::remove_dir_all(&p);
        p
    }
    fn file(path: &str, content: &str) -> Artifact {
        Artifact { path: path.into(), content: content.into(), kind: Kind::File, fragments: 1 }
    }
    fn turn(u: &str, a: &str) -> Vec<(String, String)> {
        vec![("user".into(), u.into()), ("assistant".into(), a.into())]
    }

    #[test]
    fn blob_dedup_and_roundtrip() {
        let v = Vault::open(tmp()).unwrap();
        let h1 = v.put_blob(b"hello").unwrap();
        let h2 = v.put_blob(b"hello").unwrap();
        assert_eq!(h1, h2, "same content -> same id");
        assert_ne!(h1, v.put_blob(b"world").unwrap());
        assert_eq!(v.get_blob(&h1).unwrap(), b"hello");
    }

    #[test]
    fn commit_chain_and_transcript_delta() {
        let mut v = Vault::open(tmp()).unwrap();
        let id0 = v.commit(None, "root", &turn("hi", "hello"), &[], 1, &[], "").unwrap();
        let mut four = turn("hi", "hello");
        four.extend(turn("more", "ok"));
        let id1 = v.commit(Some(id0), "t2", &four, &[], 3, &[], "").unwrap();
        assert_eq!(v.node(id1).unwrap().added.len(), 2, "only the delta is added");
        assert_eq!(v.transcript(id1).unwrap().len(), 4);
        assert_eq!(v.transcript(id0).unwrap(), turn("hi", "hello"));
    }

    #[test]
    fn fork_shares_prefix_blobs() {
        let mut v = Vault::open(tmp()).unwrap();
        let id0 = v.commit(None, "root", &turn("hi", "hello"), &[], 1, &[], "").unwrap();
        let mut a = turn("hi", "hello"); a.extend(turn("path A", "A"));
        let mut b = turn("hi", "hello"); b.extend(turn("path B", "B"));
        let ida = v.commit(Some(id0), "A", &a, &[], 3, &[], "").unwrap();
        let idb = v.commit(Some(id0), "B", &b, &[], 3, &[], "").unwrap();
        let kids = v.node(id0).unwrap().children;
        assert!(kids.contains(&ida) && kids.contains(&idb), "both forks are children of the shared node");
        // shared prefix message ("hi"/"hello") is one blob, reused by both branches
        let ta = v.transcript(ida).unwrap();
        let tb = v.transcript(idb).unwrap();
        assert_eq!(&ta[..2], &tb[..2]);
        assert_eq!(ta[3].1, "A");
        assert_eq!(tb[3].1, "B");
    }

    #[test]
    fn file_history_links_versions_to_reasoning() {
        let mut v = Vault::open(tmp()).unwrap();
        // turn 1 makes main.rs v1; the assistant message "wrote v1 because X" IS the reasoning
        let id0 = v.commit(None, "v1", &turn("make it", "wrote v1 because X"),
                           &[file("main.rs", "fn main(){}//v1")], 1, &[], "").unwrap();
        // turn 2 rewrites main.rs; reasoning is the new assistant message
        let mut t2 = turn("make it", "wrote v1 because X");
        t2.extend(turn("change it", "rewrote because Y"));
        let id1 = v.commit(Some(id0), "v2", &t2, &[file("main.rs", "fn main(){ /*v2*/ }")], 3, &[], "").unwrap();
        // turn 3 leaves main.rs unchanged
        let mut t3 = t2.clone();
        t3.extend(turn("docs?", "added docs, file unchanged"));
        let id2 = v.commit(Some(id1), "v2b", &t3, &[file("main.rs", "fn main(){ /*v2*/ }")], 5, &[], "").unwrap();

        let hist = v.file_history(id2, "main.rs").unwrap();
        assert_eq!(hist.len(), 2, "unchanged version is not duplicated");
        assert_eq!(hist[0].0, id0);
        assert_eq!(hist[1].0, id1);
        // the provenance link: each version resolves to the turn that reasoned about it
        assert_eq!(v.get_text(&hist[0].1.produced_by), "wrote v1 because X");
        assert_eq!(v.get_text(&hist[1].1.produced_by), "rewrote because Y");
    }

    #[test]
    fn large_input_is_referenced_not_copied() {
        let dir = tmp();
        let big = dir.join("big.log");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&big, vec![b'x'; 64]).unwrap(); // 64 bytes
        let mut v = Vault::open(dir.join("vault")).unwrap().with_inline_max(8); // threshold below file size
        let id = v.commit(None, "with input", &turn("see log", "ok"), &[], 1, &[big.clone()], "").unwrap();
        let n = v.node(id).unwrap();
        assert_eq!(n.inputs.len(), 1);
        assert_eq!(n.inputs[0].size, 64);
        assert!(!v.has_blob(&n.inputs[0].hash), "input over threshold must NOT be copied into the store");
    }

    #[test]
    fn streaming_hash_matches_in_memory() {
        let dir = tmp();
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        let data = vec![7u8; 3 * (1 << 20) + 13]; // > one chunk
        fs::write(&p, &data).unwrap();
        let (h, sz) = Vault::hash_file(&p).unwrap();
        assert_eq!(sz, data.len() as u64);
        assert_eq!(h, hash_bytes(&data), "chunked file hash == in-memory hash");
    }

    #[test]
    fn reopen_persists_head_and_tree() {
        let dir = tmp();
        let id1;
        {
            let mut v = Vault::open(&dir).unwrap();
            let id0 = v.commit(None, "root", &turn("hi", "hello"), &[], 1, &[], "").unwrap();
            let mut t = turn("hi", "hello"); t.extend(turn("next", "again"));
            id1 = v.commit(Some(id0), "t2", &t, &[], 3, &[], "").unwrap();
        }
        let v = Vault::open(&dir).unwrap();
        assert_eq!(v.head(), Some(id1));
        assert_eq!(v.transcript(id1).unwrap().len(), 4);
    }
}
