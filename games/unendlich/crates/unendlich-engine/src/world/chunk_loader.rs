//! Async chunk loader.
//!
//! Architecture:
//!
//! ```text
//!   game thread -> [request channel] -> worker thread(s)
//!                                            |
//!                                            v
//!                                     TerrainGenerator (CPU or GPU)
//!                                            |
//!                                            v
//!   game thread <- [chunk channel] <- worker thread(s)
//! ```
//!
//! Workers pull batches of nearby chunk requests off a priority queue keyed
//! on distance to the player. The GPU path benefits massively from batching,
//! and the CPU path is fine with it too — Rayon parallelizes the inner loop.

use crate::constants::GPU_MAX_QUEUED_CHUNKS;
use crate::world::chunk::{Chunk, ChunkCoord};
use crate::world::octree::OctreeArray;
use crate::world::terrain_generator::TerrainGenerator;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use parking_lot::Mutex;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone, Copy)]
pub struct ChunkRequest {
    pub coord: ChunkCoord,
    pub priority: i32,
}

impl PartialEq for ChunkRequest {
    fn eq(&self, other: &Self) -> bool { self.priority == other.priority }
}
impl Eq for ChunkRequest {}
impl PartialOrd for ChunkRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}
impl Ord for ChunkRequest {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority value = higher importance (closer to player).
        // BinaryHeap is a max-heap, so we invert.
        other.priority.cmp(&self.priority)
    }
}

/// What a worker emits per generated chunk: the dense `Chunk` plus its
/// sparse `OctreeArray` encoding. Building the octree on the worker
/// parallelizes naturally with worker count and keeps the build cost
/// off the main thread's drain path.
///
/// v0.24.23 propagation: previously the channel sent bare `Chunk`. The
/// octree is now built alongside so consumers (ChunkManager, the
/// future GPU mesher, physics, raycast, item region reads) all see a
/// chunk-and-octree pair as the unit of chunk-loaded state.
#[derive(Debug)]
pub struct LoadedChunk {
    pub chunk: Chunk,
    pub octree: OctreeArray,
    /// True when the worker already applied the on-disk edit records for
    /// this chunk (and built `octree` from the edited blocks). The main
    /// thread must not load them again — and must not rebuild the
    /// octree for `chunk.dirty`, because the dirt IS those edits.
    pub edits_applied: bool,
}

pub struct ChunkLoader {
    request_tx: Sender<()>,
    chunk_rx: Receiver<LoadedChunk>,
    queue: Arc<Mutex<PendingQueue>>,
    /// 2026-09-02: the persistent world root, shared with the workers so
    /// edit records are read from disk and applied ON THE WORKER. Until
    /// now `ChunkManager::update` did that on the main thread per
    /// arriving chunk — a file read plus, for any edited chunk, a 32³
    /// octree rebuild — in the frame. `None` = no persistence.
    world_root: Arc<Mutex<Option<PathBuf>>>,
    /// Hold a clone of the generator so accessors like `model_arc()`
    /// can return its world-model without going through workers.
    generator: Arc<TerrainGenerator>,
    _workers: Vec<thread::JoinHandle<()>>,
}

struct PendingQueue {
    /// Priority heap of pending requests. Ordered by `ChunkRequest::priority`
    /// (closer chunks first when player position drives priority). May
    /// contain STALE entries — see `wanted` below — so a pop is only
    /// valid if it matches the authoritative priority there.
    heap: BinaryHeap<ChunkRequest>,
    /// Authoritative "is this coord still wanted, and at what priority"
    /// map. This replaces the old `queued: HashSet` mirror.
    ///
    /// Two things a plain heap+set can't do that this enables:
    ///   - Priority raise: if a coord is re-requested with a MORE urgent
    ///     priority (player turned toward it, or it entered a closer
    ///     ring), we push a second heap entry and bump the value here.
    ///     The heap ends up with a stale low-priority duplicate; that's
    ///     fine, it's discarded lazily on pop (classic decrease-key via
    ///     duplicate insert, same trick C2ME's 0.2.0-alpha.11 priority
    ///     rewrite uses for "urgent loads").
    ///   - Cancellation: `cancel()` / `retain_wanted()` just remove the
    ///     entry from this map. Stale heap entries for a cancelled coord
    ///     are silently dropped when popped, no generation work is ever
    ///     started for them. Previously a coord queued while the player
    ///     was moving toward it, then abandoned when they turned away,
    ///     would still generate and take a worker slot before anyone
    ///     noticed it was unwanted.
    ///
    /// INVARIANT: a heap entry is "live" iff `wanted.get(&coord) ==
    /// Some(&entry.priority)`. Pop discards anything else.
    wanted: std::collections::HashMap<ChunkCoord, i32>,
    /// Coords currently being generated by a worker (removed from
    /// `wanted`, not yet sent back to the manager). Tracked separately so
    /// we don't redundantly request a chunk that's already being made,
    /// and so cancellation never touches work already in flight.
    in_flight: HashSet<ChunkCoord>,
}

impl ChunkLoader {
    pub fn new(generator: Arc<TerrainGenerator>, num_workers: usize) -> Self {
        let world_root: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let queue = Arc::new(Mutex::new(PendingQueue {
            heap: BinaryHeap::with_capacity(256),
            wanted: std::collections::HashMap::with_capacity(1024),
            in_flight: HashSet::with_capacity(256),
        }));

        // The "wake worker" channel — workers block on this and check the queue.
        let (request_tx, request_rx) = bounded::<()>(GPU_MAX_QUEUED_CHUNKS);
        let (chunk_tx, chunk_rx) = unbounded::<LoadedChunk>();

        let mut workers = Vec::with_capacity(num_workers);
        for worker_id in 0..num_workers {
            let queue = Arc::clone(&queue);
            let generator = Arc::clone(&generator);
            let request_rx = request_rx.clone();
            let chunk_tx = chunk_tx.clone();
            let world_root = Arc::clone(&world_root);

            let handle = thread::Builder::new()
                .name(format!("chunk-loader-{}", worker_id))
                .spawn(move || {
                    // v0.24.22-worker-isolation: deprioritize chunk-loader
                    // threads so they yield to render + mesh work. This is
                    // the surgical change identified by Increment 61 as the
                    // documented fix for the chug pattern (CPU contention
                    // between gen / mesh / render on shared P-cores).
                    crate::thread_priority::configure_current_thread(
                        crate::thread_priority::ThreadRole::ChunkLoader,
                    );
                    Self::worker_loop(worker_id, queue, generator, request_rx, chunk_tx, world_root);
                })
                .expect("Failed to spawn chunk loader worker");

            workers.push(handle);
        }

        Self {
            request_tx,
            chunk_rx,
            queue,
            world_root,
            generator,
            _workers: workers,
        }
    }

    /// Get an `Arc` clone of the world's field model. Plumbed through
    /// to `MeshRequest` so the mesher can sample temperature/humidity
    /// per face for material-response tinting.
    pub fn model_arc(&self) -> Arc<crate::world::world_model::WorldModel> {
        self.generator.model_arc()
    }

    /// Submit a chunk request. Already-in-flight requests are dropped
    /// silently. A request for a coord that's already queued only takes
    /// effect if it's MORE urgent than what's queued (priority raise) —
    /// otherwise it's a no-op, same as before.
    pub fn request(&self, coord: ChunkCoord, priority: i32) {
        let mut q = self.queue.lock();
        if q.in_flight.contains(&coord) { return; }
        if let Some(&existing) = q.wanted.get(&coord) {
            if existing <= priority { return; } // already queued at >= urgency
        }
        q.wanted.insert(coord, priority);
        q.heap.push(ChunkRequest { coord, priority });
        drop(q);

        // Wake a worker. If the channel is full there's already plenty queued.
        let _ = self.request_tx.try_send(());
    }

    /// Submit many requests in one go. Caller pre-computes priorities (usually
    /// distance-squared from the player). Same priority-raise behavior as
    /// `request`: a coord already queued at an equal-or-better priority is
    /// skipped; a coord re-requested more urgently gets bumped.
    pub fn request_many(&self, requests: &[(ChunkCoord, i32)]) {
        if requests.is_empty() { return; }

        {
            let mut q = self.queue.lock();
            for &(coord, priority) in requests {
                if q.in_flight.contains(&coord) { continue; }
                if let Some(&existing) = q.wanted.get(&coord) {
                    if existing <= priority { continue; }
                }
                q.wanted.insert(coord, priority);
                q.heap.push(ChunkRequest { coord, priority });
            }
        }

        for _ in 0..requests.len().min(GPU_MAX_QUEUED_CHUNKS) {
            let _ = self.request_tx.try_send(());
        }
    }

    /// Cancel a single queued (not yet in-flight) request. No-op if it's
    /// already being generated or wasn't queued. In-flight work is left
    /// alone — cancelling only stops work that hasn't started, same
    /// scope as C2ME's chunk-cancellation mechanism.
    pub fn cancel(&self, coord: ChunkCoord) {
        let mut q = self.queue.lock();
        q.wanted.remove(&coord);
    }

    /// Drop every queued request whose coord doesn't satisfy `keep`.
    /// Call this after the player moves far enough that a chunk of the
    /// previous request batch is no longer worth generating — e.g. once
    /// per second with `keep = |c| c.distance_sq(player_chunk) <=
    /// render_distance_sq`. Cheap: one lock, one `retain` over `wanted`
    /// (bounded by queue depth, not chunk count), no heap rebuild.
    pub fn retain_wanted<F>(&self, keep: F)
    where
        F: Fn(ChunkCoord) -> bool,
    {
        let mut q = self.queue.lock();
        q.wanted.retain(|&coord, _| keep(coord));
    }

    /// Drain all chunks that have finished generating since the last call.
    /// Each item carries the chunk AND its octree encoding (built on the
    /// worker thread, ready for ChunkManager to install into both maps).
    pub fn drain_completed(&self) -> Vec<LoadedChunk> {
        self.chunk_rx.try_iter().collect()
    }

    /// Drain completed chunks as a borrowing iterator — no intermediate `Vec`.
    /// The per-tick `chunk_manager` paths consume results immediately in a `for`
    /// loop, so this avoids a heap allocation each `update()`. `try_iter` yields
    /// the same items in the same order; `drain_completed` (the `Vec` form) is
    /// kept for callers that need to count or retain the batch.
    pub fn drain_completed_iter(&self) -> impl Iterator<Item = LoadedChunk> + '_ {
        self.chunk_rx.try_iter()
    }

    /// How many chunks are still waiting to be generated. Uses `wanted`,
    /// not `heap.len()` — the heap may hold stale superseded-priority
    /// duplicates that don't represent real pending work.
    pub fn pending_count(&self) -> usize {
        let q = self.queue.lock();
        q.wanted.len() + q.in_flight.len()
    }

    /// Point the workers at the persistent world root. Chunks completed
    /// after this call arrive with their edits applied.
    pub fn set_world_root(&self, root: Option<PathBuf>) {
        *self.world_root.lock() = root;
    }

    fn worker_loop(
        _worker_id: usize,
        queue: Arc<Mutex<PendingQueue>>,
        generator: Arc<TerrainGenerator>,
        request_rx: Receiver<()>,
        chunk_tx: Sender<LoadedChunk>,
        world_root: Arc<Mutex<Option<PathBuf>>>,
    ) {
        while request_rx.recv().is_ok() {
            // Drain extra wake signals — we'll process whatever's queued.
            while request_rx.try_recv().is_ok() {}

            let coords: Vec<ChunkCoord> = {
                let mut q = queue.lock();
                let take_n = generator.batch_size().max(1);
                let mut taken = Vec::with_capacity(take_n);
                while taken.len() < take_n {
                    match q.heap.pop() {
                        Some(req) => {
                            // Only act on this entry if it's still the
                            // live one for its coord. A stale duplicate
                            // (superseded by a priority raise) or a
                            // cancelled coord (removed via `cancel` /
                            // `retain_wanted`) won't match and is
                            // silently discarded — no generation work
                            // is started for it.
                            match q.wanted.get(&req.coord) {
                                Some(&cur) if cur == req.priority => {
                                    q.wanted.remove(&req.coord);
                                    q.in_flight.insert(req.coord);
                                    taken.push(req.coord);
                                }
                                _ => continue,
                            }
                        }
                        None => break,
                    }
                }
                taken
            };

            if coords.is_empty() { continue; }

            // PERF: stream chunks back ONE AT A TIME as each finishes
            // rather than waiting for the whole batch.
            //
            // Previously: `generator.generate(&coords)` returned Vec<Chunk>
            // with all 16 chunks at once. With ~80ms/chunk on CPU, the
            // user saw gen_pending=24 stuck for 1.3+ seconds while a
            // worker chewed through its batch, then all chunks landed
            // simultaneously. That LOOKED like a freeze.
            //
            // Now: each chunk is generated and shipped immediately to
            // the chunk channel. The main thread sees a steady stream
            // of completions instead of bursty batch deliveries.
            //
            // For GPU path, batching IS necessary (one dispatch =
            // multiple chunks). The streaming benefit applies only
            // to CPU. We use the per-chunk path which dispatches
            // through generator.generate(&[single]) — for CPU this
            // is the cheap path; for GPU this gives up some efficiency
            // (one-chunk batches) but the GPU path isn't the user's
            // configuration today (Apple M2 Max doesn't have the
            // required Vulkan feature, GPU generation falls back to
            // CPU). When GPU comes back, we can branch on the path
            // and batch only there.
            for &coord in &coords {
                let single = generator.generate(&[coord]);
                {
                    let mut q = queue.lock();
                    q.in_flight.remove(&coord);
                }
                for mut chunk in single {
                    // 2026-09-02: apply persisted edits here, before the
                    // octree is built, so the main thread neither touches
                    // the disk nor rebuilds the octree for an edited chunk.
                    let root = world_root.lock().clone();
                    let edits_applied = match root {
                        Some(root) => {
                            match crate::storage::load_chunk_edits(&root, &mut chunk) {
                                Ok(0) => {}
                                Ok(n) => log::debug!("loader: {} edits applied for {:?}", n, chunk.coord),
                                Err(e) => log::warn!("loader: edit load failed for {:?}: {}", chunk.coord, e),
                            }
                            true
                        }
                        None => false,
                    };
                    // v0.24.23 propagation: build the sparse octree on
                    // the worker so the per-chunk cost (~ a few hundred
                    // microseconds, dominated by 32K cell reads) is
                    // parallelized across the worker pool. The receiver
                    // (ChunkManager) gets a chunk-and-octree pair as
                    // one unit.
                    let octree = OctreeArray::from_chunk(&chunk);
                    let loaded = LoadedChunk { chunk, octree, edits_applied };
                    if chunk_tx.send(loaded).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Compute the priority of a chunk request relative to a player position.
/// Lower = higher priority. Squared distance keeps things integer-only.
pub fn priority_for(player_chunk: ChunkCoord, target: ChunkCoord) -> i32 {
    target.distance_sq(player_chunk)
}

/// Compute world-space coordinates for a player-relative chunk loading region.
/// 3D Chebyshev radius: returns all chunks with |dx|, |dy|, |dz| <= radius.
pub fn chunks_in_radius(center: ChunkCoord, radius: i32) -> Vec<(ChunkCoord, i32)> {
    let side = (2 * radius + 1) as usize;
    let mut out = Vec::with_capacity(side * side * side);
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                // Sphere cull (Euclidean radius, slightly tighter than Chebyshev).
                if dx * dx + dy * dy + dz * dz > radius * radius { continue; }
                let coord = ChunkCoord::new(center.x + dx, center.y + dy, center.z + dz);
                out.push((coord, priority_for(center, coord)));
            }
        }
    }
    out.sort_by_key(|(_, p)| *p);
    out
}

/// Convert a world-space block position to chunk coordinates.
pub fn block_to_chunk(world_x: i32, world_y: i32, world_z: i32) -> ChunkCoord {
    ChunkCoord::from_block_pos(world_x, world_y, world_z)
}
