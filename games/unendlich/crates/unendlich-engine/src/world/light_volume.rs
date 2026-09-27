//! Sky light and block light for the chunks around the player, as a toroidal
//! 3D texture the chunk shader samples.
//!
//! Before this, every surface was lit by the outdoor sun and ambient, so a cave
//! a hundred blocks down was as bright as a meadow, and a glowing block or lava
//! lit nothing. `cave_biomes.docx` is written around darkness ("placed torches
//! are the only light"), so this is the prerequisite for most of it.
//!
//! Two layers:
//!
//! * [`light_chunk`] is the pure core: given block lookups around one chunk
//!   and whether each column is open to the sky above, it floods sky light and
//!   coloured block light over the chunk plus a 15-block apron (light never
//!   travels further than 15) with the tested BFS in [`crate::light`], and
//!   returns the chunk's 32^3 cells as `[sky, r, g, b]` bytes.
//! * [`LightVolumeSys`] is the steerable loop around it: it keeps a window of
//!   5 x 3 x 5 chunks centred on the player, notices which chunks changed (the
//!   chunk manager replaces the `Arc` on every edit, so a pointer compare is
//!   enough), relights them and their neighbours on a worker thread, and hands
//!   back boxes to upload. Its counters say what it is doing.
//!
//! The texture maps world cell `w` to texel `w mod SIZE` on each axis. SIZE is
//! a multiple of 32, so a chunk is always one contiguous 32^3 box, and the
//! window can move without moving any data: chunks that leave free their box
//! for the chunks that arrive.
use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use super::blocks::BlockId;
use super::chunk::{Chunk, ChunkCoord};
use crate::constants::CHUNK_SIZE;
use crate::light::{propagate_block_light_tinted, MAX_LIGHT};

const CS: i32 = CHUNK_SIZE;

/// Light never travels further than this many blocks, so this much of the
/// neighbourhood decides a chunk's light.
pub const APRON: i32 = MAX_LIGHT as i32;

/// Chunk layers above the window whose changes are watched (they decide
/// what the window sees of the sky).
pub const ABOVE_TRACKED: i32 = 6;

/// Window size in chunks (x, y, z), centred on the player's chunk.
pub const WIN_CHUNKS: [i32; 3] = [5, 3, 5];

/// Texture size in cells.
pub const SIZE: [i32; 3] = [WIN_CHUNKS[0] * CS, WIN_CHUNKS[1] * CS, WIN_CHUNKS[2] * CS];

/// Does this block stop light? Full opaque cubes do; leaves, water, glass,
/// plants and see-through minerals let it through (some with extra loss).
#[inline]
pub fn blocks_light(b: BlockId) -> bool {
    b.is_opaque_occluder()
}

/// Extra levels lost entering this block, on top of the one level per step:
/// water and leaves dim what passes through them, so a pond's floor and a
/// forest floor are darker than open ground.
#[inline]
pub fn extra_loss(b: BlockId) -> u8 {
    match b {
        BlockId::Water | BlockId::Ice => 1,
        BlockId::OakLeaves | BlockId::BirchLeaves => 1,
        _ => 0,
    }
}

/// The chunks the volume holds: `min` is the lowest chunk coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub min: [i32; 3],
}

impl Window {
    /// The window centred on `chunk`.
    pub fn around(chunk: [i32; 3]) -> Self {
        Window {
            min: [
                chunk[0] - WIN_CHUNKS[0] / 2,
                chunk[1] - WIN_CHUNKS[1] / 2,
                chunk[2] - WIN_CHUNKS[2] / 2,
            ],
        }
    }

    pub fn contains(&self, c: [i32; 3]) -> bool {
        (0..3).all(|k| c[k] >= self.min[k] && c[k] < self.min[k] + WIN_CHUNKS[k])
    }

    pub fn chunks(&self) -> Vec<[i32; 3]> {
        let mut v = Vec::with_capacity((WIN_CHUNKS[0] * WIN_CHUNKS[1] * WIN_CHUNKS[2]) as usize);
        for y in 0..WIN_CHUNKS[1] {
            for z in 0..WIN_CHUNKS[2] {
                for x in 0..WIN_CHUNKS[0] {
                    v.push([self.min[0] + x, self.min[1] + y, self.min[2] + z]);
                }
            }
        }
        v
    }

    /// World position of the window's lowest corner, in blocks.
    pub fn origin_blocks(&self) -> [i32; 3] {
        [self.min[0] * CS, self.min[1] * CS, self.min[2] * CS]
    }
}

/// The texel a chunk's lowest cell lands on. The chunk fills the 32^3 box from
/// there.
pub fn chunk_texel_origin(c: [i32; 3]) -> [u32; 3] {
    [
        (c[0] * CS).rem_euclid(SIZE[0]) as u32,
        (c[1] * CS).rem_euclid(SIZE[1]) as u32,
        (c[2] * CS).rem_euclid(SIZE[2]) as u32,
    ]
}

/// Index of a chunk-local cell in a 32^3 light box: x fastest, then y, then z,
/// the row order `vkCmdCopyBufferToImage` reads with a 32 x 32 row/image size.
#[inline]
pub fn box_index(x: i32, y: i32, z: i32) -> usize {
    (x + y * CS + z * CS * CS) as usize
}

/// Scale a 0..=15 level to a texel byte.
#[inline]
fn byte(level: u8) -> u8 {
    level.min(MAX_LIGHT) * 17
}

/// A box of light for a chunk that has not been lit yet: open sky, no block
/// light. Outdoors that is right; underground it is the old look until the
/// relight lands.
pub fn unlit_box() -> Vec<[u8; 4]> {
    vec![[byte(MAX_LIGHT), 0, 0, 0]; (CS * CS * CS) as usize]
}

/// Light one chunk.
///
/// `block(w)` is the block at world cell `w`, or `None` where no chunk is
/// loaded. An unloaded cell beside or below the chunk blocks light (so a cave
/// at the edge of the loaded world does not glow from its missing neighbour);
/// one above the chunk is open air. `open_above(x, y, z)` says
/// whether nothing above world cell `(x, y, z)` blocks the sky, for columns
/// that leave the region at its top; unloaded space above counts as open.
///
/// Returns the chunk's 32^3 cells as `[sky, r, g, b]`, each level 0..=15
/// scaled to 0..=255, in [`box_index`] order.
pub fn light_chunk(
    chunk: [i32; 3],
    block: &dyn Fn([i32; 3]) -> Option<BlockId>,
    open_above: &dyn Fn(i32, i32, i32) -> bool,
) -> Vec<[u8; 4]> {
    light_chunk_with(chunk, block, open_above, &|_, b| b.light_emission())
}

/// [`light_chunk`] with the emission decided per cell: `emit(w, b)` is what
/// block `b` at world cell `w` gives off (a torch in a CO₂ basin gives off
/// nothing, see [`gas_aware_emission`]).
pub fn light_chunk_with(
    chunk: [i32; 3],
    block: &dyn Fn([i32; 3]) -> Option<BlockId>,
    open_above: &dyn Fn(i32, i32, i32) -> bool,
    emit: &dyn Fn([i32; 3], BlockId) -> [u8; 3],
) -> Vec<[u8; 4]> {
    let r = (CS + 2 * APRON) as usize;
    let min = [chunk[0] * CS - APRON, chunk[1] * CS - APRON, chunk[2] * CS - APRON];
    let ri = |x: usize, y: usize, z: usize| x + r * (y + r * z);

    // Region snapshot: opacity, extra loss, emitters.
    let mut opaque = vec![false; r * r * r];
    let mut loss = vec![0u8; r * r * r];
    let mut emitters: Vec<([usize; 3], [u8; 3])> = Vec::new();
    let top_of_chunk = (chunk[1] + 1) * CS;
    for z in 0..r {
        for y in 0..r {
            for x in 0..r {
                let w = [min[0] + x as i32, min[1] + y as i32, min[2] + z as i32];
                let i = ri(x, y, z);
                match block(w) {
                    // Unloaded beside or below the chunk blocks light (no glow
                    // from a missing neighbour); unloaded above it is open
                    // air, the way the sky test treats it.
                    None => opaque[i] = w[1] < top_of_chunk,
                    Some(b) => {
                        opaque[i] = blocks_light(b);
                        loss[i] = extra_loss(b);
                        let e = emit(w, b);
                        if e != [0, 0, 0] {
                            emitters.push(([x, y, z], e));
                        }
                    }
                }
            }
        }
    }

    // Sky: straight down from the top of every open column at full strength,
    // losing only what water and leaves take; then the same flood as block
    // light spreads it sideways under overhangs and into cave mouths.
    let mut sky_seeds: Vec<([usize; 3], [u8; 3])> = Vec::new();
    let top_y = min[1] + r as i32 - 1;
    for z in 0..r {
        for x in 0..r {
            if !open_above(min[0] + x as i32, top_y, min[2] + z as i32) {
                continue;
            }
            let mut level = MAX_LIGHT;
            for y in (0..r).rev() {
                let i = ri(x, y, z);
                if opaque[i] {
                    break;
                }
                level = level.saturating_sub(loss[i]);
                if level == 0 {
                    break;
                }
                sky_seeds.push(([x, y, z], [level, 0, 0]));
            }
        }
    }
    let is_opaque = |x: usize, y: usize, z: usize| opaque[ri(x, y, z)];
    let tint = |x: usize, y: usize, z: usize| {
        let l = loss[ri(x, y, z)];
        [l, l, l]
    };
    let sky = propagate_block_light_tinted(r, r, r, &sky_seeds, is_opaque, tint);
    let blk = propagate_block_light_tinted(r, r, r, &emitters, is_opaque, tint);

    // Solid cells take the light of the open cells beside them: the full
    // level one cell deep, one less two deep. A face is shaded from the open
    // cell in front of it, so this only matters where geometry sits inside
    // its cell: soft tops the mesher lowers into a slope, sloped faces, and
    // the far-terrain LOD surface, which runs a block or two inside the real
    // ground and drew as black shapes before this.
    let mut lit: Vec<[u8; 4]> = (0..r * r * r)
        .map(|i| {
            let (x, y, z) = (i % r, (i / r) % r, i / (r * r));
            let s = sky.get(x, y, z)[0];
            let b = blk.get(x, y, z);
            [s, b[0], b[1], b[2]]
        })
        .collect();
    for pass in 0..SOLID_DEPTH {
        let prev = lit.clone();
        for z in 1..r - 1 {
            for y in 1..r - 1 {
                for x in 1..r - 1 {
                    let i = ri(x, y, z);
                    if !opaque[i] || prev[i] != [0, 0, 0, 0] {
                        continue;
                    }
                    let mut m = [0u8; 4];
                    for j in [ri(x + 1, y, z), ri(x - 1, y, z), ri(x, y + 1, z), ri(x, y - 1, z), ri(x, y, z + 1), ri(x, y, z - 1)] {
                        for c in 0..4 {
                            m[c] = m[c].max(prev[j][c]);
                        }
                    }
                    for c in 0..4 {
                        lit[i][c] = m[c].saturating_sub(pass as u8);
                    }
                }
            }
        }
    }

    let a = APRON as usize;
    let mut out = vec![[0u8; 4]; (CS * CS * CS) as usize];
    for z in 0..CS {
        for y in 0..CS {
            for x in 0..CS {
                let l = lit[ri(x as usize + a, y as usize + a, z as usize + a)];
                out[box_index(x, y, z)] = [byte(l[0]), byte(l[1]), byte(l[2]), byte(l[3])];
            }
        }
    }
    out
}

/// How many cells deep solid blocks carry the light of the open cells beside
/// them (see the end of [`light_chunk`]).
pub const SOLID_DEPTH: usize = 2;

/// The chunks whose changes the loop watches: the window, a one-chunk ring
/// around it (beside and below), and `ABOVE_TRACKED` layers above it and the
/// ring.
pub fn tracked_chunks(w: Window) -> Vec<[i32; 3]> {
    let mut v = Vec::new();
    for x in (w.min[0] - 1)..(w.min[0] + WIN_CHUNKS[0] + 1) {
        for z in (w.min[2] - 1)..(w.min[2] + WIN_CHUNKS[2] + 1) {
            for y in (w.min[1] - 1)..(w.min[1] + WIN_CHUNKS[1] + ABOVE_TRACKED) {
                v.push([x, y, z]);
            }
        }
    }
    v
}

/// A hash of a light box, to compare what the GPU was sent with the CPU copy.
pub fn box_hash(data: &[[u8; 4]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in data {
        h = (h ^ u32::from_le_bytes(*t) as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A box for the renderer to copy into the light texture.
#[derive(Clone, Debug)]
pub struct Upload {
    /// Chunk this box is for (for logs and tests).
    pub chunk: [i32; 3],
    /// Lowest texel of the 32^3 box.
    pub texel: [u32; 3],
    /// 32^3 `[sky, r, g, b]` in [`box_index`] order.
    pub data: Vec<[u8; 4]>,
}

/// Counters for the trace line and the HUD: what the loop is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightStats {
    pub dirty: usize,
    pub in_flight: usize,
    pub relit: u64,
    pub window_moves: u64,
    /// Chunk replacements that did not change anything light depends on.
    pub ignored: u64,
}

/// A hash of what light depends on in a chunk: which cells block light, what
/// they take from light passing through, and what they emit. Two chunks with
/// the same signature light the same way (up to hash collisions); a chunk has
/// signature 0 only if it is not loaded.
pub fn light_signature(blocks: &[BlockId]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, &b) in blocks.iter().enumerate() {
        let e = b.light_emission();
        let code = (blocks_light(b) as u64)
            | (extra_loss(b) as u64) << 1
            | (e[0] as u64) << 4
            | (e[1] as u64) << 8
            | (e[2] as u64) << 12;
        if code != 0 {
            h = (h ^ (i as u64) ^ (code << 20)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h | 1
}

/// Emission with fire put out by bad air: a torch whose cell is in a CO₂
/// basin (world::cave_gas) too deep for combustion (sim::cave_air) gives no
/// light. The glows (crystal, glowshroom, the basin's lichen) and lava do not
/// need air.
pub fn gas_aware_emission(w: [i32; 3], b: BlockId, gas_seed: Option<u32>) -> [u8; 3] {
    if b == BlockId::Torch {
        if let Some(seed) = gas_seed {
            let d = super::cave_gas::co2_density(w[0], w[1], w[2], seed);
            if super::cave_gas::o2_for_density(d) < crate::sim::cave_air::FLAME_O2_MIN {
                return [0, 0, 0];
            }
        }
    }
    b.light_emission()
}

struct Job {
    gas_seed: Option<u32>,
    chunk: [i32; 3],
    chunks: HashMap<[i32; 3], Arc<Chunk>>,
}

/// Light a chunk from a snapshot of its neighbourhood (the 3 x 3 x 3 chunks
/// around it and the loaded chunks above those columns).
fn light_from_snapshot(job: &Job) -> Vec<[u8; 4]> {
    let get = |w: [i32; 3]| -> Option<BlockId> {
        let c = [w[0].div_euclid(CS), w[1].div_euclid(CS), w[2].div_euclid(CS)];
        job.chunks
            .get(&c)
            .map(|ch| ch.blocks[Chunk::block_index(w[0].rem_euclid(CS), w[1].rem_euclid(CS), w[2].rem_euclid(CS))])
    };
    let open_above = |x: i32, y: i32, z: i32| -> bool {
        let (cx, cz) = (x.div_euclid(CS), z.div_euclid(CS));
        let mut cy = y.div_euclid(CS);
        let mut from = y.rem_euclid(CS) + 1;
        loop {
            let Some(ch) = job.chunks.get(&[cx, cy, cz]) else { return true };
            for ly in from..CS {
                if blocks_light(ch.blocks[Chunk::block_index(x.rem_euclid(CS), ly, z.rem_euclid(CS))]) {
                    return false;
                }
            }
            cy += 1;
            from = 0;
        }
    };
    let gas_seed = job.gas_seed;
    light_chunk_with(job.chunk, &get, &open_above, &|w, b| gas_aware_emission(w, b, gas_seed))
}

/// The chunks a relight of `c` reads: its 3 x 3 x 3 neighbourhood (whatever
/// of it is loaded) and the loaded chunks above those columns.
fn snapshot(c: [i32; 3], get: &dyn Fn(ChunkCoord) -> Option<Arc<Chunk>>) -> HashMap<[i32; 3], Arc<Chunk>> {
    let mut chunks = HashMap::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (cx, cz) = (c[0] + dx, c[2] + dz);
            // The 3 x 3 x 3 neighbourhood, whatever of it is
            // loaded (the first cut walked up from below and
            // stopped at the first gap, so a neighbour with
            // nothing loaded under it was left out)...
            for cy in (c[1] - 1)..=(c[1] + 1) {
                if let Some(ch) = get(ChunkCoord::new(cx, cy, cz)) {
                    chunks.insert([cx, cy, cz], ch);
                }
            }
            // ...then up the column while chunks are loaded, for
            // the sky test (unloaded above counts as open).
            let mut cy = c[1] + 2;
            while let Some(ch) = get(ChunkCoord::new(cx, cy, cz)) {
                chunks.insert([cx, cy, cz], ch);
                cy += 1;
                if cy > c[1] + 16 {
                    break;
                }
            }
        }
    }
    chunks
}

/// The loop that keeps the light texture in step with the world.
pub struct LightVolumeSys {
    window: Option<Window>,
    /// Chunk -> the `Arc` pointer it was last seen with (0 = not loaded) and
    /// its light signature at that time.
    seen: HashMap<[i32; 3], (usize, u64)>,
    dirty: BTreeSet<[i32; 3]>,
    in_flight: BTreeSet<[i32; 3]>,
    job_tx: Sender<Job>,
    done_rx: Receiver<([i32; 3], Vec<[u8; 4]>)>,
    pub stats: LightStats,
    /// Most jobs queued to the worker at once.
    pub max_in_flight: usize,
    /// CPU copy of every relit chunk in the window, for `light_at` (gameplay
    /// that needs the light level, and the trace line).
    lit: HashMap<[i32; 3], Arc<Vec<[u8; 4]>>>,
    /// World seed for CO₂ basins (torches in them do not burn); `None`
    /// ignores the gas. Changing it relights nothing by itself.
    pub gas_seed: Option<u32>,
}

impl Default for LightVolumeSys {
    fn default() -> Self {
        Self::new()
    }
}

impl LightVolumeSys {
    /// Start the loop with one worker thread.
    pub fn new() -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (done_tx, done_rx) = channel();
        std::thread::Builder::new()
            .name("light-volume".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let data = light_from_snapshot(&job);
                    if done_tx.send((job.chunk, data)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn light-volume worker");
        LightVolumeSys {
            window: None,
            seen: HashMap::new(),
            dirty: BTreeSet::new(),
            in_flight: BTreeSet::new(),
            job_tx,
            done_rx,
            stats: LightStats::default(),
            max_in_flight: 4,
            lit: HashMap::new(),
            gas_seed: None,
        }
    }

    /// Relight every lit chunk in the window from the world as it is now and
    /// compare with what is stored. Returns `(chunk, cells that differ)` for
    /// every chunk whose stored light is stale. On a settled loop this should
    /// be empty; anything here is a change the loop did not notice.
    /// Synchronous and slow (one relight per chunk): a diagnostic.
    pub fn audit(&self, get: &dyn Fn(ChunkCoord) -> Option<Arc<Chunk>>) -> Vec<([i32; 3], usize)> {
        let mut bad = Vec::new();
        let Some(w) = self.window else { return bad };
        for c in w.chunks() {
            let Some(stored) = self.lit.get(&c) else { continue };
            let job = Job { gas_seed: self.gas_seed, chunk: c, chunks: snapshot(c, get) };
            let fresh = light_from_snapshot(&job);
            let n = fresh.iter().zip(stored.iter()).filter(|(a, b)| a != b).count();
            if n > 0 {
                bad.push((c, n));
            }
        }
        bad
    }

    /// Hash of the stored light for a chunk (see [`box_hash`]).
    pub fn lit_hash(&self, c: [i32; 3]) -> Option<u64> {
        self.lit.get(&c).map(|d| box_hash(d))
    }

    /// `[sky, r, g, b]` (0..=15) at world cell `w`, if its chunk is in the
    /// window and has been lit.
    pub fn light_at(&self, w: [i32; 3]) -> Option<[u8; 4]> {
        let c = [w[0].div_euclid(CS), w[1].div_euclid(CS), w[2].div_euclid(CS)];
        let d = self.lit.get(&c)?;
        let t = d[box_index(w[0].rem_euclid(CS), w[1].rem_euclid(CS), w[2].rem_euclid(CS))];
        Some([t[0] / 17, t[1] / 17, t[2] / 17, t[3] / 17])
    }

    pub fn window(&self) -> Option<Window> {
        self.window
    }

    /// Mark a chunk and everything its light can reach as needing a relight:
    /// its 26 neighbours (light crosses 15 blocks) and the chunks below it in
    /// the window (its roof shades them).
    fn mark(&mut self, c: [i32; 3], w: Window) {
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n = [c[0] + dx, c[1] + dy, c[2] + dz];
                    if w.contains(n) {
                        self.dirty.insert(n);
                    }
                }
            }
        }
        let mut y = c[1] - 2;
        while y >= w.min[1] {
            self.dirty.insert([c[0], y, c[2]]);
            y -= 1;
        }
    }

    /// One step of the loop. `player_chunk` centres the window; `get` returns a
    /// loaded chunk. Returns the boxes to upload this frame: placeholders for
    /// chunks that just entered the window, and finished relights.
    pub fn tick(
        &mut self,
        player_chunk: [i32; 3],
        get: &dyn Fn(ChunkCoord) -> Option<Arc<Chunk>>,
    ) -> Vec<Upload> {
        let mut out = Vec::new();
        let w = Window::around(player_chunk);
        if self.window != Some(w) {
            let old = self.window;
            self.window = Some(w);
            self.stats.window_moves += 1;
            let keep: std::collections::HashSet<[i32; 3]> = tracked_chunks(w).into_iter().collect();
            self.seen.retain(|c, _| keep.contains(c));
            self.lit.retain(|c, _| w.contains(*c));
            self.dirty.retain(|c| w.contains(*c));
            for c in w.chunks() {
                if old.map_or(true, |o| !o.contains(c)) {
                    out.push(Upload { chunk: c, texel: chunk_texel_origin(c), data: unlit_box() });
                }
            }
        }

        // Changes: a new Arc means the chunk loaded, unloaded or was edited.
        // Many replacements change nothing light cares about (plant growth,
        // leaf decay, crop stages replace the chunk every few ticks), so a new
        // Arc only marks work when the chunk's light signature changed too.
        // Without this the first soak relit ~8 chunks a second forever.
        // The chunks above the window count too: they decide what is open to
        // the sky, and a relight that read an old version of one (the title
        // screen's world, a chunk that had not streamed in yet) left a whole
        // shore black in the first outdoor soak, because nothing noticed when
        // it changed. So does the ring of chunks around the window: a window
        // chunk lit before its outside neighbour streamed in treats that
        // neighbour as solid, and kept black wedges along the window's edge.
        let tracked = tracked_chunks(w);
        for c in tracked {
            let ch = get(ChunkCoord::new(c[0], c[1], c[2]));
            let ptr = ch.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize);
            match self.seen.get(&c) {
                Some(&(p, _)) if p == ptr => {}
                prev => {
                    let sig = ch.as_ref().map_or(0, |a| light_signature(&a.blocks));
                    let changed = prev.map_or(true, |&(_, s)| s != sig);
                    self.seen.insert(c, (ptr, sig));
                    if changed && w.contains(c) {
                        self.mark(c, w);
                    } else if changed {
                        // Outside the window: every window chunk beside or
                        // under it (its light and its shade reach them).
                        for d in w.chunks() {
                            if (d[0] - c[0]).abs() <= 1 && (d[2] - c[2]).abs() <= 1 && d[1] <= c[1] + 1 {
                                self.dirty.insert(d);
                            }
                        }
                    } else {
                        self.stats.ignored += 1;
                    }
                }
            }
        }

        // Finished work.
        while let Ok((c, data)) = self.done_rx.try_recv() {
            self.in_flight.remove(&c);
            self.stats.relit += 1;
            if w.contains(c) {
                self.lit.insert(c, Arc::new(data.clone()));
                out.push(Upload { chunk: c, texel: chunk_texel_origin(c), data });
            }
        }

        // New work, nearest first.
        if self.in_flight.len() < self.max_in_flight && !self.dirty.is_empty() {
            let mut order: Vec<[i32; 3]> = self.dirty.iter().copied().filter(|c| !self.in_flight.contains(c)).collect();
            order.sort_by_key(|c| {
                let d = [c[0] - player_chunk[0], c[1] - player_chunk[1], c[2] - player_chunk[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
            });
            for c in order {
                if self.in_flight.len() >= self.max_in_flight {
                    break;
                }
                self.dirty.remove(&c);
                if get(ChunkCoord::new(c[0], c[1], c[2])).is_none() {
                    continue; // nothing to draw there; it relights when it loads
                }
                let chunks = snapshot(c, get);
                if self.job_tx.send(Job { gas_seed: self.gas_seed, chunk: c, chunks }).is_ok() {
                    self.in_flight.insert(c);
                }
            }
        }
        self.stats.dirty = self.dirty.len();
        self.stats.in_flight = self.in_flight.len();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: u8 = 17; // one level as a byte

    /// A world from a closure over world cells, all loaded.
    fn light(chunk: [i32; 3], f: impl Fn([i32; 3]) -> BlockId, sky: bool) -> Vec<[u8; 4]> {
        light_chunk(chunk, &|w| Some(f(w)), &|_, _, _| sky)
    }

    #[test]
    fn open_air_under_open_sky_is_fully_lit() {
        let out = light([0, 0, 0], |_| BlockId::Air, true);
        assert!(out.iter().all(|t| t[0] == 255 && t[1..] == [0, 0, 0]));
    }

    #[test]
    fn a_sealed_room_is_dark_and_a_torch_falls_off_one_level_a_block() {
        // Solid rock everywhere except a 20-long corridor along x at y=16,z=16.
        let rock = |w: [i32; 3]| {
            if w[1] == 16 && w[2] == 16 && (5..25).contains(&w[0]) {
                if w[0] == 5 { BlockId::Torch } else { BlockId::Air }
            } else {
                BlockId::Stone
            }
        };
        let out = light([0, 0, 0], rock, true);
        let at = |x| out[box_index(x, 16, 16)];
        let e = BlockId::Torch.light_emission();
        assert_eq!(at(5)[1], e[0] * L, "torch cell at full red");
        for x in 6..20 {
            assert_eq!(at(x)[0], 0, "no sky in a sealed corridor");
            let expect = e[0].saturating_sub((x - 5) as u8);
            assert_eq!(at(x)[1], expect * L, "red at distance {}", x - 5);
        }
        // Rock deep inside is not lit; the rock skin next to the corridor
        // carries the corridor's light (one level less two cells in).
        assert_eq!(out[box_index(10, 10, 10)], [0, 0, 0, 0]);
        assert_eq!(at(8)[1], out[box_index(8, 17, 16)][1], "the wall cell above takes the corridor's level");
        assert_eq!(out[box_index(8, 18, 16)][1], at(8)[1] - L, "one less a cell further in");
        assert_eq!(out[box_index(8, 19, 16)][1], 0, "and nothing three deep");
    }

    #[test]
    fn sky_bleeds_into_a_cave_mouth_and_fades() {
        // Ground surface at y=20 (rock at and below), a horizontal tunnel at
        // y=10 running in from x=0, whose mouth at x<0 is an open pit.
        let world = |w: [i32; 3]| {
            let pit = w[0] < 0;
            let tunnel = w[1] == 10 && w[2] == 16 && w[0] >= 0 && w[0] < 30;
            if pit || tunnel || w[1] > 20 { BlockId::Air } else { BlockId::Stone }
        };
        let out = light([0, 0, 0], world, true);
        let s = |x| out[box_index(x, 10, 16)][0] / L;
        assert_eq!(s(0), 14, "one step in from the open pit");
        assert!(s(5) < s(1) && s(5) > 0, "fades along the tunnel: {} {}", s(1), s(5));
        assert_eq!(s(20), 0, "dark 20 blocks in");
        assert_eq!(out[box_index(5, 25, 5)][0], 255, "open ground above is lit");
    }

    #[test]
    fn a_torch_in_the_next_chunk_lights_across_the_boundary() {
        // Air box, sky closed; a torch 3 blocks past the chunk's +x face.
        let world = |w: [i32; 3]| if w == [CS + 2, 16, 16] { BlockId::Torch } else { BlockId::Air };
        let out = light([0, 0, 0], world, false);
        let red = out[box_index(CS - 1, 16, 16)][1] / L;
        assert_eq!(red, BlockId::Torch.light_emission()[0] - 3);
    }

    #[test]
    fn unloaded_neighbours_block_light_but_open_sky_above_still_lights() {
        // Only the target chunk is loaded, all air. Nothing leaks in from the
        // unloaded sides; the sky still comes from above.
        let only = |w: [i32; 3]| {
            let c = [w[0].div_euclid(CS), w[1].div_euclid(CS), w[2].div_euclid(CS)];
            if c == [0, 0, 0] { Some(BlockId::Air) } else { None }
        };
        let closed = light_chunk([0, 0, 0], &only, &|_, _, _| false);
        assert!(closed.iter().all(|t| t[0] == 0), "no sky when the columns are closed above");
        let open = light_chunk([0, 0, 0], &only, &|_, _, _| true);
        assert!(open.iter().all(|t| t[0] == 255), "unloaded space above is open sky");
        // A torch beside the chunk in an unloaded chunk cannot exist, and a
        // loaded torch 3 blocks outside an unloaded wall does not shine
        // through it: covered by the opacity of unloaded side cells.
        let side = |w: [i32; 3]| {
            let c = [w[0].div_euclid(CS), w[1].div_euclid(CS), w[2].div_euclid(CS)];
            if c == [0, 0, 0] { Some(BlockId::Air) } else if w == [CS + 2, 16, 16] { Some(BlockId::Torch) } else { None }
        };
        let lit = light_chunk([0, 0, 0], &side, &|_, _, _| false);
        assert!(lit.iter().all(|t| t[1] == 0), "no light through unloaded cells");
    }

    #[test]
    fn water_dims_the_sky_below_it() {
        let world = |w: [i32; 3]| if (10..20).contains(&w[1]) { BlockId::Water } else if w[1] < 10 { BlockId::Stone } else { BlockId::Air };
        let out = light([0, 0, 0], world, true);
        let s = |y| out[box_index(16, y, 16)][0] / L;
        assert_eq!(s(25), 15);
        assert_eq!(s(19), 14, "one block of water");
        assert!(s(10) < s(15) && s(15) < s(19), "darker with depth");
    }

    #[test]
    fn a_roof_shades_what_is_under_it() {
        // A slab at y=25 over x,z in 8..24; open elsewhere.
        let world = |w: [i32; 3]| {
            if w[1] == 25 && (8..24).contains(&w[0]) && (8..24).contains(&w[2]) { BlockId::Stone }
            else if w[1] < 0 { BlockId::Stone }
            else { BlockId::Air }
        };
        let out = light([0, 0, 0], world, true);
        let under = out[box_index(16, 5, 16)][0] / L;
        let edge = out[box_index(9, 5, 16)][0] / L;
        assert!(under < 15, "under the middle of the roof: {under}");
        assert!(edge > under, "brighter near the roof's edge: {edge} vs {under}");
        assert_eq!(out[box_index(2, 5, 2)][0], 255, "open sky beside it");
    }

    #[test]
    fn a_torch_in_a_co2_basin_gives_no_light_and_one_above_it_does() {
        let seed = 77;
        let b = (0..40)
            .flat_map(|rx| (0..40).map(move |rz| (rx, rz)))
            .find_map(|(rx, rz)| crate::world::cave_gas::basin(rx, rz, seed))
            .expect("a basin");
        let deep = [b.center[0], b.surface_y - 2, b.center[1]];
        let high = [b.center[0], b.surface_y + 3, b.center[1]];
        assert_eq!(gas_aware_emission(deep, BlockId::Torch, Some(seed)), [0, 0, 0]);
        assert_eq!(gas_aware_emission(high, BlockId::Torch, Some(seed)), BlockId::Torch.light_emission());
        assert_eq!(gas_aware_emission(deep, BlockId::Torch, None), BlockId::Torch.light_emission(), "gas off");
        assert_eq!(gas_aware_emission(deep, BlockId::Glowshroom, Some(seed)), BlockId::Glowshroom.light_emission(), "glows need no air");
    }

    #[test]
    fn window_and_texels() {
        let w = Window::around([10, 2, -7]);
        assert_eq!(w.min, [8, 1, -9]);
        assert!(w.contains([12, 3, -5]) && !w.contains([13, 3, -5]) && !w.contains([12, 0, -5]));
        assert_eq!(w.chunks().len(), 75);
        // Every chunk in any window has its own box.
        let mut boxes: Vec<[u32; 3]> = w.chunks().iter().map(|&c| chunk_texel_origin(c)).collect();
        boxes.sort();
        boxes.dedup();
        assert_eq!(boxes.len(), 75);
        for b in &boxes {
            assert!(b.iter().zip(SIZE).all(|(&t, s)| t as i32 + CS <= s), "box fits: {b:?}");
        }
    }

    fn solid_chunk(c: [i32; 3], b: BlockId) -> Arc<Chunk> {
        Arc::new(Chunk::from_blocks(ChunkCoord::new(c[0], c[1], c[2]), vec![b; (CS * CS * CS) as usize]))
    }

    /// Drive the loop until it goes quiet, collecting uploads.
    fn run(sys: &mut LightVolumeSys, pc: [i32; 3], get: &dyn Fn(ChunkCoord) -> Option<Arc<Chunk>>) -> Vec<Upload> {
        let mut all = Vec::new();
        for _ in 0..2000 {
            all.extend(sys.tick(pc, get));
            if sys.stats.dirty == 0 && sys.stats.in_flight == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        all
    }

    #[test]
    fn the_loop_lights_the_window_then_goes_quiet_and_relights_an_edit() {
        // A world of air above y=0 chunks, stone below.
        let store: std::sync::Mutex<HashMap<[i32; 3], Arc<Chunk>>> = std::sync::Mutex::new(HashMap::new());
        for c in Window::around([0, 0, 0]).chunks() {
            let b = if c[1] < 0 { BlockId::Stone } else { BlockId::Air };
            store.lock().unwrap().insert(c, solid_chunk(c, b));
        }
        let get = |c: ChunkCoord| store.lock().unwrap().get(&[c.x, c.y, c.z]).cloned();
        let mut sys = LightVolumeSys::new();
        let ups = run(&mut sys, [0, 0, 0], &get);
        let placeholders = ups.iter().filter(|u| u.data == unlit_box()).count();
        assert!(placeholders >= 75, "every chunk got a placeholder first ({placeholders})");
        let relit: std::collections::HashSet<[i32; 3]> = ups.iter().filter(|u| u.data != unlit_box()).map(|u| u.chunk).collect();
        assert!(relit.len() >= 25, "the stone layer was relit ({})", relit.len());
        let quiet = sys.tick([0, 0, 0], &get);
        assert!(quiet.is_empty(), "nothing to do on a settled window");
        assert!(sys.audit(&get).is_empty(), "a settled window matches a fresh relight");

        // A replacement that changes nothing light depends on (the plant and
        // crop ticks do this constantly) is noticed and ignored.
        let c1 = [1, -1, 1];
        let same = store.lock().unwrap()[&c1].clone_for_edit();
        store.lock().unwrap().insert(c1, Arc::new(same));
        let before = sys.stats.ignored;
        let ups = sys.tick([0, 0, 0], &get);
        assert!(ups.is_empty() && sys.stats.dirty == 0, "a light-neutral replacement relights nothing");
        assert_eq!(sys.stats.ignored, before + 1);

        // Put a torch in an air chunk: that chunk is relit with block light.
        let c = [0, 0, 0];
        let mut ch = store.lock().unwrap()[&c].clone_for_edit();
        ch.set(16, 16, 16, BlockId::Torch);
        store.lock().unwrap().insert(c, Arc::new(ch));
        let ups = run(&mut sys, [0, 0, 0], &get);
        let mine = ups.iter().rev().find(|u| u.chunk == c).expect("the edited chunk was relit");
        assert_eq!(mine.data[box_index(16, 16, 16)][1], BlockId::Torch.light_emission()[0] * L);
        // The CPU copy agrees with what was uploaded.
        assert_eq!(sys.light_at([16, 16, 16]).map(|l| l[1]), Some(BlockId::Torch.light_emission()[0]));
        assert_eq!(sys.light_at([16, -16, 16]), Some([0, 0, 0, 0]), "inside the stone layer");
        assert_eq!(sys.light_at([16, 16, 1000]), None, "outside the window");
    }

    #[test]
    fn a_roof_appearing_above_the_window_relights_the_column_under_it() {
        let store: std::sync::Mutex<HashMap<[i32; 3], Arc<Chunk>>> = std::sync::Mutex::new(HashMap::new());
        // Air in the window and in two layers above it (loaded, as they are in
        // play; the relight's sky test stops at the first unloaded chunk).
        for c in Window::around([0, 0, 0]).chunks() {
            for dy in 0..=3 {
                let a = [c[0], c[1] + dy, c[2]];
                store.lock().unwrap().insert(a, solid_chunk(a, BlockId::Air));
            }
        }
        let get = |c: ChunkCoord| store.lock().unwrap().get(&[c.x, c.y, c.z]).cloned();
        let mut sys = LightVolumeSys::new();
        run(&mut sys, [0, 0, 0], &get);
        assert_eq!(sys.light_at([5, 5, 5]).map(|l| l[0]), Some(15), "open sky at first");
        // A stone chunk streams in two layers above the window's top.
        let roof = [0, 4, 0];
        store.lock().unwrap().insert(roof, solid_chunk(roof, BlockId::Stone));
        run(&mut sys, [0, 0, 0], &get);
        let under = sys.light_at([16, 5, 16]).map(|l| l[0]);
        assert!(under.map_or(false, |s| s < 15), "the column under the new roof was relit: {under:?}");
    }

    #[test]
    fn a_roof_built_at_the_top_of_the_window_darkens_the_bottom_layer() {
        let store: std::sync::Mutex<HashMap<[i32; 3], Arc<Chunk>>> = std::sync::Mutex::new(HashMap::new());
        for c in Window::around([0, 0, 0]).chunks() {
            for dy in 0..=3 {
                let a = [c[0], c[1] + dy, c[2]];
                store.lock().unwrap().insert(a, solid_chunk(a, BlockId::Air));
            }
        }
        let get = |c: ChunkCoord| store.lock().unwrap().get(&[c.x, c.y, c.z]).cloned();
        let mut sys = LightVolumeSys::new();
        run(&mut sys, [0, 0, 0], &get);
        let low = [16, -16, 16]; // in chunk (0, -1, 0), two layers under the top
        assert_eq!(sys.light_at(low).map(|l| l[0]), Some(15));
        let top = [0, 1, 0];
        store.lock().unwrap().insert(top, solid_chunk(top, BlockId::Stone));
        run(&mut sys, [0, 0, 0], &get);
        let s = sys.light_at(low).map(|l| l[0]);
        assert!(s.map_or(false, |v| v < 15), "the bottom layer under the new roof was relit: {s:?}");
    }

    #[test]
    fn a_neighbour_streaming_in_beside_the_window_relights_its_edge() {
        let store: std::sync::Mutex<HashMap<[i32; 3], Arc<Chunk>>> = std::sync::Mutex::new(HashMap::new());
        for c in Window::around([0, 0, 0]).chunks() {
            for dy in 0..=3 {
                let a = [c[0], c[1] + dy, c[2]];
                store.lock().unwrap().insert(a, solid_chunk(a, BlockId::Stone));
            }
        }
        // A torch just outside the window's +x edge, in a chunk that is not
        // loaded yet; the edge chunk beside it is hollow.
        let edge = [2, 0, 0];
        store.lock().unwrap().insert(edge, solid_chunk(edge, BlockId::Air));
        let get = |c: ChunkCoord| store.lock().unwrap().get(&[c.x, c.y, c.z]).cloned();
        let mut sys = LightVolumeSys::new();
        run(&mut sys, [0, 0, 0], &get);
        let at_edge = [3 * CS - 1, 16, 16];
        assert_eq!(sys.light_at(at_edge).map(|l| l[1]), Some(0));
        let out = [3, 0, 0];
        let mut ch = Chunk::from_blocks(ChunkCoord::new(3, 0, 0), vec![BlockId::Air; (CS * CS * CS) as usize]);
        ch.set(1, 16, 16, BlockId::Torch);
        store.lock().unwrap().insert(out, Arc::new(ch));
        run(&mut sys, [0, 0, 0], &get);
        let red = sys.light_at(at_edge).map(|l| l[1]);
        assert_eq!(red, Some(BlockId::Torch.light_emission()[0] - 2), "the edge sees the outside torch");
    }

    #[test]
    fn moving_the_window_placeholders_only_the_new_slice() {
        let get = |_c: ChunkCoord| -> Option<Arc<Chunk>> { None };
        let mut sys = LightVolumeSys::new();
        let first = sys.tick([0, 0, 0], &get);
        assert_eq!(first.len(), 75);
        let moved = sys.tick([1, 0, 0], &get);
        assert_eq!(moved.len(), 15, "one new 1 x 3 x 5 slice");
        assert!(moved.iter().all(|u| u.chunk[0] == 3));
    }
}
