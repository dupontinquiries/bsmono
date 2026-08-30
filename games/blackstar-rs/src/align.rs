//! STRIP ALIGNMENT — make a fold strip blendable by finding which prim is
//! which across poses.
//!
//! `xform::Morph::build` requires the poses of a strip to agree prim for
//! prim and vertex for vertex. No shipped strip does, so every chassis
//! silently falls back to stepping between whole poses — which is the
//! "bad transforms" a player actually sees. Three correspondence keys
//! were probed and rejected before this module (`foldsim::shapes`):
//! colour, colour+count, and rigid shape invariants. All three read
//! attributes a fold CHANGES. The key that works — measured across the
//! roster by `foldsim::align_probe` before this was written — is the one
//! attribute a fold CANNOT change: texture coordinates. A triangle keeps
//! its UV mapping while its corners travel, or the paint would swim
//! during the animation.
//!
//! What the probe measured, and this module is designed against:
//!
//! * A prim's (vertex count, sorted quantised UV multiset) matches
//!   exactly across poses wherever the same part exists in both.
//! * For every such match, the intra-prim VERTEX ORDER also agrees —
//!   100% of matches, every chassis probed — so a per-index lerp of
//!   matched prims is valid as-is.
//! * The residue is not a regrouping (the unmatched sides' vertex sets
//!   differ): poses carry POSE-SPECIFIC prims, transitional pieces that
//!   exist only in part of the strip. Counts like 13→18→26→27→13 are
//!   parts appearing toward mid-fold and going away again.
//!
//! So the output is a UNION prim list: one entry per distinct part seen
//! anywhere in the strip. Where a part exists in a pose, that pose holds
//! its real vertices; where it does not, a placeholder holds the nearest
//! present pose's copy COLLAPSED to its centroid. A straight per-index
//! lerp then does the right thing everywhere: matched parts travel,
//! appearing parts scale in from a point across the segment where they
//! arrive, departing parts scale out. Scale-about-centroid reads as the
//! part folding out of (or into) the body, which for a transformer is
//! the effect wanted anyway — and it is C0 at every pose boundary, so
//! the per-pose POP that stepping produces cannot happen.
//!
//! The contract with the existing pipeline is deliberately thin: this
//! runs BEFORE `Morph::build` and emits exactly the uniform-shape input
//! Morph already requires. Morph, its compaction layer, `bounds_at`, and
//! `foldaudit` are all untouched. On refusal (`None`) the caller keeps
//! stepping, which is today's behaviour — alignment can only improve a
//! chassis, never regress one.

use glam::Vec3;

use crate::meshgen::Vertex;
use crate::model::{RawPrim, Tag};

/// UV quantisation: 1/4096 of texture space. Coarse enough that a
/// re-emitted float differing in its last bits still lands on the same
/// cell (the same reason `foldsim::shapes` quantises colour), fine
/// enough that distinct texels do not collide.
const UV_Q: f32 = 4096.0;

/// Refuse a whole strip when fewer than this fraction of prim slots
/// across all adjacent pose pairs matched. Measured worst case on the
/// roster is well above it (13/18); a strip where under half the parts
/// persist between neighbouring poses is not the pattern this was
/// designed against, and the known fallback beats a guess.
const MIN_MATCHED: f32 = 0.5;

/// A union prim beyond the first pose's own set. The base model has no
/// draw slot for it, so the caller allocates one from this.
pub struct Extra {
    pub color: [f32; 4],
    pub tag: Tag,
}

/// What alignment decided, printable so a chassis can be judged on real
/// assets rather than argued about.
#[derive(Default)]
pub struct Report {
    /// prims matched to an existing union entry, summed over poses 1..
    pub matched: usize,
    /// prims that joined the union as new entries (pose-specific parts)
    pub joined: usize,
    /// matches DOWNGRADED to fade because the vertex order disagreed or
    /// the centroid travel exceeded the body bound — either would have
    /// lerped garbage
    pub downgraded: usize,
}

impl Report {
    pub fn line(&self) -> String {
        format!(
            "align: {} matched, {} pose-specific, {} downgraded",
            self.matched, self.joined, self.downgraded
        )
    }
}

pub struct Unified {
    /// `poses[k][u]` — union prim `u`'s vertices at pose `k`. Uniform
    /// shape by construction: exactly what `Morph::build` requires.
    pub poses: Vec<Vec<Vec<Vertex>>>,
    /// per union prim, the index buffer from its defining pose
    pub idx: Vec<Vec<u32>>,
    /// union prims `base_prims..`, in order, for draw-slot allocation
    pub extras: Vec<Extra>,
    /// how many union prims the FIRST pose already had. Union entries
    /// below this line map one-to-one onto the base model's prims, in
    /// the base model's own order.
    pub base_prims: usize,
    pub report: Report,
}

/// One union entry under construction.
struct Entry {
    key: u64,
    /// quantised per-vertex UV hashes, in emission order — the identity
    /// AND the order check in one vector
    seq: Vec<u64>,
    idx: Vec<u32>,
    color: [f32; 4],
    tag: Tag,
    /// verts per pose; None until filled (placeholder pass runs last)
    at: Vec<Option<Vec<Vertex>>>,
    /// centroid at the latest pose where present — what "nearest
    /// candidate" is measured against, so matching chains through the
    /// strip instead of comparing everything to pose 0
    last_c: Vec3,
}

fn centroid(vs: &[Vertex]) -> Vec3 {
    if vs.is_empty() {
        return Vec3::ZERO;
    }
    vs.iter().fold(Vec3::ZERO, |a, v| a + Vec3::from_array(v.pos)) / vs.len() as f32
}

fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn uv_seq(vs: &[Vertex]) -> Vec<u64> {
    vs.iter()
        .map(|v| {
            let u = (v.uv[0] * UV_Q).round() as i64;
            let w = (v.uv[1] * UV_Q).round() as i64;
            fnv(u.to_le_bytes().into_iter().chain(w.to_le_bytes()))
        })
        .collect()
}

fn key_of(seq: &[u64]) -> u64 {
    let mut s = seq.to_vec();
    s.sort_unstable();
    fnv(
        (s.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(s.into_iter().flat_map(|h| h.to_le_bytes())),
    )
}

/// The matching pass shared by `unify` and `diagnose`: for each pose in
/// order, key each prim by its UV sequence and match it against the
/// nearest same-key entry not yet filled at this pose, falling back to a
/// new entry (a join) when nothing matches. Returns the resulting
/// entries, the match `Report`, and `pair_slots` (the threshold
/// denominator: `min(prims)` summed over adjacent pose pairs). Pulled out
/// of `unify` verbatim so `diagnose` can see the report on a refusal
/// `unify` itself would just return `None` for.
fn match_pass(strip: Vec<Vec<RawPrim>>) -> (Vec<Entry>, Report, usize) {
    let n_poses = strip.len();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in &strip[0] {
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let travel_cap = ((hi - lo) * 0.5).max_element().max(0.1);

    let mut entries: Vec<Entry> = Vec::new();
    let mut report = Report::default();
    let mut pair_slots = 0usize;
    let mut prev_count = 0usize;

    for (k, pose) in strip.into_iter().enumerate() {
        if k > 0 {
            pair_slots += prev_count.min(pose.len());
        }
        prev_count = pose.len();
        for r in pose {
            let seq = uv_seq(&r.verts);
            let key = key_of(&seq);
            let c = centroid(&r.verts);
            let cand = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.key == key && e.at[k].is_none())
                .min_by(|(_, a), (_, b)| {
                    (a.last_c - c)
                        .length_squared()
                        .partial_cmp(&(b.last_c - c).length_squared())
                        .unwrap()
                });
            let accepted = match cand {
                Some((ei, e)) if k > 0 => {
                    if e.seq == seq && (e.last_c - c).length() <= travel_cap {
                        Some(ei)
                    } else {
                        report.downgraded += 1;
                        None
                    }
                }
                _ => None,
            };
            match accepted {
                Some(ei) => {
                    report.matched += 1;
                    entries[ei].last_c = c;
                    entries[ei].at[k] = Some(r.verts);
                }
                None => {
                    if k > 0 {
                        report.joined += 1;
                    }
                    let mut at = vec![None; n_poses];
                    at[k] = Some(r.verts);
                    entries.push(Entry {
                        key,
                        seq,
                        idx: r.idx,
                        color: r.color,
                        tag: r.tag,
                        at,
                        last_c: c,
                    });
                }
            }
        }
    }
    (entries, report, pair_slots)
}

/// Same match pass as `diagnose`, but names which prims (by pose-local
/// index) never found a partner in the pose before them — the actual
/// list behind the percentage, for finding out whether a near-miss is
/// one bad prim or scattered noise.
pub fn diagnose_unmatched(strip: &[Vec<RawPrim>]) -> Vec<(usize, usize)> {
    if strip.len() < 2 || strip.iter().any(|p| p.is_empty()) {
        return Vec::new();
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in &strip[0] {
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let travel_cap = ((hi - lo) * 0.5).max_element().max(0.1);

    struct Light {
        key: u64,
        seq: Vec<u64>,
        last_c: Vec3,
        filled: Vec<bool>,
    }
    let n_poses = strip.len();
    let mut entries: Vec<Light> = Vec::new();
    let mut unmatched: Vec<(usize, usize)> = Vec::new(); // (pose, prim-in-pose)

    for (k, pose) in strip.iter().enumerate() {
        for (pi, r) in pose.iter().enumerate() {
            let seq = uv_seq(&r.verts);
            let key = key_of(&seq);
            let c = centroid(&r.verts);
            let cand = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.key == key && !e.filled[k])
                .min_by(|(_, a), (_, b)| {
                    (a.last_c - c)
                        .length_squared()
                        .partial_cmp(&(b.last_c - c).length_squared())
                        .unwrap()
                });
            let accepted = match cand {
                Some((ei, e)) if k > 0 => {
                    (e.seq == seq && (e.last_c - c).length() <= travel_cap).then_some(ei)
                }
                _ => None,
            };
            match accepted {
                Some(ei) => {
                    entries[ei].last_c = c;
                    entries[ei].filled[k] = true;
                }
                None => {
                    if k > 0 {
                        unmatched.push((k, pi));
                    }
                    let mut filled = vec![false; n_poses];
                    filled[k] = true;
                    entries.push(Light { key, seq, last_c: c, filled });
                }
            }
        }
    }
    unmatched
}

/// Read-only diagnostic: how close a strip got to unifying, even when it
/// refused. Returns `(matched, pair_slots)` — `unify` refuses below
/// `MIN_MATCHED * pair_slots`, so `matched as f32 / pair_slots as f32`
/// against `MIN_MATCHED` (0.5) tells a caller whether a chassis is a
/// near-miss worth a second look or wildly mismatched. `None` only for
/// the same degenerate inputs `unify` itself refuses outright (fewer
/// than 2 poses, or an empty pose).
///
/// Deliberately independent of `match_pass`/`unify` rather than sharing
/// it: `RawPrim` isn't `Clone` and this only needs the match counts, not
/// ownership of every vertex, so it re-derives just the key/centroid
/// bookkeeping by reference instead of taking `strip` twice.
pub fn diagnose(strip: &[Vec<RawPrim>]) -> Option<(usize, usize)> {
    if strip.len() < 2 || strip.iter().any(|p| p.is_empty()) {
        return None;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in &strip[0] {
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let travel_cap = ((hi - lo) * 0.5).max_element().max(0.1);

    struct Light {
        key: u64,
        seq: Vec<u64>,
        last_c: Vec3,
        filled: Vec<bool>,
    }
    let n_poses = strip.len();
    let mut entries: Vec<Light> = Vec::new();
    let mut matched = 0usize;
    let mut pair_slots = 0usize;
    let mut prev_count = 0usize;

    for (k, pose) in strip.iter().enumerate() {
        if k > 0 {
            pair_slots += prev_count.min(pose.len());
        }
        prev_count = pose.len();
        for r in pose {
            let seq = uv_seq(&r.verts);
            let key = key_of(&seq);
            let c = centroid(&r.verts);
            let cand = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.key == key && !e.filled[k])
                .min_by(|(_, a), (_, b)| {
                    (a.last_c - c)
                        .length_squared()
                        .partial_cmp(&(b.last_c - c).length_squared())
                        .unwrap()
                });
            let accepted = match cand {
                Some((ei, e)) if k > 0 => {
                    (e.seq == seq && (e.last_c - c).length() <= travel_cap).then_some(ei)
                }
                _ => None,
            };
            match accepted {
                Some(ei) => {
                    matched += 1;
                    entries[ei].last_c = c;
                    entries[ei].filled[k] = true;
                }
                None => {
                    let mut filled = vec![false; n_poses];
                    filled[k] = true;
                    entries.push(Light { key, seq, last_c: c, filled });
                }
            }
        }
    }
    Some((matched, pair_slots))
}

/// Align a strip into a union-prim blend input. `None` refuses — the
/// caller falls back to stepping, exactly today's behaviour.
pub fn unify(strip: Vec<Vec<RawPrim>>) -> Option<Unified> {
    let n_poses = strip.len();
    if n_poses < 2 || strip.iter().any(|p| p.is_empty()) {
        return None;
    }
    let (entries, report, pair_slots) = match_pass(strip);

    if pair_slots > 0 && (report.matched as f32) < MIN_MATCHED * pair_slots as f32 {
        return None; // not the measured pattern — the known fallback wins
    }

    // base_prims: union entries created by pose 0, which are its prims
    // in its own order because pose 0 ran first and matched nothing
    let base_prims = entries.iter().filter(|e| e.at[0].is_some()).count();

    // PLACEHOLDERS. Two cases, and they must not share a rule:
    //
    // * Absent at an END of the entry's presence range — the part truly
    //   appears or disappears there. Hold the nearest present copy
    //   collapsed to its own centroid, so the segment lerp is a pure
    //   scale about a fixed point: the part grows out of the body
    //   arriving, shrinks into it leaving.
    //
    // * Absent in a GAP with presence on both sides (`#.#` — exporters
    //   drop parts from intermediate poses). Collapsing here would
    //   shrink the part to a point and regrow it, a flicker. The part
    //   did not really vanish, so interpolate its two neighbouring
    //   copies through the gap instead: counts and order agree (that is
    //   what being the same entry means), so a per-index lerp is valid.
    let n_entries = entries.len();
    let mut poses: Vec<Vec<Vec<Vertex>>> = vec![Vec::with_capacity(n_entries); n_poses];
    let mut idx = Vec::with_capacity(n_entries);
    let mut extras = Vec::new();
    for e in entries {
        for k in 0..n_poses {
            let verts = match &e.at[k] {
                Some(v) => v.clone(),
                None => {
                    let below = (0..k).rev().find(|j| e.at[*j].is_some());
                    let above = (k + 1..n_poses).find(|j| e.at[*j].is_some());
                    match (below, above) {
                        (Some(j0), Some(j1)) => {
                            let (a, b) = (e.at[j0].as_ref().unwrap(), e.at[j1].as_ref().unwrap());
                            let w = (k - j0) as f32 / (j1 - j0) as f32;
                            a.iter()
                                .zip(b.iter())
                                .map(|(va, vb)| Vertex {
                                    pos: Vec3::from_array(va.pos)
                                        .lerp(Vec3::from_array(vb.pos), w)
                                        .to_array(),
                                    nrm: Vec3::from_array(va.nrm)
                                        .lerp(Vec3::from_array(vb.nrm), w)
                                        .normalize_or(Vec3::from_array(va.nrm))
                                        .to_array(),
                                    uv: va.uv,
                                })
                                .collect()
                        }
                        (j0, j1) => {
                            let j = j0.or(j1).expect("every entry is present somewhere");
                            let src = e.at[j].as_ref().unwrap();
                            let c = centroid(src).to_array();
                            src.iter().map(|v| Vertex { pos: c, nrm: v.nrm, uv: v.uv }).collect()
                        }
                    }
                }
            };
            poses[k].push(verts);
        }
        if e.at[0].is_none() {
            extras.push(Extra { color: e.color, tag: e.tag });
        }
        idx.push(e.idx);
    }

    // pose 0's entries lead the list in pose-0 order (construction
    // order), so slot u < base_prims IS base prim u — assert it rather
    // than trust it, since the draw path writes meshes by this mapping
    debug_assert!(poses[0][..base_prims].iter().all(|v| !v.is_empty()));

    Some(Unified { poses, idx, extras, base_prims, report })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(offset: Vec3, uv0: f32, n: usize) -> RawPrim {
        let verts = (0..n)
            .map(|i| Vertex {
                pos: (offset + Vec3::new(i as f32 * 0.1, 0.0, 0.0)).to_array(),
                nrm: [0.0, 1.0, 0.0],
                uv: [uv0 + i as f32 * 0.01, 0.5],
            })
            .collect();
        RawPrim {
            verts,
            idx: (0..n as u32).collect(),
            color: [1.0; 4],
            tex_uri: None,
            tag: Tag::Body,
            pivot: None,
        }
    }

    fn verts_at(u: &Unified, k: usize, p: usize) -> Vec<Vec3> {
        u.poses[k][p].iter().map(|v| Vec3::from_array(v.pos)).collect()
    }

    #[test]
    fn a_gap_in_presence_interpolates_through_instead_of_flickering() {
        // present at f0 and f2, missing from the f1 export — the part
        // did not really vanish, so it must travel through f1 rather
        // than shrinking to a point and regrowing
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let anchor0 = prim(Vec3::new(3.0, 0.0, 0.0), 0.9, 6);
        let anchor1 = prim(Vec3::new(3.0, 0.1, 0.0), 0.9, 6);
        let anchor2 = prim(Vec3::new(3.0, 0.2, 0.0), 0.9, 6);
        let a2 = prim(Vec3::new(1.0, 0.0, 0.0), 0.0, 5);
        let u = unify(vec![vec![a0, anchor0], vec![anchor1], vec![a2, anchor2]]).expect("aligns");
        let vs = verts_at(&u, 1, 0);
        let spread = vs.iter().map(|p| (*p - vs[0]).length()).fold(0.0f32, f32::max);
        assert!(spread > 0.1, "gap pose collapsed the part: flicker");
        // halfway between its f0 and f2 copies, per index
        let f0 = verts_at(&u, 0, 0);
        let f2 = verts_at(&u, 2, 0);
        for i in 0..vs.len() {
            let want = (f0[i] + f2[i]) * 0.5;
            assert!((vs[i] - want).length() < 1e-4, "gap fill is not the midpoint at v{i}");
        }
    }

    #[test]
    fn a_permuted_strip_aligns_and_lerps_exactly() {
        // pose 0: A then B; pose 1: B then A, both moved a little
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let b0 = prim(Vec3::new(2.0, 0.0, 0.0), 0.3, 7);
        let a1 = prim(Vec3::new(0.2, 0.1, 0.0), 0.0, 5);
        let b1 = prim(Vec3::new(2.2, 0.1, 0.0), 0.3, 7);
        let u = unify(vec![vec![a0, b0], vec![b1, a1]]).expect("aligns");
        assert_eq!(u.poses[0].len(), 2, "no union growth for a pure permutation");
        assert_eq!(u.report.matched, 2);
        assert_eq!(u.report.joined, 0);
        // entry 0 is pose-0's A; at pose 1 it must hold A's moved verts,
        // not B's — the whole point of the key
        assert_eq!(u.poses[1][0].len(), 5, "A matched to B: sizes crossed");
        let travel = (verts_at(&u, 1, 0)[0] - verts_at(&u, 0, 0)[0]).length();
        assert!(travel < 0.5, "entry 0 teleported: {travel}");
    }

    #[test]
    fn a_pose_specific_prim_collapses_to_a_point_where_absent() {
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let a1 = prim(Vec3::new(0.1, 0.0, 0.0), 0.0, 5);
        let mid = prim(Vec3::new(1.0, 1.0, 0.0), 0.7, 4); // only in pose 1
        let a2 = prim(Vec3::new(0.2, 0.0, 0.0), 0.0, 5);
        let u = unify(vec![vec![a0], vec![a1, mid], vec![a2]]).expect("aligns");
        assert_eq!(u.poses[0].len(), 2);
        assert_eq!(u.base_prims, 1);
        assert_eq!(u.extras.len(), 1, "the transitional part needs a draw slot");
        // absent at pose 0 and 2: collapsed to a single point, which is
        // its own pose-1 centroid — so the segment lerp is a scale about
        // a fixed point, in from nothing and back out
        let c1 = verts_at(&u, 1, 1).iter().fold(Vec3::ZERO, |a, b| a + *b) / 4.0;
        for k in [0usize, 2] {
            let vs = verts_at(&u, k, 1);
            assert!(vs.iter().all(|p| (*p - vs[0]).length() < 1e-6), "not collapsed at pose {k}");
            assert!((vs[0] - c1).length() < 1e-4, "collapsed somewhere other than its centroid");
        }
    }

    #[test]
    fn duplicate_parts_keep_their_own_sides() {
        // two IDENTICAL prims (same key), mirrored positions — wheels.
        // each pose moves both slightly; matching must not swap them.
        let l0 = prim(Vec3::new(-2.0, 0.0, 0.0), 0.0, 6);
        let r0 = prim(Vec3::new(2.0, 0.0, 0.0), 0.0, 6);
        let l1 = prim(Vec3::new(-1.8, 0.2, 0.0), 0.0, 6);
        let r1 = prim(Vec3::new(1.8, 0.2, 0.0), 0.0, 6);
        let u = unify(vec![vec![l0, r0], vec![r1, l1]]).expect("aligns");
        // entry 0 is the LEFT wheel; its pose-1 copy must still be on
        // the left, or the fold carries each wheel across the body
        assert!(verts_at(&u, 1, 0)[0].x < 0.0, "wheels traded places");
        assert!(verts_at(&u, 1, 1)[0].x > 0.0, "wheels traded places");
    }

    #[test]
    fn a_teleporting_match_is_downgraded_not_lerped() {
        // same key, but the pose-1 copy is far outside the body's own
        // half-extent — a lerp would sweep it across the world. With a
        // sound match alongside it, the strip still aligns and only the
        // teleporter fades.
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let b0 = prim(Vec3::new(2.0, 0.0, 0.0), 0.3, 7);
        let a1 = prim(Vec3::new(0.1, 0.0, 0.0), 0.0, 5);
        let b1 = prim(Vec3::new(80.0, 0.0, 0.0), 0.3, 7);
        let u = unify(vec![vec![a0, b0], vec![a1, b1]]).expect("aligns, teleporter as fades");
        assert_eq!(u.report.downgraded, 1);
        assert_eq!(u.report.matched, 1);
        assert_eq!(u.poses[0].len(), 3, "the far copy joined as its own entry");
        // and BOTH directions fade rather than travel: entry 1 (b0)
        // collapses at pose 1, entry 2 (b1) collapses at pose 0
        let e1p1 = verts_at(&u, 1, 1);
        assert!(e1p1.iter().all(|p| (*p - e1p1[0]).length() < 1e-6));
        let e2p0 = verts_at(&u, 0, 2);
        assert!(e2p0.iter().all(|p| (*p - e2p0[0]).length() < 1e-6));
    }

    #[test]
    fn a_majority_of_teleports_refuses_like_no_persistence() {
        // when the only slot on offer teleports, nothing persists and
        // the stepping fallback is the better picture — same floor as
        // `a_strip_with_no_persistence_is_refused`, reached through the
        // downgrade path rather than the no-key-match path
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let a1 = prim(Vec3::new(40.0, 0.0, 0.0), 0.0, 5);
        assert!(unify(vec![vec![a0], vec![a1]]).is_none());
    }

    #[test]
    fn a_strip_with_no_persistence_is_refused() {
        // nothing matches between poses: every part is pose-specific.
        // blending this would dissolve the whole body every segment —
        // the stepping fallback is the better picture, so refuse.
        let u = unify(vec![
            vec![prim(Vec3::ZERO, 0.0, 5)],
            vec![prim(Vec3::ZERO, 0.5, 7)],
        ]);
        assert!(u.is_none());
    }

    #[test]
    fn refusal_needs_a_majority_not_perfection() {
        // 2 of 2 slots match on one pair — comfortably above the floor
        // even with a third pose contributing an extra
        let a0 = prim(Vec3::ZERO, 0.0, 5);
        let b0 = prim(Vec3::new(2.0, 0.0, 0.0), 0.3, 7);
        let a1 = prim(Vec3::new(0.1, 0.0, 0.0), 0.0, 5);
        let b1 = prim(Vec3::new(2.1, 0.0, 0.0), 0.3, 7);
        let solo = prim(Vec3::new(0.0, 2.0, 0.0), 0.8, 4);
        let u = unify(vec![vec![a0, b0], vec![a1, b1, solo]]);
        assert!(u.is_some(), "one pose-specific part must not sink the strip");
    }
}
