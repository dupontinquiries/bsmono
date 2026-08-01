//! descriptor — a shape fingerprint that survives being re-exported.
//!
//! ## Why vertex count is the wrong key
//!
//! Matching Brawl's tank to his robot worked on vertex count, but only because
//! those two models share an export lineage. Any decimation, triangulation
//! change, weld pass or re-export shifts the count and the match evaporates.
//! Counting vertices measures the *encoding*, not the shape.
//!
//! ## What survives instead: integrals over the surface
//!
//! Slide a box over the model's bounds with overlap, and inside each box sum the
//! triangle normals **weighted by area**. That sum is an integral of the normal
//! field over whatever surface is in the box, and an integral does not care how
//! the surface was cut up: split one triangle into fifty and the area-weighted
//! normals still sum to the same vector. Total area behaves the same way.
//!
//! So the fingerprint is a grid of (vector, area) cells — robust to
//! retessellation, and cheap. A test decimates a mesh into eight times as many
//! triangles and requires the descriptor to be unchanged, which is precisely the
//! property vertex count lacks.
//!
//! Overlapping boxes matter: a feature straddling a boundary would otherwise be
//! split between two cells and appear in neither, which is the standard failure
//! of grid descriptors. At 50% overlap every feature sits wholly inside some
//! cell.
//!
//! ## What this is for
//!
//! Two jobs. Matching a part between two models of one character, as the Brawl
//! work needs. And, further out, looking for the PATTERN across characters — if
//! the cells that become an arm look alike across twenty vehicles, that is the
//! rule for building an arm from a vehicle, learned rather than hand-written.

/// One cell of the fingerprint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// Area-weighted sum of triangle normals. Direction says which way the
    /// surface faces on balance; magnitude says how much of it agrees.
    pub normal_sum: [f64; 3],
    /// Total triangle area in the cell.
    pub area: f64,
}

impl Cell {
    pub const EMPTY: Cell = Cell { normal_sum: [0.0; 3], area: 0.0 };
    /// How "flat" the surface in this cell is: 1.0 when every triangle faces the
    /// same way, toward 0 when they cancel. A panel and a greeble of equal area
    /// are told apart by this, not by size.
    pub fn coherence(&self) -> f64 {
        if self.area <= 0.0 { return 0.0; }
        let m = (self.normal_sum[0].powi(2) + self.normal_sum[1].powi(2)
               + self.normal_sum[2].powi(2)).sqrt();
        (m / self.area).min(1.0)
    }
}

/// A grid fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub res: usize,
    pub cells: Vec<Cell>,
    pub total_area: f64,
}

fn tri_normal_area(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> ([f64; 3], f64) {
    let u = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
    let v = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
    let n = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
    let m = (n[0]*n[0] + n[1]*n[1] + n[2]*n[2]).sqrt();
    if m < 1e-18 { return ([0.0; 3], 0.0); }
    ([n[0]/m, n[1]/m, n[2]/m], m * 0.5)
}

/// Build the fingerprint. `res` cells per axis; boxes overlap by half a cell, so
/// a feature on a boundary still lands wholly inside some cell.
pub fn describe(verts: &[[f64; 3]], tris: &[u32], res: usize) -> Descriptor {
    let res = res.max(1);
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for p in verts { for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); } }
    let span = [ (hi[0]-lo[0]).max(1e-9), (hi[1]-lo[1]).max(1e-9), (hi[2]-lo[2]).max(1e-9) ];
    let mut cells = vec![Cell::EMPTY; res * res * res];
    let mut total = 0.0;
    for t in tris.chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if a >= verts.len() || b >= verts.len() || c >= verts.len() { continue; }
        let (n, ar) = tri_normal_area(verts[a], verts[b], verts[c]);
        if ar <= 0.0 { continue; }
        total += ar;
        // Bin by centroid — but SUBDIVIDE first, so a triangle never spans more
        // than a fraction of a cell.
        //
        // Binning a whole triangle by its centroid is not retessellation-
        // invariant: one big triangle dumps all its area into a single cell
        // while its subdivided twin spreads across many, and the first version
        // of this scored a distance of 5.5 between two identical surfaces.
        // Splitting until every piece is small relative to a cell makes coarse
        // and fine inputs converge on the same binning, which is the whole
        // property being claimed.
        let cell_size = span.iter().fold(0.0f64, |m, s| m.max(*s)) / res as f64;
        let limit = cell_size * 0.35;
        let mut stack = vec![(verts[a], verts[b], verts[c])];
        let mut guard = 0;
        while let Some((p, q, r)) = stack.pop() {
            guard += 1;
            if guard > 20000 { break; }   // pathological input: stop splitting
            let ext = (0..3).map(|i| {
                p[i].max(q[i]).max(r[i]) - p[i].min(q[i]).min(r[i])
            }).fold(0.0f64, f64::max);
            if ext > limit && guard < 20000 {
                // split at the midpoints into four similar triangles
                let m = |u: [f64;3], v: [f64;3]| [(u[0]+v[0])*0.5, (u[1]+v[1])*0.5, (u[2]+v[2])*0.5];
                let (mp, mq, mr) = (m(p,q), m(q,r), m(r,p));
                stack.push((p, mp, mr));
                stack.push((mp, q, mq));
                stack.push((mr, mq, r));
                stack.push((mp, mq, mr));
                continue;
            }
            let (nn, aa) = tri_normal_area(p, q, r);
            if aa <= 0.0 { continue; }
            let ctr = [ (p[0]+q[0]+r[0])/3.0, (p[1]+q[1]+r[1])/3.0, (p[2]+q[2]+r[2])/3.0 ];
            let f = [ (ctr[0]-lo[0])/span[0], (ctr[1]-lo[1])/span[1], (ctr[2]-lo[2])/span[2] ];
            for dx in 0..2 { for dy in 0..2 { for dz in 0..2 {
                let idx = |v: f64, d: usize| -> Option<usize> {
                    let g = v * res as f64 - 0.5 * d as f64;
                    if g < 0.0 { return None; }
                    let g = g as usize;
                    if g >= res { None } else { Some(g) }
                };
                if let (Some(ix), Some(iy), Some(iz)) = (idx(f[0], dx), idx(f[1], dy), idx(f[2], dz)) {
                    let k = (iz * res + iy) * res + ix;
                    let cell = &mut cells[k];
                    for i in 0..3 { cell.normal_sum[i] += nn[i] * aa; }
                    cell.area += aa;
                }
            }}}
        }
    }
    Descriptor { res, cells, total_area: total }
}

/// Distance between two fingerprints, 0 = identical.
///
/// Areas are normalised by each model's total, so a part matches its twin at a
/// different scale — the mistake that made the first Brawl run match nothing.
pub fn distance(a: &Descriptor, b: &Descriptor) -> f64 {
    if a.res != b.res { return f64::INFINITY; }
    let (na, nb) = (a.total_area.max(1e-12), b.total_area.max(1e-12));
    let mut acc = 0.0;
    for (x, y) in a.cells.iter().zip(&b.cells) {
        let da = x.area / na;
        let db = y.area / nb;
        acc += (da - db).abs();
        // direction disagreement, weighted by how much surface is there
        let w = da.min(db);
        if w > 0.0 {
            let ma = (x.normal_sum[0].powi(2)+x.normal_sum[1].powi(2)+x.normal_sum[2].powi(2)).sqrt().max(1e-12);
            let mb = (y.normal_sum[0].powi(2)+y.normal_sum[1].powi(2)+y.normal_sum[2].powi(2)).sqrt().max(1e-12);
            let dot: f64 = (0..3).map(|i| (x.normal_sum[i]/ma) * (y.normal_sum[i]/mb)).sum();
            acc += w * (1.0 - dot.clamp(-1.0, 1.0)) * 0.5;
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad as two triangles.
    fn quad() -> (Vec<[f64; 3]>, Vec<u32>) {
        (vec![[0.0,0.0,0.0],[1.0,0.0,0.0],[1.0,1.0,0.0],[0.0,1.0,0.0]],
         vec![0,1,2, 0,2,3])
    }

    /// The same quad cut into n x n cells — many more triangles, identical
    /// surface. This is what a re-export or a decimation pass does.
    fn quad_subdivided(n: usize) -> (Vec<[f64; 3]>, Vec<u32>) {
        let mut v = Vec::new();
        for j in 0..=n { for i in 0..=n {
            v.push([i as f64 / n as f64, j as f64 / n as f64, 0.0]);
        }}
        let mut t = Vec::new();
        let w = n + 1;
        for j in 0..n { for i in 0..n {
            let a = (j*w + i) as u32; let b = a + 1;
            let c = ((j+1)*w + i) as u32; let d = c + 1;
            t.extend([a, b, d, a, d, c]);
        }}
        (v, t)
    }

    #[test]
    fn the_fingerprint_survives_retessellation_where_vertex_count_does_not() {
        // THE property. Brawl matched on vertex count only because his two
        // models share an export lineage; any decimation breaks that. An
        // area-weighted normal sum is an INTEGRAL over the surface, and cutting
        // a triangle into fifty does not change an integral.
        let (v1, t1) = quad();
        let (v8, t8) = quad_subdivided(8);
        assert_eq!(v1.len(), 4);
        assert_eq!(v8.len(), 81, "20x the vertices");
        assert_eq!(t8.len() / 3, 128, "64x the triangles");

        let d1 = describe(&v1, &t1, 4);
        let d8 = describe(&v8, &t8, 4);
        assert!(distance(&d1, &d8) < 0.02,
                "same surface, different tessellation: distance {}", distance(&d1, &d8));
        assert!((d1.total_area - d8.total_area).abs() < 1e-9, "area is identical");
    }

    #[test]
    fn scale_does_not_break_a_match() {
        // The bug that made the first Brawl run match zero parts: the two models
        // are at different scales.
        let (v, t) = quad_subdivided(4);
        let big: Vec<[f64; 3]> = v.iter().map(|p| [p[0]*7.0, p[1]*7.0, p[2]*7.0]).collect();
        let a = describe(&v, &t, 4);
        let b = describe(&big, &t, 4);
        assert!(distance(&a, &b) < 0.02, "scale must not matter: {}", distance(&a, &b));
    }

    #[test]
    fn different_shapes_are_told_apart() {
        // A descriptor that matches everything is worse than none: it would
        // weld unrelated parts together and destroy geometry.
        let (vq, tq) = quad_subdivided(6);
        // an L-shape: same kind of surface, different distribution
        let mut vl = Vec::new();
        let mut tl = Vec::new();
        for (ox, oy) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0)] {
            let base = vl.len() as u32;
            vl.extend([[ox,oy,0.0],[ox+1.0,oy,0.0],[ox+1.0,oy+1.0,0.0],[ox,oy+1.0,0.0]]);
            tl.extend([base, base+1, base+2, base, base+2, base+3]);
        }
        let a = describe(&vq, &tq, 4);
        let b = describe(&vl, &tl, 4);
        assert!(distance(&a, &b) > 0.15, "an L is not a square: {}", distance(&a, &b));
    }

    #[test]
    fn coherence_separates_a_panel_from_a_greeble() {
        // Equal area, wildly different character: a flat panel's normals all
        // agree; a cluster of detail's cancel out.
        let (vp, tp) = quad_subdivided(4);
        let flat = describe(&vp, &tp, 1);
        assert!(flat.cells[0].coherence() > 0.98, "a flat panel is coherent");

        // a closed-ish box: normals point every way and largely cancel
        let mut vb = Vec::new();
        let mut tb = Vec::new();
        for (n, o) in [([0.0,0.0,1.0], 0.0), ([0.0,0.0,-1.0], 1.0)] {
            let base = vb.len() as u32;
            let z = o;
            vb.extend([[0.0,0.0,z],[1.0,0.0,z],[1.0,1.0,z],[0.0,1.0,z]]);
            if n[2] > 0.0 { tb.extend([base, base+1, base+2, base, base+2, base+3]); }
            else { tb.extend([base, base+2, base+1, base, base+3, base+2]); }
        }
        let box_ = describe(&vb, &tb, 1);
        assert!(box_.cells[0].coherence() < 0.2,
                "opposing faces cancel: {}", box_.cells[0].coherence());
    }

    #[test]
    fn an_empty_or_degenerate_mesh_does_not_panic() {
        let d = describe(&[], &[], 4);
        assert_eq!(d.total_area, 0.0);
        assert_eq!(distance(&d, &d), 0.0);
        // degenerate triangles carry no area and are skipped rather than
        // producing NaN normals
        let v = vec![[0.0;3], [0.0;3], [0.0;3]];
        let d2 = describe(&v, &[0,1,2], 2);
        assert_eq!(d2.total_area, 0.0);
        assert!(d2.cells.iter().all(|c| c.area == 0.0));
    }

    #[test]
    fn a_feature_on_a_cell_boundary_is_not_orphaned() {
        // The standard failure of grid descriptors: a feature straddling a seam
        // is split between two cells and dominates neither.
        let n = 8;
        let (v, t) = quad_subdivided(n);
        let d = describe(&v, &t, 2);
        let occupied = d.cells.iter().filter(|c| c.area > 0.0).count();
        assert!(occupied >= 4, "the sheet spans the grid, got {occupied} occupied cells");
        let sum: f64 = d.cells.iter().map(|c| c.area).sum();
        assert!(sum >= d.total_area, "overlap means cells double-count deliberately");
    }
}
