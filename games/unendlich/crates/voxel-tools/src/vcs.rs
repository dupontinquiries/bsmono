//! A git tree over the world.
//!
//! WHY THIS IS CHEAP HERE
//!
//! The world is generated from a seed. Everything a player builds is a
//! DELTA from that generation, and the deltas are sparse: a village is
//! a few thousand cells in a world of billions. So a "blob" is not a
//! chunk, it is the set of edited cells in a chunk -- tiny, and
//! content-addressed, so two branches that made the same change share
//! the object. A "tree" maps chunk coordinates to blob hashes. A
//! "commit" is a tree plus its parents plus THE IR THAT PRODUCED IT.
//!
//! That last part is what makes rebase interesting. In git a rebase
//! replays patches, and a patch is bytes: move the base and the bytes
//! land in the wrong place. Here a commit's payload is its IR, and the
//! IR is relative to the ground. Rebase a tower onto raised terrain and
//! the tower is REBUILT on the new surface, because `cylinder @0,0`
//! finds the surface again. The plan is small; replaying it is cheap;
//! and the result is what the player meant, not where the bytes were.
//!
//! MERGE
//!
//! Two players on two branches. Their trees differ only in the chunks
//! they touched, so a merge is a walk over the union of touched chunks
//! -- not the world -- and a chunk touched by only one side is taken
//! whole. A chunk touched by both gets a three-way merge at the CELL
//! level: a cell changed by one side takes that side, changed by both
//! to the same value is fine, changed by both to different values is a
//! conflict. Conflicts are resolved by a stated policy and RECORDED, so
//! nothing is silently lost.
//!
//! NOTHING HERE TOUCHES THE WORLD
//!
//! `checkout` returns the changes that would take the world from one
//! tree to another; the hive applies them within the frame budget. The
//! repository is data, saved beside the worksets in the same
//! dependency-free line format.

use crate::weather::Change;
use std::collections::{BTreeMap, BTreeSet};

/// A content hash. 64 bits of FNV-1a over the canonical cell list:
/// enough to dedupe within a game, not meant to survive an adversary.
pub type Hash = u64;

fn fnv(bytes: impl Iterator<Item = u8>) -> Hash {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes { h ^= b as u64; h = h.wrapping_mul(0x0000_0100_0000_01b3); }
    h
}

/// The edited cells of one chunk, in LOCAL coordinates, sorted. The
/// generated base is implicit: a cell absent here is "as generated".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blob {
    pub cells: BTreeMap<(u8, u8, u8), u16>,
}

impl Blob {
    pub fn hash(&self) -> Hash {
        fnv(self.cells.iter().flat_map(|((x, y, z), b)| {
            [*x, *y, *z, (*b & 0xff) as u8, (*b >> 8) as u8]
        }))
    }
    pub fn is_empty(&self) -> bool { self.cells.is_empty() }
}

/// Chunk coordinate -> blob hash. Only chunks with edits appear.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub chunks: BTreeMap<(i32, i32, i32), Hash>,
}

impl Tree {
    pub fn hash(&self) -> Hash {
        fnv(self.chunks.iter().flat_map(|((x, y, z), h)| {
            x.to_le_bytes().into_iter().chain(y.to_le_bytes()).chain(z.to_le_bytes())
                .chain(h.to_le_bytes())
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: Hash,
    pub tree: Hash,
    pub parents: Vec<Hash>,
    pub author: String,
    pub message: String,
    /// The IR that produced this commit's changes over its first
    /// parent. Empty for a merge or a snapshot. This is what a rebase
    /// replays.
    pub ir: String,
    /// The x,z the IR's `@` coordinates were relative to. The Y is
    /// deliberately NOT stored: a rebase re-derives it from the ground
    /// it lands on. Replaying with the old y searches for the surface
    /// from the old height, finds it inside a raised hill, and buries
    /// the build -- which is exactly the failure a rebase exists to
    /// avoid.
    pub anchor: (i32, i32),
}

/// How a cell-level conflict is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Ours,
    Theirs,
    /// Air wins: a carve on either side survives, since a doorway lost
    /// to a wall is worse than a wall lost to a doorway.
    AirWins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub at: (i32, i32, i32),
    pub base: Option<u16>,
    pub ours: u16,
    pub theirs: u16,
    pub took: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeReport {
    /// Chunks taken whole from one side.
    pub fast_forwarded: usize,
    /// Chunks merged cell by cell.
    pub merged: usize,
    pub conflicts: Vec<Conflict>,
}

impl MergeReport {
    pub fn summary(&self) -> String {
        // usize::MAX marks a fast-forward; it printed as a twenty-digit
        // chunk count before this branch existed.
        if self.fast_forwarded == usize::MAX { return "fast-forward: nothing to reconcile".into(); }
        format!("{} chunk(s) taken whole, {} merged cell by cell, {} conflict(s){}",
                self.fast_forwarded, self.merged, self.conflicts.len(),
                if self.conflicts.is_empty() { String::new() }
                else { " resolved by policy and recorded".into() })
    }
}

/// The object store, the commits, the branches.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Repo {
    blobs: BTreeMap<Hash, Blob>,
    trees: BTreeMap<Hash, Tree>,
    commits: BTreeMap<Hash, Commit>,
    branches: BTreeMap<String, Hash>,
    pub head: String,
    pub chunk_size: i32,
}

impl Repo {
    pub fn new(chunk_size: i32) -> Self {
        let mut r = Self { chunk_size: chunk_size.max(1), head: "main".into(), ..Default::default() };
        // The root: an empty tree, "as generated".
        let root_tree = Tree::default();
        let th = root_tree.hash();
        r.trees.insert(th, root_tree);
        let root = Commit { hash: 0, tree: th, parents: vec![], author: "world".into(),
                            message: "as generated".into(), ir: String::new(), anchor: (0, 0) };
        let root = Commit { hash: commit_hash(&root), ..root };
        r.branches.insert("main".into(), root.hash);
        r.commits.insert(root.hash, root);
        r
    }

    pub fn branch_head(&self, name: &str) -> Option<Hash> { self.branches.get(name).copied() }
    pub fn head_commit(&self) -> &Commit { &self.commits[&self.branches[&self.head]] }
    pub fn commit(&self, h: Hash) -> Option<&Commit> { self.commits.get(&h) }
    pub fn tree(&self, h: Hash) -> Option<&Tree> { self.trees.get(&h) }
    pub fn blob(&self, h: Hash) -> Option<&Blob> { self.blobs.get(&h) }
    pub fn branches(&self) -> impl Iterator<Item = (&String, &Hash)> { self.branches.iter() }
    pub fn object_count(&self) -> usize { self.blobs.len() }

    fn store_tree(&mut self, t: Tree) -> Hash {
        let h = t.hash();
        self.trees.entry(h).or_insert(t);
        h
    }

    /// Apply a change list to the head's tree and commit it. The
    /// changes are sharded into blobs; a blob identical to one already
    /// stored is shared, not duplicated.
    pub fn commit_changes(&mut self, author: &str, message: &str, ir: &str,
                          changes: &[Change]) -> Hash {
        self.commit_changes_at(author, message, ir, (0, 0), changes)
    }

    /// As `commit_changes`, recording the anchor the IR was relative to.
    pub fn commit_changes_at(&mut self, author: &str, message: &str, ir: &str,
                             anchor: (i32, i32), changes: &[Change]) -> Hash {
        let parent = self.branches[&self.head];
        let mut tree = self.trees[&self.commits[&parent].tree].clone();
        let cs = self.chunk_size;
        let mut touched: BTreeMap<(i32, i32, i32), Blob> = BTreeMap::new();
        for c in changes {
            let key = (c.at.0.div_euclid(cs), c.at.1.div_euclid(cs), c.at.2.div_euclid(cs));
            let blob = touched.entry(key).or_insert_with(|| {
                tree.chunks.get(&key).and_then(|h| self.blobs.get(h)).cloned().unwrap_or_default()
            });
            let local = (c.at.0.rem_euclid(cs) as u8, c.at.1.rem_euclid(cs) as u8,
                         c.at.2.rem_euclid(cs) as u8);
            blob.cells.insert(local, c.to);
        }
        for (key, blob) in touched {
            if blob.is_empty() { tree.chunks.remove(&key); continue; }
            let h = blob.hash();
            self.blobs.entry(h).or_insert(blob);
            tree.chunks.insert(key, h);
        }
        let th = self.store_tree(tree);
        let c = Commit { hash: 0, tree: th, parents: vec![parent], author: author.into(),
                         message: message.into(), ir: ir.into(), anchor };
        let c = Commit { hash: commit_hash(&c), ..c };
        let h = c.hash;
        self.commits.insert(h, c);
        self.branches.insert(self.head.clone(), h);
        h
    }

    pub fn branch(&mut self, name: &str) -> Result<(), String> {
        if self.branches.contains_key(name) { return Err(format!("branch '{name}' exists")); }
        let at = self.branches[&self.head];
        self.branches.insert(name.into(), at);
        Ok(())
    }

    pub fn switch(&mut self, name: &str) -> Result<(), String> {
        if !self.branches.contains_key(name) { return Err(format!("no branch '{name}'")); }
        self.head = name.into();
        Ok(())
    }

    /// The changes that take the world from tree `from` to tree `to`.
    /// A cell in `to` but not `from` is written; a cell in `from` but
    /// not `to` is reverted to generation -- which the caller expresses
    /// by writing the generated block, so it is reported as `None`.
    pub fn diff(&self, from: Hash, to: Hash) -> Vec<(Change, bool)> {
        let (a, b) = (&self.trees[&from], &self.trees[&to]);
        let cs = self.chunk_size;
        let mut out = Vec::new();
        let keys: BTreeSet<_> = a.chunks.keys().chain(b.chunks.keys()).copied().collect();
        for key in keys {
            let ah = a.chunks.get(&key);
            let bh = b.chunks.get(&key);
            if ah == bh { continue; }
            let ab = ah.and_then(|h| self.blobs.get(h)).cloned().unwrap_or_default();
            let bb = bh.and_then(|h| self.blobs.get(h)).cloned().unwrap_or_default();
            let cells: BTreeSet<_> = ab.cells.keys().chain(bb.cells.keys()).copied().collect();
            for (lx, ly, lz) in cells {
                let world = (key.0 * cs + lx as i32, key.1 * cs + ly as i32, key.2 * cs + lz as i32);
                match (ab.cells.get(&(lx, ly, lz)), bb.cells.get(&(lx, ly, lz))) {
                    (Some(x), Some(y)) if x == y => {}
                    (_, Some(y)) => out.push((Change { at: world, to: *y }, false)),
                    (Some(_), None) => out.push((Change { at: world, to: 0 }, true)),
                    (None, None) => {}
                }
            }
        }
        out
    }

    /// The nearest common ancestor of two commits.
    pub fn merge_base(&self, a: Hash, b: Hash) -> Option<Hash> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![a];
        while let Some(h) = stack.pop() {
            if seen.insert(h) { stack.extend(self.commits[&h].parents.iter().copied()); }
        }
        // BFS from b: the first hit is the nearest.
        let mut q = std::collections::VecDeque::from([b]);
        let mut visited = BTreeSet::new();
        while let Some(h) = q.pop_front() {
            if !visited.insert(h) { continue; }
            if seen.contains(&h) { return Some(h); }
            q.extend(self.commits[&h].parents.iter().copied());
        }
        None
    }

    /// Three-way merge of `other` into the head branch.
    ///
    /// Walks the UNION OF TOUCHED CHUNKS, not the world. A chunk touched
    /// by one side is taken whole; one touched by both is merged cell
    /// by cell against the merge base.
    pub fn merge(&mut self, other: &str, policy: Policy, author: &str)
        -> Result<(Hash, MergeReport), String> {
        let ours_c = self.branches[&self.head];
        let theirs_c = *self.branches.get(other).ok_or_else(|| format!("no branch '{other}'"))?;
        if ours_c == theirs_c { return Err("already up to date".into()); }
        let base_c = self.merge_base(ours_c, theirs_c).ok_or("no common history")?;
        if base_c == ours_c {
            // Fast-forward: we have nothing they lack.
            self.branches.insert(self.head.clone(), theirs_c);
            return Ok((theirs_c, MergeReport { fast_forwarded: usize::MAX, ..Default::default() }));
        }
        let (base, ours, theirs) = (
            self.trees[&self.commits[&base_c].tree].clone(),
            self.trees[&self.commits[&ours_c].tree].clone(),
            self.trees[&self.commits[&theirs_c].tree].clone());
        let mut report = MergeReport::default();
        let mut result = ours.clone();
        let keys: BTreeSet<_> = ours.chunks.keys().chain(theirs.chunks.keys()).copied().collect();
        let cs = self.chunk_size;
        for key in keys {
            let (bh, oh, th) = (base.chunks.get(&key), ours.chunks.get(&key), theirs.chunks.get(&key));
            if oh == th { continue; }
            if oh == bh {
                // only they changed it: take theirs whole
                match th { Some(h) => { result.chunks.insert(key, *h); }
                           None => { result.chunks.remove(&key); } }
                report.fast_forwarded += 1;
                continue;
            }
            if th == bh { continue; }   // only we changed it: already ours
            // both changed: cell by cell
            let get = |h: Option<&Hash>| h.and_then(|h| self.blobs.get(h)).cloned().unwrap_or_default();
            let (bb, ob, tb) = (get(bh), get(oh), get(th));
            let mut merged = Blob::default();
            let cells: BTreeSet<_> = ob.cells.keys().chain(tb.cells.keys()).copied().collect();
            for c in cells {
                let (b, o, t) = (bb.cells.get(&c).copied(), ob.cells.get(&c).copied(),
                                 tb.cells.get(&c).copied());
                let took = match (o, t) {
                    (Some(o), Some(t)) if o == t => o,
                    (Some(o), None) => if b == Some(o) { continue } else { o },
                    (None, Some(t)) => if b == Some(t) { continue } else { t },
                    (Some(o), Some(t)) => {
                        if b == Some(o) { t } else if b == Some(t) { o } else {
                            let took = match policy {
                                Policy::Ours => o,
                                Policy::Theirs => t,
                                Policy::AirWins => if o == 0 || t == 0 { 0 } else { t },
                            };
                            report.conflicts.push(Conflict {
                                at: (key.0 * cs + c.0 as i32, key.1 * cs + c.1 as i32,
                                     key.2 * cs + c.2 as i32),
                                base: b, ours: o, theirs: t, took });
                            took
                        }
                    }
                    (None, None) => continue,
                };
                merged.cells.insert(c, took);
            }
            report.merged += 1;
            if merged.is_empty() { result.chunks.remove(&key); }
            else {
                let h = merged.hash();
                self.blobs.entry(h).or_insert(merged);
                result.chunks.insert(key, h);
            }
        }
        let th = self.store_tree(result);
        let c = Commit { hash: 0, tree: th, parents: vec![ours_c, theirs_c], author: author.into(),
                         message: format!("merge {other} into {}", self.head), ir: String::new(),
                         anchor: (0, 0) };
        let c = Commit { hash: commit_hash(&c), ..c };
        let h = c.hash;
        self.commits.insert(h, c);
        self.branches.insert(self.head.clone(), h);
        Ok((h, report))
    }

    /// The commits on `branch` since it diverged from `onto`, oldest
    /// first, with their IR. This is the rebase plan: the caller replays
    /// each IR against the world at `onto` and commits the result. The
    /// plans are small; replaying them is cheap; and because the IR is
    /// relative to the ground, a tower rebased onto raised terrain is
    /// rebuilt ON the raised terrain.
    pub fn rebase_plan(&self, branch: &str, onto: &str) -> Result<Vec<Commit>, String> {
        let b = *self.branches.get(branch).ok_or_else(|| format!("no branch '{branch}'"))?;
        let o = *self.branches.get(onto).ok_or_else(|| format!("no branch '{onto}'"))?;
        let base = self.merge_base(b, o).ok_or("no common history")?;
        let mut out = Vec::new();
        let mut h = b;
        while h != base {
            let c = &self.commits[&h];
            if c.parents.len() != 1 {
                return Err("cannot rebase across a merge commit; merge instead".into());
            }
            if c.ir.trim().is_empty() {
                return Err(format!("commit {} has no IR to replay", short(c.hash)));
            }
            out.push(c.clone());
            h = c.parents[0];
        }
        out.reverse();
        Ok(out)
    }

    /// Move a branch to a commit. Used after the caller has replayed a
    /// rebase plan and committed the results on `onto`.
    pub fn reset_branch(&mut self, name: &str, to: Hash) -> Result<(), String> {
        if !self.commits.contains_key(&to) { return Err("no such commit".into()); }
        self.branches.insert(name.into(), to);
        Ok(())
    }

    pub fn log(&self, branch: &str, n: usize) -> Vec<&Commit> {
        let mut out = Vec::new();
        let Some(mut h) = self.branches.get(branch).copied() else { return out };
        while out.len() < n {
            let c = &self.commits[&h];
            out.push(c);
            match c.parents.first() { Some(p) => h = *p, None => break }
        }
        out
    }

    // ── persistence: the same line format as land.rs ──

    pub fn save(&self) -> String {
        let mut s = format!("# unendlich worldtree v1\nchunk {}\nhead {}\n", self.chunk_size, esc(&self.head));
        // Branch names are ESCAPED like every other free string: a
        // player names a branch "friend's mill", and splitting the line
        // on its first space lost every branch with a space in it.
        for (name, h) in &self.branches { s.push_str(&format!("branch {} {h:x}\n", esc(name))); }
        for (h, b) in &self.blobs {
            s.push_str(&format!("blob {h:x}"));
            for ((x, y, z), v) in &b.cells { s.push_str(&format!(" {x},{y},{z}:{v}")); }
            s.push('\n');
        }
        for (h, t) in &self.trees {
            s.push_str(&format!("tree {h:x}"));
            for ((x, y, z), bh) in &t.chunks { s.push_str(&format!(" {x},{y},{z}:{bh:x}")); }
            s.push('\n');
        }
        for (h, c) in &self.commits {
            s.push_str(&format!("commit {h:x} {:x} {} {} {}\n", c.tree,
                                c.parents.iter().map(|p| format!("{p:x}")).collect::<Vec<_>>().join(","),
                                esc(&c.author), esc(&c.message)));
            s.push_str(&format!("anchor {} {}\n", c.anchor.0, c.anchor.1));
            for line in c.ir.lines() { s.push_str(&format!("ir {}\n", esc(line))); }
        }
        s
    }

    pub fn load(text: &str) -> Self {
        let r = Self::load_inner(text);
        // A file that is empty, truncated, or from before the tree had
        // a root has no `main`. Every other method indexes the head
        // branch and would panic; a fresh repo is the honest result.
        if r.branches.get(&r.head).map(|h| r.commits.contains_key(h)).unwrap_or(false) { r }
        else { Self::new(r.chunk_size) }
    }

    fn load_inner(text: &str) -> Self {
        let mut r = Repo { chunk_size: 32, head: "main".into(), ..Default::default() };
        let mut last_commit: Option<Hash> = None;
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() { continue; }
            let (k, rest) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "chunk" => r.chunk_size = rest.parse().unwrap_or(32),
                "head" => r.head = unesc(rest),
                "branch" => if let Some((n, h)) = rest.rsplit_once(' ') {
                    if let Ok(h) = u64::from_str_radix(h, 16) { r.branches.insert(unesc(n), h); }
                },
                "blob" => {
                    let mut it = rest.split_whitespace();
                    let Some(h) = it.next().and_then(|h| u64::from_str_radix(h, 16).ok()) else { continue };
                    let mut b = Blob::default();
                    for cell in it {
                        if let Some((pos, v)) = cell.split_once(':') {
                            let p: Vec<u8> = pos.split(',').filter_map(|n| n.parse().ok()).collect();
                            if p.len() == 3 { if let Ok(v) = v.parse() { b.cells.insert((p[0], p[1], p[2]), v); } }
                        }
                    }
                    r.blobs.insert(h, b);
                }
                "tree" => {
                    let mut it = rest.split_whitespace();
                    let Some(h) = it.next().and_then(|h| u64::from_str_radix(h, 16).ok()) else { continue };
                    let mut t = Tree::default();
                    for ch in it {
                        if let Some((pos, bh)) = ch.split_once(':') {
                            let p: Vec<i32> = pos.split(',').filter_map(|n| n.parse().ok()).collect();
                            if p.len() == 3 { if let Ok(bh) = u64::from_str_radix(bh, 16) {
                                t.chunks.insert((p[0], p[1], p[2]), bh); } }
                        }
                    }
                    r.trees.insert(h, t);
                }
                "commit" => {
                    let mut it = rest.splitn(5, ' ');
                    let h = it.next().and_then(|h| u64::from_str_radix(h, 16).ok());
                    let t = it.next().and_then(|h| u64::from_str_radix(h, 16).ok());
                    let parents: Vec<Hash> = it.next().unwrap_or("").split(',')
                        .filter_map(|p| u64::from_str_radix(p, 16).ok()).collect();
                    let author = unesc(it.next().unwrap_or(""));
                    let message = unesc(it.next().unwrap_or(""));
                    if let (Some(h), Some(t)) = (h, t) {
                        r.commits.insert(h, Commit { hash: h, tree: t, parents, author, message,
                                                     ir: String::new(), anchor: (0, 0) });
                        last_commit = Some(h);
                    }
                }
                "anchor" => if let Some(h) = last_commit {
                    let n: Vec<i32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                    if n.len() == 2 { if let Some(c) = r.commits.get_mut(&h) { c.anchor = (n[0], n[1]); } }
                },
                "ir" => if let Some(h) = last_commit {
                    if let Some(c) = r.commits.get_mut(&h) { c.ir.push_str(&unesc(rest)); c.ir.push('\n'); }
                },
                _ => {}
            }
        }
        r
    }
}

fn commit_hash(c: &Commit) -> Hash {
    fnv(c.tree.to_le_bytes().into_iter()
        .chain(c.anchor.0.to_le_bytes()).chain(c.anchor.1.to_le_bytes())
        .chain(c.parents.iter().flat_map(|p| p.to_le_bytes()))
        .chain(c.author.bytes()).chain([0]).chain(c.message.bytes()).chain([0])
        .chain(c.ir.bytes()))
}

pub fn short(h: Hash) -> String { format!("{:x}", h)[..8.min(format!("{:x}", h).len())].to_string() }

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace(' ', "\\s").replace('\n', "\\n") }
fn unesc(s: &str) -> String {
    let mut out = String::new(); let mut e = false;
    for c in s.chars() {
        if e { out.push(match c { 's' => ' ', 'n' => '\n', o => o }); e = false; }
        else if c == '\\' { e = true } else { out.push(c) }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(x: i32, y: i32, z: i32, b: u16) -> Change { Change { at: (x, y, z), to: b } }

    #[test]
    fn identical_edits_on_two_branches_share_one_blob() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "wall", "fill ...", &[ch(1, 65, 1, 1), ch(2, 65, 1, 1)]);
        r.branch("b").unwrap(); r.switch("b").unwrap();
        // b redoes the same thing on top of main: same blob content
        let before = r.object_count();
        r.commit_changes("b", "same wall", "fill ...", &[ch(1, 65, 1, 1), ch(2, 65, 1, 1)]);
        assert_eq!(r.object_count(), before, "content-addressed: no new object");
    }

    #[test]
    fn diff_is_the_changes_between_two_trees_and_nothing_else() {
        let mut r = Repo::new(32);
        let c1 = r.commit_changes("a", "one", "", &[ch(1, 65, 1, 1), ch(2, 65, 1, 1)]);
        let c2 = r.commit_changes("a", "two", "", &[ch(2, 65, 1, 4), ch(3, 65, 1, 1)]);
        let d = r.diff(r.commit(c1).unwrap().tree, r.commit(c2).unwrap().tree);
        let mut got: Vec<_> = d.iter().map(|(c, _)| (c.at, c.to)).collect();
        got.sort();
        assert_eq!(got, vec![((2, 65, 1), 4), ((3, 65, 1), 1)], "only what changed");
        // and backwards, the second cell reverts to generation
        let back = r.diff(r.commit(c2).unwrap().tree, r.commit(c1).unwrap().tree);
        assert!(back.iter().any(|(c, revert)| c.at == (3, 65, 1) && *revert));
    }

    #[test]
    fn disjoint_chunks_merge_by_taking_each_side_whole() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "base", "", &[ch(1, 65, 1, 1)]);
        r.branch("mill").unwrap();
        r.commit_changes("a", "tower on main", "", &[ch(5, 66, 5, 58)]);        // chunk (0,2,0)
        r.switch("mill").unwrap();
        r.commit_changes("b", "mill far away", "", &[ch(200, 66, 200, 17)]);    // chunk (6,2,6)
        r.switch("main").unwrap();
        let (_, rep) = r.merge("mill", Policy::Theirs, "a").unwrap();
        assert_eq!(rep.fast_forwarded, 1, "the mill's chunk taken whole: {}", rep.summary());
        assert_eq!(rep.merged, 0);
        assert!(rep.conflicts.is_empty());
        let t = r.tree(r.head_commit().tree).unwrap();
        assert!(t.chunks.contains_key(&(0, 2, 0)) && t.chunks.contains_key(&(6, 2, 6)),
                "both builds present");
    }

    #[test]
    fn a_shared_chunk_merges_cell_by_cell_without_conflict_when_cells_differ() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "base", "", &[ch(1, 65, 1, 1)]);
        r.branch("b").unwrap();
        r.commit_changes("a", "ours", "", &[ch(5, 66, 5, 58)]);
        r.switch("b").unwrap();
        r.commit_changes("b", "theirs", "", &[ch(9, 66, 9, 17)]);   // same chunk, different cell
        r.switch("main").unwrap();
        let (h, rep) = r.merge("b", Policy::Ours, "a").unwrap();
        assert_eq!(rep.merged, 1);
        assert!(rep.conflicts.is_empty(), "{:?}", rep.conflicts);
        let t = r.tree(r.commit(h).unwrap().tree).unwrap();
        let blob = r.blob(t.chunks[&(0, 2, 0)]).unwrap();
        assert_eq!(blob.cells.get(&(5, 2, 5)), Some(&58), "ours kept");
        assert_eq!(blob.cells.get(&(9, 2, 9)), Some(&17), "theirs kept");
        assert_eq!(blob.cells.get(&(1, 1, 1)), Some(&1), "base kept");
    }

    #[test]
    fn a_real_conflict_is_resolved_by_policy_and_recorded() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "base", "", &[ch(5, 66, 5, 1)]);
        r.branch("b").unwrap();
        r.commit_changes("a", "ours: brick", "", &[ch(5, 66, 5, 58)]);
        r.switch("b").unwrap();
        r.commit_changes("b", "theirs: carve", "", &[ch(5, 66, 5, 0)]);
        r.switch("main").unwrap();
        let (_, rep) = r.merge("b", Policy::AirWins, "a").unwrap();
        assert_eq!(rep.conflicts.len(), 1);
        let c = &rep.conflicts[0];
        assert_eq!((c.base, c.ours, c.theirs, c.took), (Some(1), 58, 0, 0),
                   "air wins: a doorway lost to a wall is worse than the reverse");
        assert!(rep.summary().contains("1 conflict"));
    }

    #[test]
    fn fast_forward_when_we_have_nothing_they_lack() {
        let mut r = Repo::new(32);
        r.branch("b").unwrap(); r.switch("b").unwrap();
        let h = r.commit_changes("b", "work", "", &[ch(1, 65, 1, 1)]);
        r.switch("main").unwrap();
        let (got, rep) = r.merge("b", Policy::Ours, "a").unwrap();
        assert_eq!(got, h, "main now points at b's commit");
        assert_eq!(rep.fast_forwarded, usize::MAX);
        assert!(rep.summary().starts_with("fast-forward"), "no sentinel in the text: {}", rep.summary());
    }

    #[test]
    fn a_rebase_plan_is_the_branchs_ir_oldest_first() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "base", "", &[ch(1, 65, 1, 1)]);
        r.branch("tower").unwrap();
        r.commit_changes("a", "raise the ground", "brush raise @0,0 by=10 r=30\n", &[ch(0, 70, 0, 2)]);
        r.switch("tower").unwrap();
        r.commit_changes("b", "tower", "cylinder @0,0 r=3 h=9\n", &[ch(0, 66, 0, 1)]);
        r.commit_changes("b", "door", "sphere @3,0 r=1 block=air\n", &[ch(3, 66, 0, 0)]);
        let plan = r.rebase_plan("tower", "main").unwrap();
        assert_eq!(plan.len(), 2);
        assert!(plan[0].ir.starts_with("cylinder"), "oldest first");
        assert!(plan[1].ir.starts_with("sphere"));
        // the point: the IR is what gets replayed, on the NEW ground
        assert!(!plan.iter().any(|c| c.message.contains("raise")), "main's own commits are not replayed");
    }

    #[test]
    fn rebasing_across_a_merge_is_refused_with_a_reason() {
        let mut r = Repo::new(32);
        r.branch("b").unwrap();
        r.commit_changes("a", "x", "fill 1,1,1 1,1,1\n", &[ch(1, 65, 1, 1)]);
        r.switch("b").unwrap();
        r.commit_changes("b", "y", "fill 9,9,9 9,9,9\n", &[ch(200, 65, 200, 1)]);
        r.merge("main", Policy::Ours, "b").unwrap();
        r.branch("c").unwrap(); r.switch("c").unwrap();
        r.commit_changes("c", "z", "fill 2,2,2 2,2,2\n", &[ch(400, 65, 400, 1)]);
        r.switch("main").unwrap();
        r.commit_changes("a", "w", "fill 3,3,3 3,3,3\n", &[ch(600, 65, 600, 1)]);
        let e = r.rebase_plan("c", "main").unwrap_err();
        assert!(e.contains("merge"), "{e}");
    }

    #[test]
    fn a_repo_round_trips_through_its_save() {
        let mut r = Repo::new(32);
        r.commit_changes("kess", "a tower here", "cylinder @0,0 r=3 h=9\nsphere @3,0 r=1 block=air\n",
                         &[ch(0, 66, 0, 1), ch(3, 66, 0, 0)]);
        r.branch("friend's mill").unwrap(); r.switch("friend's mill").unwrap();
        r.commit_changes("friend", "the mill\nwith a wheel", "form house @0,0 @8,8\n", &[ch(200, 66, 200, 17)]);
        let back = Repo::load(&r.save());
        assert_eq!(back, r);
    }

    #[test]
    fn the_log_walks_first_parents() {
        let mut r = Repo::new(32);
        r.commit_changes("a", "one", "", &[ch(1, 65, 1, 1)]);
        r.commit_changes("a", "two", "", &[ch(2, 65, 1, 1)]);
        let l = r.log("main", 10);
        let msgs: Vec<&str> = l.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(msgs, ["two", "one", "as generated"]);
    }

    #[test]
    fn a_rebase_replays_on_the_new_ground_where_a_merge_would_bury() {
        // The whole argument for a tree whose commits carry IR rather
        // than bytes, as a test.
        use crate::plan::{parse_plan_at, run_plan};
        use crate::{VoxelTools, VoxelWorld};
        #[derive(Clone)]
        struct W(BTreeMap<(i32, i32, i32), u16>);
        impl VoxelWorld for W {
            fn get(&self, x: i32, y: i32, z: i32) -> u16 {
                if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
                if y <= 64 { 2 } else { 0 }
            }
            fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
        }
        let height = |w: &W| (0..200).rev().find(|&y| w.get(0, y, 0) != 0).unwrap();
        // re-anchored replay: y from the ground, x,z from the commit
        let replay = |w: &mut W, c: &Commit| {
            let ay = (0..200).rev().find(|&y| w.get(c.anchor.0, y, c.anchor.1) != 0).unwrap();
            let (plan, _) = parse_plan_at(&c.ir, Some((c.anchor.0, ay, c.anchor.1)));
            let mut t = VoxelTools::new(w);
            run_plan(&mut t, &plan, 200, 32, false);
        };
        let mut r = Repo::new(32);
        r.branch("tower").unwrap();
        r.commit_changes("friend", "raise", "brush raise @0,0 by=12 r=30 hill\n", &[ch(0, 76, 0, 2)]);
        r.switch("tower").unwrap();
        r.commit_changes_at("kess", "tower", "cylinder @0,0 r=3 h=9 block=stone_bricks\n",
                            (0, 0), &[ch(0, 66, 0, 58)]);
        // the world after main's raise
        let mut hill = W(BTreeMap::new());
        let (p, _) = parse_plan_at("brush raise @0,0 by=12 r=30 hill\n", Some((0, 64, 0)));
        { let mut t = VoxelTools::new(&mut hill); run_plan(&mut t, &p, 200, 32, false); }
        let ground = height(&hill);
        assert!(ground > 70, "the hill is there: {ground}");
        let plan = r.rebase_plan("tower", "main").unwrap();
        for c in &plan { replay(&mut hill, c); }
        let top = height(&hill);
        assert_eq!(top - ground, 9, "the whole tower stands on the hill: top {top}, ground {ground}");
    }

    #[test]
    fn the_anchor_survives_a_save() {
        let mut r = Repo::new(32);
        r.commit_changes_at("k", "t", "cylinder @0,0 r=2\n", (120, -40), &[ch(120, 66, -40, 1)]);
        let back = Repo::load(&r.save());
        assert_eq!(back.head_commit().anchor, (120, -40));
        assert_eq!(back, r);
    }

    #[test]
    fn an_empty_or_truncated_save_loads_as_a_fresh_repo() {
        // A crash between create and write leaves an empty file; the
        // first version indexed `branches["main"]` on it and panicked.
        let r = Repo::load("");
        assert_eq!(r.head_commit().message, "as generated");
        let r2 = Repo::load("# unendlich worldtree v1\nchunk 32\nhead main\n");
        assert_eq!(r2.head_commit().message, "as generated");
        let r3 = Repo::load("# unendlich worldtree v1\nchunk 16\nhead main\nbranch main deadbeef\n");
        assert_eq!(r3.chunk_size, 16, "the chunk size is kept");
        assert_eq!(r3.head_commit().message, "as generated", "a dangling branch is dropped");
    }
}
