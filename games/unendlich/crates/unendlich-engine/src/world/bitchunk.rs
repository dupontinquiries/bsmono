//! The chunk as columns of bits.
//!
//! Occupancy of a 32³ chunk as three axis-major sets of 1 024 `u32`
//! columns: `y_cols[x][z]` has bit `y` set when `(x, y, z)` is solid,
//! `x_cols[y][z]` bit `x`, `z_cols[x][y]` bit `z`. 12 KB total. With
//! that, "which cells have an exposed +Y face" for a whole column is
//! `solid & !(solid >> 1)` — 32 cells per instruction — and the same
//! for the other five directions on the other two sets. Greedy merging
//! of the exposed faces into quads runs on the resulting masks with bit
//! tricks. This is the representation
//! `docs/claude-notes/chunk-window-design.md` puts under the mesher and
//! the GPU upload in place of the octree.
//!
//! Headless core, no engine dependency: solids come in as a closure or
//! a dense `[bool]`/predicate over `(x, y, z)`, neighbour columns as
//! plain `u32`s. Tested standalone against a brute-force six-neighbour
//! scan on random chunks.

#![allow(dead_code)]

pub const N: usize = 32;
pub const NN: usize = N * N;

/// Six face directions, in the order the masks are produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Face { PosX = 0, NegX = 1, PosY = 2, NegY = 3, PosZ = 4, NegZ = 5 }

pub const FACES: [Face; 6] = [Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ];

/// Occupancy of one chunk, three ways.
#[derive(Clone, PartialEq, Eq)]
pub struct BitChunk {
    /// `y_cols[x * N + z]`: bit y.
    pub y_cols: [u32; NN],
    /// `x_cols[y * N + z]`: bit x.
    pub x_cols: [u32; NN],
    /// `z_cols[x * N + y]`: bit z.
    pub z_cols: [u32; NN],
}

impl Default for BitChunk {
    fn default() -> Self { Self::empty() }
}

/// The one column-per-(u, v) view a neighbour contributes for each
/// face: the row of cells that touches this chunk. For `PosY` the
/// neighbour above contributes its `y == 0` layer as a 32×32 bit set,
/// which in column form is "bit 0 of each `y_cols`"; we take that layer
/// already flattened to `[u32; N]` rows for the same axis order as the
/// face mask it culls. `None` = open boundary (treat as air).
#[derive(Clone, Copy)]
pub struct NeighbourLayer(pub Option<[u32; N]>);

impl BitChunk {
    pub fn empty() -> Self {
        Self { y_cols: [0; NN], x_cols: [0; NN], z_cols: [0; NN] }
    }

    pub fn full() -> Self {
        Self { y_cols: [u32::MAX; NN], x_cols: [u32::MAX; NN], z_cols: [u32::MAX; NN] }
    }

    /// Build from a predicate. O(32³) once, at chunk install; after
    /// that every query is on the columns.
    pub fn from_fn(mut solid: impl FnMut(usize, usize, usize) -> bool) -> Self {
        let mut c = Self::empty();
        for x in 0..N { for y in 0..N { for z in 0..N {
            if solid(x, y, z) { c.set(x, y, z); }
        }}}
        c
    }

    /// Build from a dense `x*N*N + y*N + z`-ordered occupancy.
    pub fn from_dense_xyz(dense: &[bool]) -> Self {
        assert_eq!(dense.len(), N * N * N);
        Self::from_fn(|x, y, z| dense[(x * N + y) * N + z])
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize) {
        self.y_cols[x * N + z] |= 1 << y;
        self.x_cols[y * N + z] |= 1 << x;
        self.z_cols[x * N + y] |= 1 << z;
    }

    #[inline]
    pub fn clear(&mut self, x: usize, y: usize, z: usize) {
        self.y_cols[x * N + z] &= !(1 << y);
        self.x_cols[y * N + z] &= !(1 << x);
        self.z_cols[x * N + y] &= !(1 << z);
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        self.y_cols[x * N + z] >> y & 1 == 1
    }

    /// All 32 768 solid, or none. Uniform chunks allocate nothing
    /// downstream (window design, "most slots are uniform").
    pub fn is_uniform(&self) -> Option<bool> {
        if self.y_cols.iter().all(|&c| c == 0) { Some(false) }
        else if self.y_cols.iter().all(|&c| c == u32::MAX) { Some(true) }
        else { None }
    }

    pub fn solid_count(&self) -> u32 {
        self.y_cols.iter().map(|c| c.count_ones()).sum()
    }

    /// The boundary layer this chunk shows to its neighbour on `face`, as
    /// 32 rows of 32 bits in the layout `face_masks` expects from a
    /// neighbour: for ±X a row per `y` with bit `z`; for ±Y a row per
    /// `x` with bit `z`; for ±Z a row per `x` with bit `y`.
    pub fn boundary_layer(&self, face: Face) -> [u32; N] {
        let mut out = [0u32; N];
        match face {
            Face::PosX | Face::NegX => {
                let bit = if face == Face::PosX { N - 1 } else { 0 };
                for y in 0..N { for z in 0..N {
                    if self.x_cols[y * N + z] >> bit & 1 == 1 { out[y] |= 1 << z; }
                }}
            }
            Face::PosY | Face::NegY => {
                let bit = if face == Face::PosY { N - 1 } else { 0 };
                for x in 0..N { for z in 0..N {
                    if self.y_cols[x * N + z] >> bit & 1 == 1 { out[x] |= 1 << z; }
                }}
            }
            Face::PosZ | Face::NegZ => {
                let bit = if face == Face::PosZ { N - 1 } else { 0 };
                for x in 0..N { for y in 0..N {
                    if self.z_cols[x * N + y] >> bit & 1 == 1 { out[x] |= 1 << y; }
                }}
            }
        }
        out
    }

    /// Exposed faces for one direction, as 32 columns of 32 bits along
    /// the face's axis — i.e. the same column layout as the axis set,
    /// with a bit set where a solid cell has air (or the neighbour's air)
    /// on that side. `neighbour` is the touching layer of the chunk
    /// beyond the boundary (`boundary_layer(opposite face)` of that
    /// chunk), `None` for open.
    ///
    /// Layouts: ±X → `[u32; NN]` indexed `y * N + z`, bit x.
    ///          ±Y → indexed `x * N + z`, bit y.
    ///          ±Z → indexed `x * N + y`, bit z.
    pub fn face_mask(&self, face: Face, neighbour: NeighbourLayer) -> [u32; NN] {
        face_mask_between(self, self, face, neighbour)
    }

    /// Union of two occupancy sets (per column OR), e.g. "opaque or glass".
    pub fn union(&self, o: &BitChunk) -> BitChunk {
        let mut r = self.clone();
        for i in 0..NN { r.y_cols[i] |= o.y_cols[i]; r.x_cols[i] |= o.x_cols[i]; r.z_cols[i] |= o.z_cols[i]; }
        r
    }

    /// Boundary layer of `self ∪ o` without materialising the union.
    pub fn boundary_layer_union(&self, o: &BitChunk, face: Face) -> [u32; N] {
        let a = self.boundary_layer(face);
        let b = o.boundary_layer(face);
        let mut out = [0u32; N];
        for i in 0..N { out[i] = a[i] | b[i]; }
        out
    }

    #[inline]
    pub fn cols(&self, face: Face) -> &[u32; NN] {
        match face {
            Face::PosX | Face::NegX => &self.x_cols,
            Face::PosY | Face::NegY => &self.y_cols,
            Face::PosZ | Face::NegZ => &self.z_cols,
        }
    }
}

/// Exposed faces of the cells in `inner` whose neighbour on `face` is not
/// in `blocker`. `face_mask` is the `inner == blocker` case. The mesher
/// uses `inner = opaque, blocker = opaque` for solid cubes and
/// `inner = glass, blocker = opaque ∪ glass` for a transparent type that
/// does not draw faces against itself (the engine's
/// `produces_face_against` rule).
pub fn face_mask_between(inner: &BitChunk, blocker: &BitChunk, face: Face, neighbour: NeighbourLayer) -> [u32; NN] {
    let positive = matches!(face, Face::PosX | Face::PosY | Face::PosZ);
    let icols = inner.cols(face);
    let bcols = blocker.cols(face);
    {
        let mut out = [0u32; NN];
        for i in 0..NN {
            let solid = bcols[i];
            // Which (row, bit-in-row) of the neighbour layer covers this
            // column's boundary cell: the column index `i` is `a * N + b`
            // in each layout and the layer row is `a`, bit `b`.
            let (a, b) = (i / N, i % N);
            let beyond_solid = match neighbour.0 {
                Some(layer) => layer[a] >> b & 1 == 1,
                None => false,
            };
            let exposed = if positive {
                // Cell at bit k is exposed if bit k+1 is not a blocker;
                // bit 31's beyond is the neighbour layer.
                let free_above = !(solid >> 1) & !(1 << (N - 1)) | if beyond_solid { 0 } else { 1 << (N - 1) };
                icols[i] & free_above
            } else {
                let free_below = !(solid << 1) & !1 | if beyond_solid { 0 } else { 1 };
                icols[i] & free_below
            };
            out[i] = exposed;
        }
        out
    }
}

impl BitChunk {

    /// Total exposed faces over all six directions.
    pub fn exposed_face_count(&self, neighbours: [NeighbourLayer; 6]) -> u32 {
        FACES.iter().zip(neighbours.iter())
            .map(|(&f, &n)| self.face_mask(f, n).iter().map(|m| m.count_ones()).sum::<u32>())
            .sum()
    }
}

/// Visit every set bit of a face mask as a cell `(x, y, z)` — used for
/// cells that get a special (non-merged) emit.
pub fn for_each_cell(mask: &[u32; NN], face: Face, mut f: impl FnMut(usize, usize, usize)) {
    for i in 0..NN {
        let mut m = mask[i];
        let (a, b) = (i / N, i % N);
        while m != 0 {
            let s = m.trailing_zeros() as usize;
            m &= m - 1;
            match face {
                Face::PosX | Face::NegX => f(s, a, b),
                Face::PosY | Face::NegY => f(a, s, b),
                Face::PosZ | Face::NegZ => f(a, b, s),
            }
        }
    }
}

/// One merged rectangle of faces on one slice, in slice-local (u, v)
/// with extents (w, h). `slice` is the coordinate along the face axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quad {
    pub slice: u8,
    pub u: u8,
    pub v: u8,
    pub w: u8,
    pub h: u8,
}

/// Greedy-merge a 32×32 bit slice into rectangles. `rows[u]` has bit `v`
/// set for a face at (u, v); a `key` per cell (block id, AO class…)
/// breaks merges: two cells merge only if `key(u, v)` is equal. Standard
/// binary greedy: for each set bit, extend along v by the run of set
/// bits with equal key, then extend along u while the whole run matches.
///
/// Faces per quad == the quad's area, so the sum of areas over all quads
/// equals the popcount of the slice — the parity the test pins.
pub fn greedy_slice(mut rows: [u32; N], slice: u8, mut key: impl FnMut(usize, usize) -> u32, out: &mut Vec<Quad>) {
    for u in 0..N {
        while rows[u] != 0 {
            let v0 = rows[u].trailing_zeros() as usize;
            let k = key(u, v0);
            // Run along v with equal key.
            let mut h = 1usize;
            while v0 + h < N && rows[u] >> (v0 + h) & 1 == 1 && key(u, v0 + h) == k { h += 1; }
            let run_mask: u32 = if h == N { u32::MAX } else { ((1u32 << h) - 1) << v0 };
            // Extend along u while the full run is present with equal keys.
            let mut w = 1usize;
            'grow: while u + w < N {
                if rows[u + w] & run_mask != run_mask { break; }
                for v in v0..v0 + h { if key(u + w, v) != k { break 'grow; } }
                w += 1;
            }
            for uu in u..u + w { rows[uu] &= !run_mask; }
            out.push(Quad { slice, u: u as u8, v: v0 as u8, w: w as u8, h: h as u8 });
        }
    }
}

/// Quads for one face direction of a chunk: transposes the face mask
/// (columns along the face axis) into per-slice 32×32 bit sets and
/// greedy-merges each. `key(x, y, z)` breaks merges (block id etc.).
///
/// Slice/u/v axes per face: ±X → slice x, u y, v z; ±Y → slice y, u x,
/// v z; ±Z → slice z, u x, v y.
pub fn greedy_faces(chunk: &BitChunk, face: Face, neighbour: NeighbourLayer,
                    key: impl FnMut(usize, usize, usize) -> u32, out: &mut Vec<Quad>) {
    let mask = chunk.face_mask(face, neighbour);
    greedy_mask(&mask, face, key, out)
}

/// Greedy-merge an already-computed face mask (see `face_mask_between`).
/// Same axis conventions as `greedy_faces`.
pub fn greedy_mask(mask: &[u32; NN], face: Face,
                   mut key: impl FnMut(usize, usize, usize) -> u32, out: &mut Vec<Quad>) {
    // Build per-slice rows: slices[s][u] bit v. Column i = a*N + b with
    // bit s, where (a, b) are (u, v) in every layout (see face_mask).
    let mut slices = vec![[0u32; N]; N];
    for i in 0..NN {
        let mut m = mask[i];
        let (a, b) = (i / N, i % N);
        while m != 0 {
            let s = m.trailing_zeros() as usize;
            m &= m - 1;
            slices[s][a] |= 1 << b;
        }
    }
    for s in 0..N {
        if slices[s].iter().all(|&r| r == 0) { continue; }
        let mut k = |u: usize, v: usize| match face {
            Face::PosX | Face::NegX => key(s, u, v),
            Face::PosY | Face::NegY => key(u, s, v),
            Face::PosZ | Face::NegZ => key(u, v, s),
        };
        greedy_slice(slices[s], s as u8, &mut k, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift so tests are reproducible without a crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x }
        fn chance(&mut self, p_num: u64, p_den: u64) -> bool { self.next() % p_den < p_num }
    }

    fn random_chunk(seed: u64, num: u64, den: u64) -> (BitChunk, Vec<bool>) {
        let mut r = Rng(seed | 1);
        let mut dense = vec![false; N * N * N];
        for v in dense.iter_mut() { *v = r.chance(num, den); }
        (BitChunk::from_dense_xyz(&dense), dense)
    }

    /// Terrain-ish: a heightfield, so most faces are +Y and the sides
    /// are long — the greedy merger's real workload.
    fn terrain_chunk(seed: u64) -> (BitChunk, Vec<bool>) {
        let mut r = Rng(seed | 1);
        let mut h = [[0usize; N]; N];
        for x in 0..N { for z in 0..N { h[x][z] = 8 + ((x * 3 + z * 5 + (r.next() % 3) as usize) % 17); } }
        let mut dense = vec![false; N * N * N];
        for x in 0..N { for y in 0..N { for z in 0..N { dense[(x * N + y) * N + z] = y < h[x][z]; } } }
        (BitChunk::from_dense_xyz(&dense), dense)
    }

    fn brute_faces(dense: &[bool], neigh: &[Option<Vec<bool>>; 6]) -> u32 {
        // neigh[f] is the full dense neighbour chunk beyond face f, or None.
        let at = |d: &[bool], x: i32, y: i32, z: i32| d[((x as usize) * N + y as usize) * N + z as usize];
        let mut n = 0;
        for x in 0..N as i32 { for y in 0..N as i32 { for z in 0..N as i32 {
            if !at(dense, x, y, z) { continue; }
            let dirs = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];
            for (f, (dx, dy, dz)) in dirs.iter().enumerate() {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                let inside = (0..N as i32).contains(&nx) && (0..N as i32).contains(&ny) && (0..N as i32).contains(&nz);
                let beyond_solid = if inside { at(dense, nx, ny, nz) } else {
                    match &neigh[f] {
                        None => false,
                        Some(nd) => at(nd, nx.rem_euclid(N as i32), ny.rem_euclid(N as i32), nz.rem_euclid(N as i32)),
                    }
                };
                if !beyond_solid { n += 1; }
            }
        }}}
        n
    }

    fn open() -> [NeighbourLayer; 6] { [NeighbourLayer(None); 6] }

    #[test]
    fn set_get_roundtrip_in_all_three_layouts() {
        let mut c = BitChunk::empty();
        c.set(3, 17, 31);
        assert!(c.get(3, 17, 31));
        assert_eq!(c.y_cols[3 * N + 31], 1 << 17);
        assert_eq!(c.x_cols[17 * N + 31], 1 << 3);
        assert_eq!(c.z_cols[3 * N + 17], 1 << 31);
        c.clear(3, 17, 31);
        assert!(c == BitChunk::empty());
    }

    #[test]
    fn uniform_detection() {
        assert_eq!(BitChunk::empty().is_uniform(), Some(false));
        assert_eq!(BitChunk::full().is_uniform(), Some(true));
        let mut c = BitChunk::full(); c.clear(0, 0, 0);
        assert_eq!(c.is_uniform(), None);
        assert_eq!(c.solid_count(), 32767);
    }

    #[test]
    fn single_cube_has_six_faces_and_full_chunk_has_six_sides_open() {
        let mut c = BitChunk::empty(); c.set(10, 10, 10);
        assert_eq!(c.exposed_face_count(open()), 6);
        assert_eq!(BitChunk::full().exposed_face_count(open()), 6 * NN as u32);
        // Fully enclosed by solid neighbours: nothing exposed.
        let full = BitChunk::full();
        let closed = [
            NeighbourLayer(Some(full.boundary_layer(Face::NegX))),
            NeighbourLayer(Some(full.boundary_layer(Face::PosX))),
            NeighbourLayer(Some(full.boundary_layer(Face::NegY))),
            NeighbourLayer(Some(full.boundary_layer(Face::PosY))),
            NeighbourLayer(Some(full.boundary_layer(Face::NegZ))),
            NeighbourLayer(Some(full.boundary_layer(Face::PosZ))),
        ];
        assert_eq!(full.exposed_face_count(closed), 0);
    }

    #[test]
    fn face_counts_match_brute_force_random_chunks_open_boundary() {
        for seed in 1..=12u64 {
            for &(num, den) in &[(1u64, 2u64), (1, 8), (7, 8), (1, 40)] {
                let (c, d) = random_chunk(seed * 7919 + num, num, den);
                let want = brute_faces(&d, &[None, None, None, None, None, None]);
                assert_eq!(c.exposed_face_count(open()), want, "seed {} p={}/{}", seed, num, den);
            }
        }
    }

    #[test]
    fn face_counts_match_brute_force_with_random_neighbours() {
        for seed in 1..=8u64 {
            let (c, d) = random_chunk(seed * 31, 1, 3);
            // Six random neighbour chunks; we hand the bitchunk only the
            // touching layer of each, the brute force sees the whole.
            let mut neigh_dense: [Option<Vec<bool>>; 6] = [None, None, None, None, None, None];
            let mut layers = open();
            let opposite = [Face::NegX, Face::PosX, Face::NegY, Face::PosY, Face::NegZ, Face::PosZ];
            for f in 0..6 {
                if seed % 2 == 0 && f == 3 { continue; } // leave one open sometimes
                let (nc, nd) = random_chunk(seed * 101 + f as u64, 1, 2);
                layers[f] = NeighbourLayer(Some(nc.boundary_layer(opposite[f])));
                neigh_dense[f] = Some(nd);
            }
            let want = brute_faces(&d, &neigh_dense);
            assert_eq!(c.exposed_face_count(layers), want, "seed {}", seed);
        }
    }

    #[test]
    fn greedy_area_equals_face_count_and_quads_do_not_overlap() {
        for seed in 1..=6u64 {
            for terrain in [false, true] {
                let (c, _) = if terrain { terrain_chunk(seed) } else { random_chunk(seed * 13, 1, 3) };
                let mut total_area = 0u32;
                let mut total_faces = 0u32;
                for &f in &FACES {
                    let mask = c.face_mask(f, NeighbourLayer(None));
                    total_faces += mask.iter().map(|m| m.count_ones()).sum::<u32>();
                    let mut quads = Vec::new();
                    greedy_faces(&c, f, NeighbourLayer(None), |_, _, _| 0, &mut quads);
                    // Coverage bitmap per slice to check no overlap.
                    let mut cover = vec![[0u32; N]; N];
                    for q in &quads {
                        total_area += q.w as u32 * q.h as u32;
                        for u in q.u..q.u + q.w {
                            let bits: u32 = if q.h as usize == N { u32::MAX } else { ((1u32 << q.h) - 1) << q.v };
                            assert_eq!(cover[q.slice as usize][u as usize] & bits, 0, "overlap");
                            cover[q.slice as usize][u as usize] |= bits;
                        }
                    }
                }
                assert_eq!(total_area, total_faces, "seed {} terrain {}", seed, terrain);
            }
        }
    }

    #[test]
    fn terrain_merges_into_few_quads() {
        // A flat 32×32 top at one height must be ONE quad on +Y.
        let c = BitChunk::from_fn(|_, y, _| y < 12);
        let mut q = Vec::new();
        greedy_faces(&c, Face::PosY, NeighbourLayer(None), |_, _, _| 0, &mut q);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0], Quad { slice: 11, u: 0, v: 0, w: 32, h: 32 });
        // Each side wall is one 32×12 quad.
        let mut s = Vec::new();
        greedy_faces(&c, Face::PosX, NeighbourLayer(None), |_, _, _| 0, &mut s);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].w, s[0].h), (12, 32));
        // Bottom open: one quad.
        let mut b = Vec::new();
        greedy_faces(&c, Face::NegY, NeighbourLayer(None), |_, _, _| 0, &mut b);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn key_breaks_merges() {
        let c = BitChunk::from_fn(|_, y, _| y < 1);
        // Checkerboard of two "block ids" → 1024 quads; uniform key → 1.
        let mut q = Vec::new();
        greedy_faces(&c, Face::PosY, NeighbourLayer(None), |x, _, z| ((x + z) % 2) as u32, &mut q);
        assert_eq!(q.len(), NN);
        let mut one = Vec::new();
        greedy_faces(&c, Face::PosY, NeighbourLayer(None), |_, _, _| 7, &mut one);
        assert_eq!(one.len(), 1);
        // Stripes along z merge into 32 quads of 1×32.
        let mut stripes = Vec::new();
        greedy_faces(&c, Face::PosY, NeighbourLayer(None), |x, _, _| x as u32, &mut stripes);
        assert_eq!(stripes.len(), N);
        assert!(stripes.iter().all(|q| q.w == 1 && q.h == 32));
    }

    #[test]
    fn two_set_mask_glass_against_glass_and_stone() {
        // stone at x=0..16, glass at x=16..24, air beyond, all at y=z=5
        let mut stone = BitChunk::empty();
        let mut glass = BitChunk::empty();
        for x in 0..16 { stone.set(x, 5, 5); }
        for x in 16..24 { glass.set(x, 5, 5); }
        let both = stone.union(&glass);
        // Stone's +X faces: only the one at x=15 whose neighbour is glass
        // (glass is not an opaque blocker) → exposed.
        let sx = face_mask_between(&stone, &stone, Face::PosX, NeighbourLayer(None));
        assert_eq!(sx.iter().map(|m| m.count_ones()).sum::<u32>(), 1);
        // Glass's -X faces against stone: blocked (stone ∈ blocker set);
        // glass-glass internal faces: blocked; the +X face at x=23: open.
        let gnx = face_mask_between(&glass, &both, Face::NegX, NeighbourLayer(None));
        assert_eq!(gnx.iter().map(|m| m.count_ones()).sum::<u32>(), 0);
        let gpx = face_mask_between(&glass, &both, Face::PosX, NeighbourLayer(None));
        assert_eq!(gpx.iter().map(|m| m.count_ones()).sum::<u32>(), 1);
        let mut cells = Vec::new();
        for_each_cell(&gpx, Face::PosX, |x, y, z| cells.push((x, y, z)));
        assert_eq!(cells, vec![(23, 5, 5)]);
        // for_each_cell on the other layouts
        let mut c = BitChunk::empty(); c.set(3, 7, 11);
        let mut got = Vec::new();
        for_each_cell(&c.face_mask(Face::PosY, NeighbourLayer(None)), Face::PosY, |x, y, z| got.push((x, y, z)));
        for_each_cell(&c.face_mask(Face::NegZ, NeighbourLayer(None)), Face::NegZ, |x, y, z| got.push((x, y, z)));
        assert_eq!(got, vec![(3, 7, 11), (3, 7, 11)]);
    }

    #[test]
    fn boundary_layer_is_what_the_neighbour_sees() {
        let mut c = BitChunk::empty();
        c.set(31, 5, 9);   // on the +X face
        c.set(0, 6, 10);   // on the -X face
        let px = c.boundary_layer(Face::PosX);
        assert_eq!(px[5], 1 << 9);
        let nx = c.boundary_layer(Face::NegX);
        assert_eq!(nx[6], 1 << 10);
        // A neighbour to +X whose -X layer is this cell closes the face.
        let mut n = BitChunk::empty(); n.set(0, 5, 9);
        let m = c.face_mask(Face::PosX, NeighbourLayer(Some(n.boundary_layer(Face::NegX))));
        assert_eq!(m.iter().map(|x| x.count_ones()).sum::<u32>(), 1, "only the (0,6,10) cube's +X face remains");
    }
}
