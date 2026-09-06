//! A latent space over the world tree.
//!
//! Every commit in the tree is a build. Measure it -- size, height,
//! durability, enclosure, shelter, symmetry, openness, silhouette,
//! material mix, footprint -- and it is a point in a space where
//! nearby points are similar buildings. No model learns this space;
//! the axes are the properties the spec tuner already targets, so a
//! point in it is directly a SPEC, and that is what makes it useful:
//!
//! * "something like the mill" -- nearest neighbours of the mill's
//!   commit among the tree's other commits
//! * "halfway between the chapel and the bunker" -- interpolate two
//!   points and hand the result to the tuner as a spec
//! * "like the mill but taller" -- the mill's point with one axis moved
//!
//! CACHED FOR THE SESSION
//!
//! The tree is content-addressed. A commit's cells never change, so
//! its vector never changes: computed once, keyed by the commit hash,
//! kept for the whole running session. A world with a thousand commits
//! embeds each on first sight and never again. Nothing is persisted --
//! the vectors are a pure function of the tree and come back for free.

use crate::spec::{Level, Sound, Spec};
use crate::vcs::{Hash, Repo};
use std::collections::{BTreeMap, BTreeSet};

pub const DIMS: usize = 16;

/// Axis names, for describing a vector or a difference in words.
pub const AXES: [&str; DIMS] = [
    "size", "height", "footprint", "durability", "enclosure", "shelter", "symmetry", "openness",
    "fill", "skyline", "aspect", "stone", "wood", "glass_water", "foliage", "carved",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vector(pub [f32; DIMS]);

impl Vector {
    pub fn distance(&self, o: &Vector) -> f32 {
        self.0.iter().zip(&o.0).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt()
    }
    pub fn lerp(&self, o: &Vector, t: f32) -> Vector {
        let t = t.clamp(0.0, 1.0);
        let mut v = [0.0; DIMS];
        for i in 0..DIMS { v[i] = self.0[i] + (o.0[i] - self.0[i]) * t; }
        Vector(v)
    }
    /// The axes on which two builds differ most, in words.
    pub fn describe_diff(&self, o: &Vector) -> String {
        let mut d: Vec<(f32, usize)> = (0..DIMS).map(|i| (o.0[i] - self.0[i], i)).collect();
        d.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap());
        d.iter().take(3).filter(|(v, _)| v.abs() > 0.08)
            .map(|(v, i)| format!("{} {}", if *v > 0.0 { "more" } else { "less" }, AXES[*i]))
            .collect::<Vec<_>>().join(", ")
    }
}

/// Which block ids count as which material class, for the mix axes.
fn class(b: u16) -> usize {
    match b {
        1 | 4 | 58 | 37 | 42 | 48 | 24 | 38 | 49 | 81 | 84 | 13 | 12 => 11,       // stone-ish
        17 | 19 | 57 | 86 | 21 => 12,                                            // wood
        61 | 9 | 31 | 80 | 32 => 13,                                             // glass, water, ice, snow
        18 | 20 | 23 | 25 | 27 | 29 | 33 => 14,                                  // foliage
        _ => 11,
    }
}

/// Embed a set of cells. Pure. `ground_y` as for `spec::measure`.
pub fn embed(cells: &BTreeMap<(i32, i32, i32), u16>, ground_y: i32) -> Vector {
    let mut v = [0.0f32; DIMS];
    if cells.is_empty() { return Vector(v); }
    let solid: BTreeSet<(i32, i32, i32)> = cells.iter().filter(|(_, b)| **b != 0).map(|(k, _)| *k).collect();
    let carved = cells.len() - solid.len();
    if solid.is_empty() { v[15] = 1.0; return Vector(v); }
    let p = crate::spec::measure(&solid, ground_y);
    let sil = crate::finish::silhouette(&solid.iter().copied().collect::<Vec<_>>()).unwrap();
    let lo = solid.iter().fold((i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.2)));
    let hi = solid.iter().fold((i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.2)));
    let footprint = ((hi.0 - lo.0 + 1) * (hi.1 - lo.1 + 1)) as f32;
    // log scales for the unbounded ones, mapped into 0..1 over the
    // range a build actually spans (10 blocks .. 1M; 2 tall .. 128)
    v[0] = ((p.blocks as f32).max(1.0).log10() / 6.0).clamp(0.0, 1.0);
    v[1] = ((p.height as f32).max(1.0).log2() / 7.0).clamp(0.0, 1.0);
    v[2] = (footprint.max(1.0).log10() / 5.0).clamp(0.0, 1.0);
    v[3] = p.durability; v[4] = p.enclosure; v[5] = p.shelter; v[6] = p.symmetry; v[7] = p.openness;
    v[8] = sil.fill; v[9] = sil.skyline_variety; v[10] = ((sil.aspect - 1.0) / 4.0).clamp(0.0, 1.0);
    let n = solid.len() as f32;
    for (k, b) in cells { if *b != 0 { v[class(*b)] += 1.0 / n; } let _ = k; }
    v[15] = carved as f32 / cells.len() as f32;
    Vector(v)
}

/// A point back to a spec: the axes the tuner can target, as targets.
/// Only the ones that mean something as a target; material mix and
/// silhouette are described, not demanded.
pub fn to_spec(v: &Vector) -> Spec {
    let blocks = 10f32.powf(v.0[0] * 6.0) as usize;
    let height = 2f32.powf(v.0[1] * 7.0) as i32;
    let level = |x: f32| if x < 0.35 { Level::Low } else if x < 0.65 { Level::Mid } else { Level::High };
    Spec {
        blocks: Some(((blocks as f32 * 0.8) as usize, (blocks as f32 * 1.2) as usize)),
        height: Some(((height as f32 * 0.85) as i32, (height as f32 * 1.15) as i32)),
        durability: Some(level(v.0[3])),
        sound: Some(if v.0[4] > 0.7 { if v.0[7] > 0.2 { Sound::Echoing } else { Sound::Muffled } } else { Sound::Airy }),
        shelter: Some(level(v.0[5])),
        symmetry: Some(v.0[6]),
        openness: Some(v.0[7]),
    }
}

/// The session's index: commit hash -> vector, computed on first sight.
#[derive(Debug, Default)]
pub struct Index {
    vectors: BTreeMap<Hash, Vector>,
    names: BTreeMap<Hash, String>,
    pub hits: usize,
    pub misses: usize,
}

impl Index {
    pub fn new() -> Self { Self::default() }
    pub fn len(&self) -> usize { self.vectors.len() }
    pub fn is_empty(&self) -> bool { self.vectors.is_empty() }

    /// The cells a commit changed over its first parent, in world
    /// coordinates, from the tree's blobs. This is the build the
    /// commit IS, as distinct from the whole world after it.
    fn cells_of(repo: &Repo, h: Hash) -> BTreeMap<(i32, i32, i32), u16> {
        let Some(c) = repo.commit(h) else { return BTreeMap::new() };
        let parent = c.parents.first().copied();
        let (Some(tree), parent_tree) = (repo.tree(c.tree), parent.and_then(|p| repo.commit(p)).and_then(|p| repo.tree(p.tree)))
            else { return BTreeMap::new() };
        let cs = repo.chunk_size;
        let mut out = BTreeMap::new();
        for (chunk, bh) in &tree.chunks {
            if parent_tree.and_then(|pt| pt.chunks.get(chunk)) == Some(bh) { continue; }
            let Some(blob) = repo.blob(*bh) else { continue };
            let pblob = parent_tree.and_then(|pt| pt.chunks.get(chunk)).and_then(|h| repo.blob(*h));
            for (&(lx, ly, lz), &b) in &blob.cells {
                if pblob.and_then(|p| p.cells.get(&(lx, ly, lz))) == Some(&b) { continue; }
                out.insert((chunk.0 * cs + lx as i32, chunk.1 * cs + ly as i32, chunk.2 * cs + lz as i32), b);
            }
        }
        out
    }

    /// The vector for a commit, from the cache or computed and cached.
    pub fn vector(&mut self, repo: &Repo, h: Hash, ground_y: i32) -> Vector {
        if let Some(v) = self.vectors.get(&h) { self.hits += 1; return *v; }
        self.misses += 1;
        let cells = Self::cells_of(repo, h);
        let v = embed(&cells, ground_y);
        self.vectors.insert(h, v);
        if let Some(c) = repo.commit(h) { self.names.insert(h, c.message.clone()); }
        v
    }

    /// Embed every commit on a branch not yet seen. Incremental: a
    /// second call after one new commit embeds one build.
    pub fn index_branch(&mut self, repo: &Repo, branch: &str, ground_y: i32) -> usize {
        let mut fresh = 0;
        let mut h = repo.branch_head(branch);
        while let Some(cur) = h {
            if !self.vectors.contains_key(&cur) { self.vector(repo, cur, ground_y); fresh += 1; }
            h = repo.commit(cur).and_then(|c| c.parents.first().copied());
        }
        fresh
    }

    /// The k nearest builds to a point, excluding `not`.
    pub fn nearest(&self, q: &Vector, k: usize, not: Option<Hash>) -> Vec<(Hash, f32, String)> {
        let mut all: Vec<(Hash, f32, String)> = self.vectors.iter()
            .filter(|(h, _)| Some(**h) != not)
            .filter(|(_, v)| v.0[0] > 0.0)                    // skip empty (root) commits
            .map(|(h, v)| (*h, q.distance(v), self.names.get(h).cloned().unwrap_or_default()))
            .collect();
        all.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(k);
        all
    }

    pub fn get(&self, h: Hash) -> Option<Vector> { self.vectors.get(&h).copied() }

    /// Find a commit by a word in its message, for "like the mill".
    pub fn find_named(&self, word: &str) -> Option<Hash> {
        let w = word.to_lowercase();
        self.names.iter().find(|(_, n)| n.to_lowercase().contains(&w)).map(|(h, _)| *h)
    }
}

/// Nudge a vector along a named axis: "taller" is +height, "thicker"
/// is +durability, "more open" is +openness. Returns None for a word
/// that names no axis.
pub fn nudge(v: &Vector, word: &str) -> Option<Vector> {
    let (axis, dir): (usize, f32) = match word.to_lowercase().trim_end_matches("er").trim_end_matches("r") {
        "tall" | "high" => (1, 1.0), "short" | "low" => (1, -1.0),
        "big" | "large" | "bigge" => (0, 1.0), "small" => (0, -1.0),
        "wide" => (2, 1.0), "narrow" => (2, -1.0),
        "thick" | "sturdy" | "solid" | "durable" => (3, 1.0), "thin" | "light" => (3, -1.0),
        "enclosed" | "sealed" | "closed" => (4, 1.0), "open" | "airy" => (4, -1.0),
        "sheltered" | "covered" => (5, 1.0), "exposed" => (5, -1.0),
        "symmetric" | "formal" => (6, 1.0), "irregular" | "organic" => (6, -1.0),
        "spacious" | "roomy" => (7, 1.0), "dense" | "cramped" => (7, -1.0),
        _ => return None,
    };
    let mut o = *v;
    // Size, height and footprint are log axes: 0.25 there is x3.4 in
    // height, and "taller" turned a 9-tall mill into a 30-tall one.
    // 0.1 is x1.6, which is what "taller" means.
    let step = if axis <= 2 { 0.1 } else { 0.25 };
    o.0[axis] = (o.0[axis] + step * dir).clamp(0.0, 1.0);
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather::Change;

    fn ch(x: i32, y: i32, z: i32, b: u16) -> Change { Change { at: (x, y, z), to: b } }
    fn hut(x0: i32, z0: i32, w: i32, h: i32, roof: bool) -> Vec<Change> {
        let mut v = Vec::new();
        for x in x0..x0 + w { for y in 65..65 + h { for z in z0..z0 + w {
            let wall = x == x0 || x == x0 + w - 1 || z == z0 || z == z0 + w - 1;
            if wall || (roof && y == 64 + h) { v.push(ch(x, y, z, 57)); }
        }}}
        v
    }

    #[test]
    fn similar_builds_are_near_and_different_ones_far() {
        let a = embed(&hut(0, 0, 9, 5, true).into_iter().map(|c| (c.at, c.to)).collect(), 64);
        let b = embed(&hut(40, 0, 9, 5, true).into_iter().map(|c| (c.at, c.to)).collect(), 64);
        // a WOODEN tower, so the difference is shape, not material --
        // material is an axis too, and a stone tower's biggest
        // difference from a wooden hut is, correctly, the stone
        let tower: BTreeMap<_, _> = (65..90).flat_map(|y| [(2, y, 2), (3, y, 2), (2, y, 3), (3, y, 3)]).map(|k| (k, 57u16)).collect();
        let t = embed(&tower, 64);
        assert!(a.distance(&b) < 0.05, "two identical huts elsewhere: {}", a.distance(&b));
        assert!(a.distance(&t) > 0.4, "a hut and a tower: {}", a.distance(&t));
        // the same hut, three times as tall: height leads the diff
        let tall = embed(&hut(0, 0, 9, 15, true).into_iter().map(|c| (c.at, c.to)).collect(), 64);
        assert!(a.describe_diff(&tall).contains("more height"), "{}", a.describe_diff(&tall));
    }

    #[test]
    fn the_index_caches_by_commit_hash_for_the_session() {
        let mut repo = Repo::new(32);
        repo.commit_changes("k", "a hut", "", &hut(0, 0, 9, 5, true));
        repo.commit_changes("k", "a bigger hut", "", &hut(20, 0, 13, 6, true));
        let mut idx = Index::new();
        assert_eq!(idx.index_branch(&repo, "main", 64), 3, "root + two builds on first sight");
        assert_eq!(idx.index_branch(&repo, "main", 64), 0, "all cached");
        let h = repo.branch_head("main").unwrap();
        let before = idx.hits;
        idx.vector(&repo, h, 64);
        assert_eq!(idx.hits, before + 1, "a hit, no recompute");
        repo.commit_changes("k", "a tower", "", &(65..90).flat_map(|y| [ch(50, y, 50, 1), ch(51, y, 50, 1)]).collect::<Vec<_>>());
        assert_eq!(idx.index_branch(&repo, "main", 64), 1, "incremental: one new build");
    }

    #[test]
    fn a_commit_embeds_the_build_it_is_not_the_whole_world_after_it() {
        // The second hut's vector must be a hut, not "two huts".
        let mut repo = Repo::new(32);
        repo.commit_changes("k", "hut one", "", &hut(0, 0, 9, 5, true));
        repo.commit_changes("k", "hut two", "", &hut(40, 0, 9, 5, true));
        let mut idx = Index::new();
        idx.index_branch(&repo, "main", 64);
        let log = repo.log("main", 3);
        let (two, one) = (idx.get(log[0].hash).unwrap(), idx.get(log[1].hash).unwrap());
        assert!(one.distance(&two) < 0.05, "each commit is its own hut: {}", one.distance(&two));
    }

    #[test]
    fn nearest_finds_the_hut_like_the_hut_and_names_it() {
        let mut repo = Repo::new(32);
        repo.commit_changes("k", "the old mill", "", &hut(0, 0, 9, 5, true));
        repo.commit_changes("k", "a watchtower", "", &(65..90).flat_map(|y| [ch(50, y, 50, 1), ch(51, y, 50, 1), ch(50, y, 51, 1), ch(51, y, 51, 1)]).collect::<Vec<_>>());
        repo.commit_changes("k", "the barn", "", &hut(40, 0, 11, 5, true));
        let mut idx = Index::new();
        idx.index_branch(&repo, "main", 64);
        let mill = idx.find_named("mill").expect("found by word");
        let near = idx.nearest(&idx.get(mill).unwrap(), 1, Some(mill));
        assert_eq!(near[0].2, "the barn", "the barn is the mill's nearest, not the tower: {near:?}");
    }

    #[test]
    fn a_point_between_two_builds_is_a_spec_the_tuner_can_target() {
        let hut_v = embed(&hut(0, 0, 9, 5, true).into_iter().map(|c| (c.at, c.to)).collect(), 64);
        let tower: BTreeMap<_, _> = (65..90).flat_map(|y| [(2, y, 2), (3, y, 2), (2, y, 3), (3, y, 3)]).map(|k| (k, 1u16)).collect();
        let tower_v = embed(&tower, 64);
        let mid = hut_v.lerp(&tower_v, 0.5);
        let spec = to_spec(&mid);
        let (h0, h1) = spec.height.unwrap();
        assert!(h0 > 5 && h1 < 25, "between a 5-tall hut and a 25-tall tower: {h0}-{h1}");
        assert!(!spec.is_empty());
        // and nudging: "taller" moves the height axis
        let taller = nudge(&hut_v, "taller").unwrap();
        assert!(taller.0[1] > hut_v.0[1]);
        let (h0, _) = to_spec(&taller).height.unwrap();
        let (b0, _) = to_spec(&hut_v).height.unwrap();
        assert!(h0 > b0 && h0 < b0 * 3, "taller is x1.6-ish, not x3: {b0} -> {h0}");
        assert!(nudge(&hut_v, "purple").is_none());
    }
}
