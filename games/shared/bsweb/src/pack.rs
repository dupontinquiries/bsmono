//! pack — build a web, in Rust.
//!
//! The crate header says the Python side builds the web and "the engines could
//! not read a byte of it". Reading is solved; this is the other half. A game
//! that generates or imports geometry at build time can now emit the same
//! format its own reader takes, which is what makes the web a pipeline rather
//! than an import of somebody else's artefact.
//!
//! ## What the encoder does, and what each step is worth
//!
//! Measured on clawbound's seventeen static animal meshes — 413,307 vertices,
//! 480,083 triangles, 19.0 MB of IQM — every stage cumulative, one zstd -19
//! pass over the result:
//!
//! | stage | bytes |
//! |---|---|
//! | IQM on disk | 19.0 MB |
//! | positions + indices only (what the loader reads) | 10.7 MB |
//! | + 14-bit quantisation per part box | 8.2 MB |
//! | + Morton reorder, planar deltas, varints, zstd | **2.57 MB** |
//!
//! 1. **Quantise** to `bits` per axis against the part's own box. The box
//!    travels in the index as `lo`/`span`, so precision is per part rather
//!    than per scene: a 2-metre cat and a 40-metre tree each get the full
//!    ladder. At 14 bits the worst round-trip error over all seventeen meshes
//!    is 0.0031% of the part's own span.
//! 2. **Morton-order the vertices** on the quantised lattice. This is the step
//!    that pays for the deltas: 3.86 MB unordered against 3.20 MB ordered on
//!    the same bytes, because a spatial walk makes consecutive positions
//!    neighbours. Indices are remapped to follow, so the mesh is unchanged —
//!    vertex order is not semantic in an indexed mesh, which is exactly why
//!    this is free.
//! 3. **Weld** vertices that land on the same lattice point. Honest result:
//!    this finds almost nothing here (413,307 -> 413,238 at 14 bits) because
//!    these meshes are already welded in float space. It is kept because it is
//!    cheap, it makes the format's vertex count mean "distinct positions", and
//!    a coarser `bits` makes it start paying (0.2% at 12 bits).
//! 4. **Plane the positions** into three delta streams and varint them
//!    ([`crate::PMODE_PLANAR`]): 21% off the fixed-width layout.
//! 5. **Canonicalise triangles** — rotate each to lowest-index-first, sort,
//!    drop exact duplicates and any triangle the weld made degenerate — then
//!    delta-varint the indices. Sorting is what makes the deltas small; the
//!    duplicate drop is only 0.4% here but it is a correctness win too, since
//!    a welded mesh can produce genuinely identical triangles.
//!
//! Compression itself is left to the caller, deliberately and for the same
//! reason the reader leaves decompression out: an engine has a compressor or a
//! build script that does. `zstd -19` on the output is what the measurements
//! above assume.

use crate::{write_svarint, PMODE_PLANAR};

/// One part to pack: a name, a triangle-indexed position mesh in any space.
pub struct PartMesh<'a> {
    /// Becomes the part's `id`, and how a game looks it up.
    pub id: &'a str,
    /// Becomes `name` — the category a slot picker filters on.
    pub category: Option<&'a str>,
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u32; 3]],
}

/// How hard to squeeze. `bits` is the quantisation ladder per axis.
#[derive(Clone, Copy, Debug)]
pub struct PackOpts {
    pub bits: u8,
    /// Morton-order vertices before delta coding. Off only for measuring what
    /// the ordering is worth — there is no correctness reason to disable it.
    pub reorder: bool,
    /// Merge vertices that quantise onto the same lattice point.
    pub weld: bool,
}

impl Default for PackOpts {
    /// 14 bits: the knee of the curve on real meshes. 16 bits costs 11% more
    /// bytes for precision already an order of magnitude finer than a flat-shaded
    /// facet can show; 12 bits saves another 13% but starts moving vertices far
    /// enough to be arguable, and this default has to be safe for meshes nobody
    /// measured. Worst error at 14 bits across the clawbound set: 0.0031% of a
    /// part's own span — on a 3-unit statue, a tenth of a millimetre.
    fn default() -> PackOpts {
        PackOpts { bits: 14, reorder: true, weld: true }
    }
}

/// What packing a part produced, for callers that want to report or test it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartStats {
    pub verts_in: usize,
    pub verts_out: usize,
    pub tris_in: usize,
    pub tris_out: usize,
    pub vertex_bytes: usize,
    pub index_bytes: usize,
    /// Worst round-trip position error, as a fraction of the part's own span.
    pub max_rel_error: f32,
}

/// Interleave three lattice coordinates into a Morton code (bits from the top,
/// so the walk is a spatial z-order rather than an axis-major sweep).
fn morton(x: u32, y: u32, z: u32) -> u64 {
    fn spread(v: u32) -> u64 {
        let mut v = (v & 0x1f_ffff) as u64;
        v = (v | v << 32) & 0x001f_0000_0000_ffff;
        v = (v | v << 16) & 0x001f_0000_ff00_00ff;
        v = (v | v << 8) & 0x100f_00f0_0f00_f00f;
        v = (v | v << 4) & 0x10c3_0c30_c30c_30c3;
        v = (v | v << 2) & 0x1249_2492_4924_9249;
        v
    }
    spread(x) | spread(y) << 1 | spread(z) << 2
}

/// Pack one part into (index JSON fragment, vertex bytes, index bytes, stats).
fn pack_part(part: &PartMesh, geom: usize, o: PackOpts) -> (String, Vec<u8>, Vec<u8>, PartStats) {
    let bits = o.bits.clamp(1, 16) as u32;
    let maxq = ((1u32 << bits) - 1) as f32;
    let n = part.positions.len();

    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in part.positions {
        for d in 0..3 {
            lo[d] = lo[d].min(p[d]);
            hi[d] = hi[d].max(p[d]);
        }
    }
    if n == 0 {
        lo = [0.0; 3];
        hi = [0.0; 3];
    }
    // A degenerate axis (a perfectly flat part) gets a nonzero span so the
    // quantiser divides by something; every value on that axis lands on 0 and
    // decodes back to `lo`, which is exactly where they all were.
    let span = [
        (hi[0] - lo[0]).max(1e-9),
        (hi[1] - lo[1]).max(1e-9),
        (hi[2] - lo[2]).max(1e-9),
    ];

    let quant = |p: &[f32; 3]| -> [u32; 3] {
        let mut q = [0u32; 3];
        for d in 0..3 {
            let t = ((p[d] - lo[d]) / span[d] * maxq).round();
            q[d] = t.clamp(0.0, maxq) as u32;
        }
        q
    };
    let qs: Vec<[u32; 3]> = part.positions.iter().map(quant).collect();

    // weld onto the lattice
    let mut uniq: Vec<[u32; 3]> = Vec::with_capacity(n);
    let mut remap: Vec<u32> = Vec::with_capacity(n);
    if o.weld {
        use std::collections::HashMap;
        let mut seen: HashMap<[u32; 3], u32> = HashMap::with_capacity(n);
        for q in &qs {
            let id = *seen.entry(*q).or_insert_with(|| {
                uniq.push(*q);
                (uniq.len() - 1) as u32
            });
            remap.push(id);
        }
    } else {
        uniq.extend_from_slice(&qs);
        remap.extend(0..n as u32);
    }

    // Morton reorder — vertex order is not semantic in an indexed mesh
    if o.reorder {
        let mut order: Vec<u32> = (0..uniq.len() as u32).collect();
        order.sort_by_key(|&i| {
            let q = uniq[i as usize];
            // shift so the code walks cells rather than single lattice steps;
            // measured indifferent between shifts 2 and 5, worse at 8
            morton(q[0] >> 5, q[1] >> 5, q[2] >> 5)
        });
        let mut inv = vec![0u32; uniq.len()];
        for (new, &old) in order.iter().enumerate() {
            inv[old as usize] = new as u32;
        }
        uniq = order.iter().map(|&i| uniq[i as usize]).collect();
        for r in remap.iter_mut() {
            *r = inv[*r as usize];
        }
    }

    // planar delta varints
    let mut vb = Vec::new();
    for axis in 0..3 {
        let mut cur: i64 = 0;
        for q in &uniq {
            let v = q[axis] as i64;
            write_svarint(&mut vb, v - cur);
            cur = v;
        }
    }

    // triangles: canonicalise, sort, dedup, delta
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(part.tris.len());
    for t in part.tris {
        let (a, b, c) = (remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]);
        if a == b || b == c || a == c {
            continue; // degenerate, or made so by the weld
        }
        // rotate to lowest-first: preserves winding, so the facet normal the
        // renderer derives from it is unchanged
        let rot = if a <= b && a <= c { [a, b, c] } else if b <= c { [b, c, a] } else { [c, a, b] };
        tris.push(rot);
    }
    tris.sort_unstable();
    tris.dedup();
    let mut ib = Vec::new();
    let mut cur: i64 = 0;
    for t in &tris {
        for &v in t {
            write_svarint(&mut ib, v as i64 - cur);
            cur = v as i64;
        }
    }

    // worst round-trip error, in units of the part's own span
    let mut max_rel = 0.0f32;
    for (i, p) in part.positions.iter().enumerate() {
        let q = uniq[remap[i] as usize];
        for d in 0..3 {
            let back = lo[d] + q[d] as f32 / maxq * span[d];
            max_rel = max_rel.max((back - p[d]).abs() / span[d]);
        }
    }

    let cat = part
        .category
        .map(|c| format!(",\"name\":\"{c}\""))
        .unwrap_or_default();
    let frag = format!(
        "{{\"id\":\"{}\"{},\"geom\":{},\"nv\":{},\"nt\":{},\"isize\":0,\
         \"lo\":[{:?},{:?},{:?}],\"span\":[{:?},{:?},{:?}],\"instances\":1,\"size\":{:?}}}",
        part.id,
        cat,
        geom,
        uniq.len(),
        tris.len(),
        lo[0],
        lo[1],
        lo[2],
        span[0],
        span[1],
        span[2],
        span[0].max(span[1]).max(span[2]),
    );
    let stats = PartStats {
        verts_in: n,
        verts_out: uniq.len(),
        tris_in: part.tris.len(),
        tris_out: tris.len(),
        vertex_bytes: vb.len(),
        index_bytes: ib.len(),
        max_rel_error: max_rel,
    };
    (frag, vb, ib, stats)
}

/// Build a complete web. Parts are packed in the order given — the caller
/// controls that order, and it matters: grouping parts that resemble each other
/// puts similar bytes near each other for the compressor that wraps this.
pub fn pack(parts: &[PartMesh], o: PackOpts) -> (Vec<u8>, Vec<PartStats>) {
    let mut frags = Vec::new();
    let mut geoms = Vec::new();
    let mut payload = Vec::new();
    let mut stats = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let (frag, vb, ib, st) = pack_part(p, i, o);
        frags.push(frag);
        geoms.push(format!("{{\"v\":{},\"i\":{}}}", vb.len(), ib.len()));
        payload.extend_from_slice(&vb);
        payload.extend_from_slice(&ib);
        stats.push(st);
    }
    let idx = format!(
        "{{\"version\":1,\"bits\":{},\"delta\":true,\"pmode\":{},\"parts\":[{}],\"geom\":[{}]}}",
        o.bits.clamp(1, 16),
        PMODE_PLANAR,
        frags.join(","),
        geoms.join(","),
    );
    let mut out = Vec::with_capacity(idx.len() + payload.len() + 16);
    out.extend_from_slice(b"BSWEB\x01");
    out.extend_from_slice(&(idx.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(idx.as_bytes());
    out.extend_from_slice(&payload);
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Web;

    /// A shape with enough vertices to exercise ordering, and no symmetry that
    /// could hide a coordinate swap.
    fn wedge() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut pos = Vec::new();
        for i in 0..40u32 {
            let f = i as f32 / 39.0;
            pos.push([f * 3.0, (f * 7.0).sin(), f * f * 2.0 - 0.5]);
        }
        let tris: Vec<[u32; 3]> = (0..38u32).map(|i| [i, i + 1, i + 2]).collect();
        (pos, tris)
    }

    #[test]
    fn a_packed_part_round_trips_through_the_reader() {
        let (pos, tris) = wedge();
        let (bytes, stats) = pack(
            &[PartMesh { id: "wedge", category: Some("test"), positions: &pos, tris: &tris }],
            PackOpts::default(),
        );
        let w = Web::parse(&bytes).expect("the reader takes what the packer makes");
        assert_eq!(w.pmode, PMODE_PLANAR);
        assert_eq!(w.bits, 14);
        let p = &w.parts[0];
        assert_eq!(p.id, "wedge");
        assert_eq!(p.name.as_deref(), Some("test"));
        let v = w.vertices(p).expect("vertices");
        let t = w.triangles(p).expect("triangles");
        assert_eq!(v.len(), stats[0].verts_out);
        assert_eq!(t.len(), stats[0].tris_out);
        // every original position survives to within the quantiser's step
        let span = p.span[0].max(p.span[1]).max(p.span[2]);
        for orig in &pos {
            let best = v
                .iter()
                .map(|d| {
                    (0..3).map(|k| (d[k] - orig[k]).abs()).fold(0.0f32, f32::max)
                })
                .fold(f32::MAX, f32::min);
            assert!(best < span * 1e-4, "position lost: {orig:?} nearest off by {best}");
        }
        assert!(stats[0].max_rel_error < 1e-4, "error {}", stats[0].max_rel_error);
    }

    #[test]
    fn triangles_survive_reorder_welding_and_winding() {
        let (pos, tris) = wedge();
        let (bytes, _) = pack(
            &[PartMesh { id: "w", category: None, positions: &pos, tris: &tris }],
            PackOpts::default(),
        );
        let w = Web::parse(&bytes).unwrap();
        let p = &w.parts[0];
        let v = w.vertices(p).unwrap();
        let got = w.triangles(p).unwrap();
        assert_eq!(got.len(), tris.len(), "no triangle lost");
        // Compare geometrically, not by index: the packer is free to renumber.
        // A triangle's identity here is its three positions AND its winding,
        // because the renderer derives the facet normal from the winding.
        let centroid = |t: &[u32; 3], pts: &[[f32; 3]]| {
            let mut c = [0.0f32; 3];
            for &i in t {
                for d in 0..3 {
                    c[d] += pts[i as usize][d] / 3.0;
                }
            }
            c
        };
        let normal = |t: &[u32; 3], pts: &[[f32; 3]]| {
            let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let vv = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            [u[1] * vv[2] - u[2] * vv[1], u[2] * vv[0] - u[0] * vv[2], u[0] * vv[1] - u[1] * vv[0]]
        };
        for t in &tris {
            let (wc, wn) = (centroid(t, &pos), normal(t, &pos));
            let hit = got.iter().any(|g| {
                let (gc, gn) = (centroid(g, &v), normal(g, &v));
                let dc = (0..3).map(|d| (gc[d] - wc[d]).abs()).fold(0.0f32, f32::max);
                let dot = (0..3).map(|d| gn[d] * wn[d]).sum::<f32>();
                dc < 0.01 && dot > 0.0
            });
            assert!(hit, "triangle {t:?} missing or flipped");
        }
    }

    #[test]
    fn the_morton_reorder_is_what_makes_the_deltas_small() {
        // The claim in the module header, as a test — on the case that actually
        // occurs. An imported mesh's vertex order is whatever the exporter felt
        // like; `wedge()` above is a parametric curve whose natural order is
        // already a spatial walk, and on THAT the reorder is worth nothing
        // (measured: 238 bytes ordered against 235 unordered). The reorder pays
        // when the order is arbitrary, which is why it is on by default.
        let mut pos = Vec::new();
        let (mut a, mut b, mut c) = (1u32, 7u32, 13u32);
        for _ in 0..600 {
            a = a.wrapping_mul(1664525).wrapping_add(1013904223);
            b = b.wrapping_mul(22695477).wrapping_add(1);
            c = c.wrapping_mul(69069).wrapping_add(12345);
            let f = |v: u32| (v >> 8) as f32 / 65536.0;
            pos.push([f(a), f(b), f(c)]);
        }
        let tris: Vec<[u32; 3]> = (0..pos.len() as u32 - 2).map(|i| [i, i + 1, i + 2]).collect();
        let mesh = |o| {
            pack(&[PartMesh { id: "w", category: None, positions: &pos, tris: &tris }], o).1[0]
        };
        let ordered = mesh(PackOpts::default());
        let shuffled = mesh(PackOpts { reorder: false, ..PackOpts::default() });
        assert!(
            ordered.vertex_bytes < shuffled.vertex_bytes,
            "spatial order must shrink the position deltas: {} vs {}",
            ordered.vertex_bytes,
            shuffled.vertex_bytes
        );
    }

    #[test]
    fn a_coarser_ladder_welds_and_a_finer_one_does_not() {
        // Welding is honest about being nearly a no-op at fine precision; it
        // earns its place as the ladder coarsens.
        let (pos, tris) = wedge();
        let at = |bits| {
            pack(
                &[PartMesh { id: "w", category: None, positions: &pos, tris: &tris }],
                PackOpts { bits, ..PackOpts::default() },
            )
            .1[0]
        };
        assert_eq!(at(14).verts_out, pos.len(), "nothing to weld at 14 bits");
        assert!(at(4).verts_out < pos.len(), "a 4-bit lattice must collapse vertices");
        assert!(at(4).tris_out < tris.len(), "and the triangles that collapse with them");
    }

    #[test]
    fn a_flat_part_does_not_divide_by_zero() {
        let pos = vec![[0.0, 2.0, 0.0], [1.0, 2.0, 0.0], [1.0, 2.0, 1.0], [0.0, 2.0, 1.0]];
        let tris = vec![[0u32, 1, 2], [0, 2, 3]];
        let (bytes, st) = pack(
            &[PartMesh { id: "flat", category: None, positions: &pos, tris: &tris }],
            PackOpts::default(),
        );
        assert!(st[0].max_rel_error.is_finite());
        let w = Web::parse(&bytes).unwrap();
        let v = w.vertices(&w.parts[0]).unwrap();
        for p in &v {
            assert!((p[1] - 2.0).abs() < 1e-6, "the degenerate axis must decode back exactly");
        }
    }

    #[test]
    fn packing_is_deterministic() {
        let (pos, tris) = wedge();
        let run = || {
            pack(&[PartMesh { id: "w", category: None, positions: &pos, tris: &tris }], PackOpts::default()).0
        };
        assert_eq!(run(), run(), "same input, same bytes — or the build is not reproducible");
    }

    #[test]
    fn several_parts_keep_their_own_boxes() {
        // The point of per-part quantisation: a small part beside a huge one
        // does not lose precision to its neighbour's extent.
        let small: Vec<[f32; 3]> = (0..8).map(|i| [i as f32 * 0.001, 0.0, 0.0]).collect();
        let big: Vec<[f32; 3]> = (0..8).map(|i| [i as f32 * 100.0, 0.0, 0.0]).collect();
        let t = vec![[0u32, 1, 2]];
        let (bytes, _) = pack(
            &[
                PartMesh { id: "small", category: None, positions: &small, tris: &t },
                PartMesh { id: "big", category: None, positions: &big, tris: &t },
            ],
            PackOpts::default(),
        );
        let w = Web::parse(&bytes).unwrap();
        let sv = w.vertices(&w.parts[0]).unwrap();
        assert!((sv[7][0] - 0.007).abs() < 1e-6, "small part kept its own ladder: {:?}", sv[7]);
        let bv = w.vertices(&w.parts[1]).unwrap();
        assert!((bv[7][0] - 700.0).abs() < 0.1);
    }
}
