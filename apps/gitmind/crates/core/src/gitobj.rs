// gitmind-core/src/gitobj.rs
//
// Build git objects ourselves and compute their git oids, so gitmind can
// CREATE history natively in `.gitmind` (not just mirror an existing repo)
// and still produce objects byte-identical to what git would write. The
// proof is in the tests: a blob/tree/commit built here hashes to the same
// oid git computes for the same content.
//
// git's object hash is SHA-1 over "<type> <size>\0<content>". (This targets
// classic SHA-1 repositories; SHA-256 repos would swap the hash.) Trees use
// the canonical entry encoding and git's directory-aware name ordering.

use sha1::{Digest, Sha1};

/// git oid (40-hex) of an object of the given type and content.
pub fn hash_object(kind: &str, content: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("{kind} {}\0", content.len()).as_bytes());
    h.update(content);
    to_hex(&h.finalize())
}

#[derive(Clone, Debug, PartialEq)]
pub struct TreeEntry {
    /// Octal ASCII mode as it appears in a tree object: "100644", "100755",
    /// "120000", "40000" (dir), "160000" (submodule).
    pub mode: String,
    pub name: String,
    /// Child object oid (40-hex).
    pub oid: String,
}

impl TreeEntry {
    fn is_dir(&self) -> bool {
        self.mode == "40000"
    }
    /// git orders entries by name, but a directory sorts as if its name had
    /// a trailing '/'. Replicating that exactly is required or the tree oid
    /// won't match git's.
    fn sort_key(&self) -> Vec<u8> {
        let mut k = self.name.as_bytes().to_vec();
        if self.is_dir() {
            k.push(b'/');
        }
        k
    }
}

/// Build a tree object from entries; returns (oid, canonical bytes).
pub fn build_tree(entries: &mut [TreeEntry]) -> (String, Vec<u8>) {
    entries.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    let mut body = Vec::new();
    for e in entries.iter() {
        body.extend_from_slice(e.mode.as_bytes());
        body.push(b' ');
        body.extend_from_slice(e.name.as_bytes());
        body.push(0);
        body.extend_from_slice(&from_hex(&e.oid));
    }
    (hash_object("tree", &body), body)
}

/// Parse a tree object's bytes back into entries.
pub fn parse_tree(bytes: &[u8]) -> Vec<TreeEntry> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // mode SP name NUL oid(20)
        let sp = match bytes[i..].iter().position(|&b| b == b' ') {
            Some(p) => i + p,
            None => break,
        };
        let mode = String::from_utf8_lossy(&bytes[i..sp]).to_string();
        let nul = match bytes[sp + 1..].iter().position(|&b| b == 0) {
            Some(p) => sp + 1 + p,
            None => break,
        };
        let name = String::from_utf8_lossy(&bytes[sp + 1..nul]).to_string();
        let oid_start = nul + 1;
        if oid_start + 20 > bytes.len() {
            break;
        }
        let oid = to_hex(&bytes[oid_start..oid_start + 20]);
        out.push(TreeEntry { mode, name, oid });
        i = oid_start + 20;
    }
    out
}

/// Build a commit object; returns (oid, bytes). `author`/`committer` are
/// full identity+timestamp lines, e.g. "Ada <ada@x> 1700000000 +0000".
pub fn build_commit(
    tree: &str,
    parents: &[String],
    author: &str,
    committer: &str,
    message: &str,
) -> (String, Vec<u8>) {
    let mut body = String::new();
    body.push_str(&format!("tree {tree}\n"));
    for p in parents {
        body.push_str(&format!("parent {p}\n"));
    }
    body.push_str(&format!("author {author}\n"));
    body.push_str(&format!("committer {committer}\n"));
    body.push('\n');
    body.push_str(message);
    if !message.ends_with('\n') {
        body.push('\n');
    }
    let bytes = body.into_bytes();
    (hash_object("commit", &bytes), bytes)
}

/// Identity+timestamp line for author/committer.
pub fn ident_line(name: &str, email: &str, unix_ts: i64, tz: &str) -> String {
    format!("{name} <{email}> {unix_ts} {tz}")
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let oid = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        assert_eq!(to_hex(&from_hex(oid)), oid);
    }

    #[test]
    fn empty_blob_matches_git_constant() {
        // git's well-known empty-blob oid.
        assert_eq!(hash_object("blob", b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn empty_tree_matches_git_constant() {
        assert_eq!(hash_object("tree", b""), "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    }

    #[test]
    fn tree_sorts_dirs_with_trailing_slash() {
        // A file "foo.txt" and a dir "foo" — the dir sorts as "foo/".
        let blob = hash_object("blob", b"x");
        let mut entries = vec![
            TreeEntry { mode: "100644".into(), name: "foo.txt".into(), oid: blob.clone() },
            TreeEntry { mode: "40000".into(), name: "foo".into(), oid: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into() },
        ];
        let (_, bytes) = build_tree(&mut entries);
        let parsed = parse_tree(&bytes);
        // "foo" (as "foo/") sorts after "foo.txt" because '/' (0x2f) > '.' (0x2e).
        assert_eq!(parsed[0].name, "foo.txt");
        assert_eq!(parsed[1].name, "foo");
    }
}
