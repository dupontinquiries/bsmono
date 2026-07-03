//! Async meshing pool.
//!
//! Computing a chunk mesh takes a few milliseconds (naive face culling on a
//! 32^3 chunk = ~33k cells * 6 face checks). Doing that on the render thread
//! costs visible frame hitches when multiple chunks need (re)meshing in the
//! same frame.
//!
//! This pool runs N worker threads that pull chunk data off a request channel,
//! call `mesh_chunk`, and push the result onto a result channel. The render
//! thread consumes results without ever blocking on mesh CPU.
//!
//! Backpressure: we use a bounded request channel. If the queue is full,
//! `submit` returns `false` and the caller can decide whether to drop the
//! request (acceptable for cosmetic remeshes) or retry next frame. For the
//! initial chunk-loading flow, the chunk-streaming layer already paces
//! requests (only the closest N coords are active at any time), so the
//! bounded queue is more of a safety net than a hot path.
//!
//! v0.24.21-gpu-mesher-prod: when constructed via [`MeshingPool::with_gpu_mesher`],
//! the pool gains a single extra "GPU dispatcher" thread that owns the
//! [`SparseGpuMesher`]. Workers handle the cheap `extract_surface` pass on
//! CPU (a face-bit scan) and forward Near-LOD chunks to the dispatcher over
//! an internal channel. The dispatcher batches up to MAX_BATCH_CHUNKS
//! requests (or waits up to `GPU_BATCH_TIMEOUT`) and issues a single GPU
//! compute submission. Per-chunk GPU dispatch floor (~550 µs on AGX) amortizes
//! across the batch.
//!
//! Mid/Far/VeryFar tiers continue to use the CPU mesher even when GPU is
//! enabled — they don't have enough vertices per chunk to amortize a GPU
//! submission, and the downsample path is already cheap (sub-millisecond).
//!
//! GPU is opt-in via `UNENDLICH_GPU_MESHER=1`. Default is CPU-only,
//! backward-compatible.

use crate::render::mesher::ChunkMesh;
use crate::world::chunk::{Chunk, ChunkCoord};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Process-wide count of chunks that overflowed the GPU mesher
/// (`ERROR_OUT_OF_DEVICE_MEMORY`) and fell back to the CPU greedy mesher this
/// run. A non-zero value at shutdown means device-memory pressure forced
/// untinted CPU-fallback chunks — the cause of the OOM stripe (see
/// docs/MEMORY-PRESSURE.md). Surfaced in the run summary as `gpu_oom_fallback`.
pub static GPU_OOM_FALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the run's GPU-OOM-fallback count (see [`GPU_OOM_FALLBACK_COUNT`]).
pub fn gpu_oom_fallback_count() -> u64 {
    GPU_OOM_FALLBACK_COUNT.load(Ordering::Relaxed)
}

/// v0.30: process-wide count of Near-LOD chunks that took the CPU greedy mesher
/// while the octree GPU mesher was active (`UNENDLICH_GPU_MESHER=2`). This is the
/// octree-absent-at-submit fallback — the source of the multi-hundred-ms `near`
/// mesher spikes (the GPU path forwards Near in ~0.3ms; this path runs the full
/// greedy mesh on the worker thread). Surfaced in the run summary as
/// `near_cpu_fallback` so we can size the fix before changing behaviour.
pub static NEAR_CPU_FALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the run's Near-CPU-fallback count (see [`NEAR_CPU_FALLBACK_COUNT`]).
pub fn near_cpu_fallback_count() -> u64 {
    NEAR_CPU_FALLBACK_COUNT.load(Ordering::Relaxed)
}

/// One unit of work for a worker: mesh this chunk, label result with this
/// version stamp so consumers can ignore stale results when chunks have been
/// re-edited since they were submitted.
pub struct MeshRequest {
    pub coord: ChunkCoord,
    /// Snapshot of the chunk to mesh. We hold an `Arc<Chunk>` so it's cheap
    /// to clone and doesn't block edits on the original. If the chunk changes
    /// after submission, this snapshot is stale-but-safe.
    pub chunk: Arc<Chunk>,
    /// World epoch at submission (r195). Stale results from a world that
    /// has since been reset (new seed) are dropped at upload even when the
    /// coord is re-loaded at the same version 0 in the new world — the
    /// version check alone can't see a cross-world collision.
    pub epoch: u64,
    /// The chunk's version at submission time. Result carries this so the
    /// consumer can detect stale meshes.
    pub version_at_submit: u64,
    /// Sky snapshot at submission time. Drives per-face sun lighting in the
    /// generated mesh. The mesh "bakes in" this lighting; if the sun moves
    /// significantly (across day/night boundary) the chunk will need re-meshing
    /// for the lighting to update.
    pub sky: crate::sky::Sky,
    /// Shared reference to the world's field model. Used by the mesher to
    /// sample temperature/humidity at each face for material-response
    /// tinting. `None` = use neutral environmental values (tests, or
    /// the legacy non-tinted path).
    pub world_model: Option<Arc<crate::world::world_model::WorldModel>>,
    /// Current simulation tick at submission time. The mesher uses this
    /// to compute the progress of in-flight visual transitions
    /// (`Chunk::transitions`) — `progress = (current_tick - started_tick)
    /// / TRANSITION_ANIM_TICKS`, clamped to [0, 1]. The mesh "bakes in"
    /// the progress at submission time; for a chunk to actually animate,
    /// it must be re-submitted as the tick advances. The lifecycle
    /// handler bumps chunk version each tick a transition is in flight,
    /// triggering re-mesh.
    ///
    /// `None` = use to_stage end-state for any active transitions
    /// (tests / fallback path with no tick-time information).
    pub current_tick: Option<u64>,
    /// LOD tier this chunk should be meshed at (v0.22.3, F3).
    /// Worker dispatches to the full-detail mesher for `Near`, or
    /// `mesh_chunk_downsampled(chunk, factor)` for any other tier
    /// where `factor = lod_settings.factor_for(lod)`.
    /// Default `Near` = backward-compatible with v0.22.0-2 behaviour
    /// (only the full-detail path was used before F3).
    pub lod: crate::world::lod::LodLevel,
    /// Reverse-regen generator handle (v0.23.7 Stage 2b).
    /// When `Some(...)` AND `lod` is Far or VeryFar, the worker uses
    /// `mesh_chunk_far_virtual(coord, generator, factor)` and ignores
    /// `chunk`. When `None`, behaves as before: materialised-grid path.
    ///
    /// For Near/Mid this field is ignored; those tiers always use the
    /// materialised path because they need block-level detail and/or
    /// edit support that virtual chunks don't provide.
    ///
    /// Lifetime: an Arc cloned at submit time. Worker drops it after
    /// meshing. Multiple workers share one `WorldGenerator` via the
    /// internal `Arc<WorldModel>` it holds.
    pub generator: Option<Arc<crate::world::cpu_generator::WorldGenerator>>,
    /// v0.24.27-octree-mesher: the chunk's octree at submission time.
    /// Used only when the pool was constructed with
    /// `with_octree_gpu_mesher` AND `lod == Near` — in that case the
    /// worker forwards `(coord, octree)` to the octree dispatcher
    /// instead of running the CPU mesher.
    ///
    /// `None` for backward compatibility; CPU-only / sparse-GPU paths
    /// don't read this field. If the pool is in octree mode but this
    /// field is None, the worker falls back to CPU mesher for that
    /// chunk (same fallback policy as sparse mode when records would
    /// overflow).
    pub octree: Option<Arc<crate::world::octree::OctreeArray>>,
}

/// Result delivered back to the render thread.
pub struct MeshResult {
    pub coord: ChunkCoord,
    pub mesh: ChunkMesh,
    /// r195: echoed world epoch (see MeshRequest::epoch).
    pub epoch: u64,
    /// Version this mesh was generated against.
    pub version: u64,
    /// LOD tier this mesh was built at. Plumbed through to the
    /// renderer's `upload_prebuilt_mesh_at_lod` so the slot key
    /// is correct.
    pub lod: crate::world::lod::LodLevel,
}

/// v0.24.21-gpu-mesher-prod: internal handoff from worker thread to
/// the GPU dispatcher thread. Workers do the cheap `extract_surface`
/// scan on CPU (a single pass of face-bit detection) then send the
/// records here instead of doing the expensive greedy meshing pass.
///
/// `records` is owned (`Vec`, not slice) so the dispatcher can hold
/// many of these across a batch window without lifetime gymnastics.
struct GpuMeshRequest {
    coord: ChunkCoord,
    records: Vec<crate::world::surface::SurfaceRecord>,
    version: u64,
    epoch: u64, // r195
    lod: crate::world::lod::LodLevel,
}

/// v0.24.27-octree-mesher: which GPU mesher backend (if any) the
/// pool is configured with. Mutually exclusive — at most one is
/// live per pool. The enum keeps the constructor surface clean
/// while letting `build()` thread one parameter through.
enum GpuMesherKind {
    None,
    Sparse(Arc<crate::render::gpu_meshing_sparse::SparseGpuMesher>),
    Octree(Arc<crate::render::gpu_meshing_octree::OctreeGpuMesher>),
}

/// v0.24.27-octree-mesher: internal handoff to the octree GPU
/// dispatcher. Carries an Arc<OctreeArray> (cheap to clone from the
/// ChunkManager's `octrees` DashMap). Workers do no CPU prep — the
/// dispatcher uploads octree bytes directly to GPU.
struct OctreeGpuMeshRequest {
    coord: ChunkCoord,
    octree: Arc<crate::world::octree::OctreeArray>,
    version: u64,
    epoch: u64, // r195
    lod: crate::world::lod::LodLevel,
    /// v0.24.33-climate-tint: per-chunk temperature in [0, 1].
    /// Computed at chunk-center world position via WorldModel.
    /// Cheap on the worker thread (the CPU mesher path that's
    /// being replaced by this pipeline was already paying this
    /// cost). Falls back to 0.5 (neutral) when world_model is
    /// unavailable.
    temperature: f32,
    humidity: f32,
    /// v0.24.NEXT+2-gpu-decorate: chunk snapshot + tick carried so the
    /// dispatcher can run the CPU decoration / fluid-surface post-passes
    /// on GPU-meshed chunks when `UNENDLICH_GPU_DECORATE=1`. The GPU
    /// octree mesher emits only solid block faces; grass tufts, water,
    /// and lava surfaces live in CPU-only passes (`decorate_chunk`,
    /// `emit_water_surface`, `emit_lava_surface`) that GPU-routed chunks
    /// otherwise never hit. The `Arc<Chunk>` clone is a refcount bump;
    /// `decorate_chunk` iterates only `chunk.plant_indices`, so chunks
    /// with no plants cost nothing even with the flag on.
    chunk: Arc<Chunk>,
    current_tick: Option<u64>,
    /// r501: per-chunk Sky so the CPU decoration / fluid-surface post-passes can
    /// sun-light tufts/water/lava instead of neutral (Sky is Copy — the earlier
    /// "Clone-ability unconfirmed" blocker is resolved).
    sky: crate::sky::Sky,
    /// v0.24.NEXT-stripe-fix: the world field model, carried so the OOM
    /// CPU-fallback can bake the SAME climate tint as a normal CPU mesh
    /// (otherwise fallback chunks render as an untinted band — the stripe).
    /// `Arc` clone is a refcount bump; `None` = neutral tint (tests/legacy).
    world_model: Option<Arc<crate::world::world_model::WorldModel>>,
}

/// v0.24.21-gpu-mesher-prod: how long the GPU dispatcher will linger
/// after receiving its first request before forcing a dispatch. With
/// a stream of workers feeding requests, this is the worst-case latency
/// any individual chunk pays waiting for batchmates. Tuned to ~half a
/// frame at 60 FPS: low enough to feel responsive, high enough to
/// usefully batch.
const GPU_BATCH_TIMEOUT: Duration = Duration::from_millis(8);


pub struct MeshingPool {
    request_tx: Sender<MeshRequest>,
    result_rx: Receiver<MeshResult>,
    /// v0.24.21-gpu-mesher-prod: when Some, workers forward Near-LOD
    /// chunks here instead of running CPU greedy mesher. The GPU
    /// dispatcher thread (held in `gpu_dispatcher` below) drains this
    /// channel, batches, and submits to `SparseGpuMesher`.
    ///
    /// Closed (dropped) automatically when the pool itself drops.
    /// On drop, the dispatcher thread sees the closed channel and exits.
    gpu_request_tx: Option<Sender<GpuMeshRequest>>,
    /// v0.24.27-octree-mesher: parallel channel to the octree GPU
    /// dispatcher. Mutually exclusive with `gpu_request_tx` — `build`
    /// asserts at most one is set.
    octree_gpu_request_tx: Option<Sender<OctreeGpuMeshRequest>>,
    workers: Vec<JoinHandle<()>>,
    /// v0.24.21-gpu-mesher-prod: the single GPU dispatcher thread,
    /// present iff GPU meshing is enabled.
    gpu_dispatcher: Option<JoinHandle<()>>,
    /// v0.24.27-octree-mesher: the single octree GPU dispatcher thread,
    /// present iff octree GPU meshing is enabled.
    octree_gpu_dispatcher: Option<JoinHandle<()>>,
    queue_capacity: usize,
    /// v0.23.1: per-LOD timing for the worker dispatch. Shared via
    /// Arc with each worker thread; the Mutex inside MesherTimer
    /// is uncontended in practice (one acquire per chunk meshing
    /// call, ~1-200ms apart).
    mesher_timer: Arc<crate::render::mesher_timer::MesherTimer>,
}

impl MeshingPool {
    /// Spin up `num_workers` worker threads with the given queue capacity.
    /// `queue_capacity` is the number of pending mesh requests — pick at
    /// least 2× the per-frame upload cap so workers stay busy.
    ///
    /// `lod_settings` (v0.22.3, F3) controls which downsample factor each
    /// worker uses for non-Near tiers. Workers share the settings via Arc;
    /// changes after construction don't propagate today (settings are
    /// effectively immutable per-pool). Future settings UI work can move
    /// to a swap-via-mutex if hot-reload becomes important.
    pub fn new(
        num_workers: usize,
        queue_capacity: usize,
        lod_settings: Arc<crate::world::lod::LodSettings>,
    ) -> Self {
        // CPU-only path; backward-compatible with all callers pre-v0.24.21.
        Self::build(num_workers, queue_capacity, lod_settings, GpuMesherKind::None)
    }

    /// v0.24.21-gpu-mesher-prod: construct the pool wired to a shared
    /// `SparseGpuMesher`. Workers will route Near-LOD chunks through
    /// the GPU dispatcher; Mid/Far/VeryFar continue to use the CPU
    /// mesher (downsample path).
    ///
    /// The pool owns an `Arc<SparseGpuMesher>` clone for the lifetime
    /// of the dispatcher thread. When the pool drops, the dispatcher
    /// channel closes, the dispatcher thread exits, and the Arc is
    /// dropped — the caller's clone (held in the renderer for cleanup
    /// ordering) is then the last reference.
    pub fn with_gpu_mesher(
        num_workers: usize,
        queue_capacity: usize,
        lod_settings: Arc<crate::world::lod::LodSettings>,
        gpu_mesher: Arc<crate::render::gpu_meshing_sparse::SparseGpuMesher>,
    ) -> Self {
        Self::build(num_workers, queue_capacity, lod_settings,
                    GpuMesherKind::Sparse(gpu_mesher))
    }

    /// v0.24.27-octree-mesher: construct the pool wired to a shared
    /// `OctreeGpuMesher`. Workers route Near-LOD chunks to the octree
    /// dispatcher (which reads from each chunk's `OctreeArray`
    /// directly instead of CPU-extracted records). Mid/Far/VeryFar
    /// continue to use the CPU mesher.
    ///
    /// Activated via `UNENDLICH_GPU_MESHER=2` in the bin. Mutually
    /// exclusive with `with_gpu_mesher`; `build()` panics if both
    /// are wired.
    pub fn with_octree_gpu_mesher(
        num_workers: usize,
        queue_capacity: usize,
        lod_settings: Arc<crate::world::lod::LodSettings>,
        octree_mesher: Arc<crate::render::gpu_meshing_octree::OctreeGpuMesher>,
    ) -> Self {
        Self::build(num_workers, queue_capacity, lod_settings,
                    GpuMesherKind::Octree(octree_mesher))
    }

    fn build(
        num_workers: usize,
        queue_capacity: usize,
        lod_settings: Arc<crate::world::lod::LodSettings>,
        gpu_mesher_kind: GpuMesherKind,
    ) -> Self {
        assert!(num_workers > 0, "meshing pool needs at least one worker");
        assert!(queue_capacity > 0, "queue capacity must be positive");

        let (request_tx, request_rx) = bounded::<MeshRequest>(queue_capacity);
        let (result_tx, result_rx) = bounded::<MeshResult>(queue_capacity * 2);
        let mesher_timer = Arc::new(crate::render::mesher_timer::MesherTimer::new());

        // Optional GPU mesh request channels — at most one of these
        // is set per pool (sparse-records or octree). Sized to
        // queue_capacity so back-pressure mirrors the CPU request
        // channel — a stuck dispatcher won't grow unbounded.
        let (gpu_request_tx, gpu_dispatcher, octree_gpu_request_tx, octree_gpu_dispatcher) =
            match gpu_mesher_kind {
                GpuMesherKind::None => (None, None, None, None),
                GpuMesherKind::Sparse(mesher) => {
                    let (tx, rx) = bounded::<GpuMeshRequest>(queue_capacity);
                    let res_tx_for_disp = result_tx.clone();
                    let timer_for_disp = Arc::clone(&mesher_timer);
                    let h = thread::Builder::new()
                        .name("gpu-mesh-dispatcher".to_string())
                        .spawn(move || {
                            crate::thread_priority::configure_current_thread(
                                crate::thread_priority::ThreadRole::GpuDispatcher,
                            );
                            gpu_dispatcher_loop(rx, res_tx_for_disp, mesher, timer_for_disp)
                        })
                        .expect("spawn gpu-mesh-dispatcher");
                    log::info!("MeshingPool: GPU mesher (sparse) dispatcher started");
                    (Some(tx), Some(h), None, None)
                }
                GpuMesherKind::Octree(mesher) => {
                    let (tx, rx) = bounded::<OctreeGpuMeshRequest>(queue_capacity);
                    let res_tx_for_disp = result_tx.clone();
                    let timer_for_disp = Arc::clone(&mesher_timer);
                    let h = thread::Builder::new()
                        .name("octree-gpu-mesh-dispatcher".to_string())
                        .spawn(move || {
                            crate::thread_priority::configure_current_thread(
                                crate::thread_priority::ThreadRole::GpuDispatcher,
                            );
                            octree_gpu_dispatcher_loop(rx, res_tx_for_disp, mesher, timer_for_disp)
                        })
                        .expect("spawn octree-gpu-mesh-dispatcher");
                    log::info!("MeshingPool: GPU mesher (octree) dispatcher started");
                    (None, None, Some(tx), Some(h))
                }
            };

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let req_rx = request_rx.clone();
            let res_tx = result_tx.clone();
            let settings = Arc::clone(&lod_settings);
            let timer = Arc::clone(&mesher_timer);
            let gpu_tx = gpu_request_tx.clone();
            let octree_gpu_tx = octree_gpu_request_tx.clone();
            let h = thread::Builder::new()
                .name(format!("mesher-{}", i))
                .spawn(move || {
                    // v0.24.22-worker-isolation: mesh workers run at default
                    // priority on whatever cores the scheduler picks (P-cores
                    // in practice). With chunk-loaders pinned to E-cores by
                    // the ChunkLoader role, mesh workers no longer compete
                    // for P-core time with terrain gen.
                    crate::thread_priority::configure_current_thread(
                        crate::thread_priority::ThreadRole::MeshWorker,
                    );
                    worker_loop(req_rx, res_tx, settings, timer, gpu_tx, octree_gpu_tx)
                })
                .expect("spawn meshing worker");
            workers.push(h);
        }

        // Drop the original request_rx and result_tx — workers each hold their
        // own clones, and the channel will close when all of them exit. The
        // caller-side request_tx and result_rx remain on `Self`.
        drop(request_rx);
        drop(result_tx);

        Self {
            request_tx, result_rx,
            gpu_request_tx, workers, gpu_dispatcher,
            octree_gpu_request_tx, octree_gpu_dispatcher,
            queue_capacity, mesher_timer,
        }
    }

    /// Format and reset the per-LOD mesher timing window. One-line
    /// suitable for the FPS log block. See
    /// `crate::render::mesher_timer` for layout.
    pub fn flush_mesher_log(&self) -> String {
        self.mesher_timer.flush_log()
    }

    /// Format the lifetime per-LOD mesher stats. Multi-line; for
    /// shutdown.
    pub fn flush_mesher_summary(&self) -> String {
        self.mesher_timer.flush_lifetime_summary()
    }

    /// Try to submit a mesh request. Returns `true` if accepted, `false` if
    /// the queue is full (caller may retry next frame).
    pub fn submit(&self, req: MeshRequest) -> bool {
        match self.request_tx.try_send(req) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => {
                log::error!("meshing pool disconnected — workers gone");
                false
            }
        }
    }

    /// Drain all completed mesh results. Returns immediately, never blocks.
    /// The render thread should call this once per frame and feed results
    /// to the GPU-upload code.
    pub fn drain_completed(&self) -> Vec<MeshResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.result_rx.try_recv() {
            out.push(r);
        }
        out
    }

    /// Try to receive at most `limit` completed mesh results. Useful when the
    /// caller wants to bound per-frame work.
    pub fn drain_up_to(&self, limit: usize) -> Vec<MeshResult> {
        let mut out = Vec::with_capacity(limit);
        for _ in 0..limit {
            match self.result_rx.try_recv() {
                Ok(r) => out.push(r),
                Err(_) => break,
            }
        }
        out
    }

    /// Size of the request queue. Useful for diagnostics.
    pub fn queue_capacity(&self) -> usize { self.queue_capacity }

    /// Number of pending requests (approximate; channel state is racy).
    pub fn pending_requests(&self) -> usize { self.request_tx.len() }

    /// Number of completed but unconsumed results.
    pub fn pending_results(&self) -> usize { self.result_rx.len() }
}

impl Drop for MeshingPool {
    fn drop(&mut self) {
        // v0.24.2c.5: fix shutdown-hang in MeshingPool::Drop.
        //
        // OLD behaviour: drop request_tx to close the request channel,
        // then join workers. Each worker's loop was:
        //   while let Ok(req) = req_rx.recv() {
        //       let mesh = mesh_chunk(...);          // 30-50ms typical
        //       if res_tx.send(result).is_err() { break; }
        //   }
        //
        // FAILURE: if the bounded result channel (capacity =
        // queue_capacity * 2) is FULL when shutdown begins — which
        // happens when the render loop stops draining results before
        // workers stop producing them — every worker mid-cycle blocks
        // forever in `res_tx.send`. They never check `req_rx.recv()`
        // again because they haven't returned to the loop top.
        //
        // FIX: drop `result_rx` BEFORE dropping `request_tx` and
        // joining. With `result_rx` gone, `res_tx.send` errors out
        // immediately on any blocked worker, the worker `break`s,
        // and joins return. Workers waiting in `req_rx.recv()` still
        // exit cleanly when `request_tx` drops below.
        //
        // We replace the receiver with a one-element disconnected
        // channel's receiver to keep the field's type intact while
        // dropping the real one (similar pattern to the request_tx
        // replacement).
        let (_, dummy_rx) = bounded::<MeshResult>(1);
        let real_rx = std::mem::replace(&mut self.result_rx, dummy_rx);
        drop(real_rx);

        // Close the request channel; workers will exit when they next try
        // to receive. We replace `request_tx` with a one-element channel
        // so the original is dropped.
        let (dummy_tx, _) = bounded::<MeshRequest>(1);
        let real_tx = std::mem::replace(&mut self.request_tx, dummy_tx);
        drop(real_tx);

        // v0.24.21-gpu-mesher-prod: close the GPU request channel.
        // Workers may still hold clones if they haven't yet exited
        // their recv loop, but once they break on req_rx closing, the
        // last Sender drops and the dispatcher sees the closed channel.
        // Dropping the outer Sender here is necessary to unblock the
        // dispatcher when it's currently in `rx.recv()` waiting for
        // the FIRST request of a new batch.
        self.gpu_request_tx.take();
        // v0.24.27-octree-mesher: same shutdown signal for octree
        // dispatcher.
        self.octree_gpu_request_tx.take();

        for h in self.workers.drain(..) {
            let _ = h.join();
        }

        // Workers' clones of `gpu_request_tx` are dropped when their
        // threads exit, which is after the join above. With the pool's
        // own sender already taken and dropped, the dispatcher channel
        // is now fully closed and `gpu_dispatcher_loop` exits.
        if let Some(h) = self.gpu_dispatcher.take() {
            let _ = h.join();
        }
        if let Some(h) = self.octree_gpu_dispatcher.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(
    req_rx: Receiver<MeshRequest>,
    res_tx: Sender<MeshResult>,
    lod_settings: Arc<crate::world::lod::LodSettings>,
    mesher_timer: Arc<crate::render::mesher_timer::MesherTimer>,
    gpu_request_tx: Option<Sender<GpuMeshRequest>>,
    octree_gpu_request_tx: Option<Sender<OctreeGpuMeshRequest>>,
) {
    use crate::render::mesher::{
        mesh_chunk_with_sky, mesh_chunk_with_sky_and_model,
        mesh_chunk_with_sky_model_and_tick, mesh_chunk_downsampled,
        mesh_chunk_far_virtual,
    };
    use crate::world::lod::LodLevel;
    use crate::render::gpu_meshing_sparse::MAX_RECORDS_PER_CHUNK;
    // (removed: redundant, already in use)
    // T6 (hot env reads): UNENDLICH_GPU_MESHER_MID and UNENDLICH_NEUTRAL_TINT are
    // launch-time A/B flags (not per-frame tunables), but were being re-read via
    // std::env::var on EVERY dequeued request inside the loop below. Read them ONCE
    // here — same behavior, no per-request env syscall during streaming. (Mirrors the
    // octree dispatcher, which already reads its copies once at thread start.)
    let mid_via_gpu = std::env::var("UNENDLICH_GPU_MESHER_MID")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    let neutral_tint = std::env::var("UNENDLICH_NEUTRAL_TINT")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    // r476: skip dispatching pure-air chunks to the GPU mesher (they always mesh
    // to nothing). Default ON — correctness-preserving, removes a large slice of
    // streaming dispatch load (flying / over-surface is mostly air). Set
    // UNENDLICH_SKIP_AIR_CHUNKS=0 to force every chunk through the mesher (A/B).
    let skip_air_chunks = std::env::var("UNENDLICH_SKIP_AIR_CHUNKS")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    while let Ok(req) = req_rx.recv() {
        let t0 = Instant::now();

        // v0.24.27-octree-mesher: if octree GPU mesher is wired and
        // this is a Near LOD chunk with an octree available, route
        // to the octree dispatcher. The octree mesher needs no CPU
        // prep — it reads the octree bytes directly. Workers do
        // zero meshing work for these chunks; the dispatcher batches
        // and submits.
        //
        // v0.24.39-mid-lod: when `UNENDLICH_GPU_MESHER_MID=1` is set,
        // Mid LOD chunks also route through the GPU mesher (shader
        // iterates 16³ cells with stride 2). Default off so the
        // ship can be A/B compared against the v0.24.38 baseline.
        //
        // Fallback: if the request didn't carry an octree (which
        // shouldn't happen when octree mode is on, but defensively),
        // fall through to the existing path. CPU mesher will produce
        // a valid mesh; this prevents silent dropped requests.
        let route_to_gpu = match req.lod {
            LodLevel::Near => true,
            LodLevel::Mid  => mid_via_gpu,
            _              => false,
        };
        if let (true, Some(ref octree_tx), Some(ref octree)) =
            (route_to_gpu, octree_gpu_request_tx.as_ref(), req.octree.as_ref())
        {
            let elapsed_us = t0.elapsed().as_micros() as u64;
            mesher_timer.record(req.lod, elapsed_us);

            // r476: skip pure-air chunks entirely. A fully-collapsed air octree
            // meshes to nothing (air emits no faces, regardless of neighbors),
            // yet dispatching it still costs the GPU ~7 latency-bound octree
            // walks on every one of 32768 cells just to emit an empty mesh. The
            // field log had ~42% of dispatches return empty, and a flying dragon
            // spends most of its time over air — so this short-circuit removes a
            // large slice of the streaming dispatch load. Return the empty mesh
            // directly and skip the GPU round-trip. Only the trivially-safe
            // uniform-air case is skipped (OctreeArray::is_uniform_air).
            if skip_air_chunks && octree.is_uniform_air() {
                let _ = res_tx.send(MeshResult {
                    coord: req.coord,
                    mesh: ChunkMesh::empty(),
                    epoch: req.epoch,
                    version: req.version_at_submit,
                    lod: req.lod,
                });
                continue;
            }

            // v0.24.33-climate-tint: sample temperature/humidity at
            // the chunk's center for shader-side color modulation.
            // One pair of world_model calls per chunk — same cost
            // the legacy CPU mesher paid per-block.
            let (chunk_x, chunk_y, chunk_z) = req.coord.block_origin();
            let center_x = chunk_x as f64 + 16.0;
            let center_y = chunk_y as f64 + 16.0;
            let center_z = chunk_z as f64 + 16.0;
            // NEUTRAL_TINT (hoisted to thread-start above) selects uniform neutral
            // climate vs sampled temperature/humidity. Default off => byte-identical.
            let (temperature, humidity) = if neutral_tint {
                // Tint disabled: uniform neutral climate (see flag above).
                (0.5f32, 0.5f32)
            } else {
                match req.world_model.as_ref() {
                    Some(m) => {
                        let base_t = m.temperature(center_x, center_y, center_z, 0.0);
                        let base_h = m.humidity(center_x, center_y, center_z, 0.0);
                        // v0.24.NEXT+4-seasons: apply the seasonal render-tint
                        // delta on top of the *base* climate sample. The base
                        // fields (and thus worldgen, which reads them) are
                        // untouched; we only shift the per-chunk tint that the
                        // shader consumes. Off unless UNENDLICH_SEASONS=1.
                        if crate::world::season::seasons_enabled() {
                            let tick = req.current_tick.unwrap_or(0);
                            let (dt, dh) = crate::world::season::render_climate_delta(tick);
                            (
                                (base_t + dt).clamp(0.0, 1.0) as f32,
                                (base_h + dh).clamp(0.0, 1.0) as f32,
                            )
                        } else {
                            (base_t as f32, base_h as f32)
                        }
                    }
                    None => (0.5, 0.5),
                }
            };

            let octree_req = OctreeGpuMeshRequest {
                coord: req.coord,
                octree: Arc::clone(octree),
                version: req.version_at_submit,
                epoch: req.epoch, // r195
                lod: req.lod,
                temperature,
                humidity,
                // v0.24.NEXT+2-gpu-decorate: carry the chunk + tick so the
                // dispatcher can apply CPU decoration post-passes. Cheap
                // Arc clone; only read when UNENDLICH_GPU_DECORATE is set.
                chunk: Arc::clone(&req.chunk),
                current_tick: req.current_tick,
                world_model: req.world_model.clone(),
                sky: req.sky, // r501: Copy — sun-light the decoration passes
            };
            if octree_tx.send(octree_req).is_err() {
                break;  // dispatcher gone — pool shutting down
            }
            continue;
        }

        // v0.24.21-gpu-mesher-prod: for Near LOD chunks, if a GPU
        // mesh path is wired in, do the cheap `extract_surface`
        // pass on this worker thread and hand the records to the
        // dispatcher. The greedy mesh + lighting + tinting pass —
        // historically the worker's main cost — moves to the GPU.
        //
        // We pre-screen `MAX_RECORDS_PER_CHUNK`. If a chunk has too
        // many surface cells for the GPU kernel's per-chunk record
        // capacity (~16K), we fall back to the CPU mesher here. In
        // practice this never triggers for 32³ chunks — surface
        // cells are bounded by ~6K per chunk even in dense terrain.
        // The check is a safety net.
        //
        // Mid/Far/VeryFar tiers always stay on CPU: they use
        // factored downsample meshers (sub-millisecond) where GPU
        // dispatch overhead would be a regression. The GPU mesher
        // is a Near-tier optimization.
        if let (LodLevel::Near, Some(ref gpu_tx)) = (req.lod, gpu_request_tx.as_ref()) {
            let records = crate::world::surface::extract_surface(&req.chunk);
            if records.len() <= MAX_RECORDS_PER_CHUNK as usize {
                // Account for the extract_surface CPU pass under the
                // Near bucket. The downstream GPU work logs separately
                // from the dispatcher thread.
                let elapsed_us = t0.elapsed().as_micros() as u64;
                mesher_timer.record(LodLevel::Near, elapsed_us);
                let gpu_req = GpuMeshRequest {
                    coord: req.coord,
                    records,
                    version: req.version_at_submit,
                    epoch: req.epoch, // r195
                    lod: req.lod,
                };
                if gpu_tx.send(gpu_req).is_err() {
                    // Dispatcher gone — pool is shutting down.
                    break;
                }
                continue;
            }
            // Fall through to CPU mesher: record count exceeds
            // GPU kernel capacity. Note: we already paid for the
            // extract_surface pass; we'd ideally not double-work,
            // but the CPU mesher does its own surface scan
            // internally as part of greedy meshing. The overhead is
            // negligible on the cold-failover path.
            log::warn!("GPU mesher fallback for {:?}: {} records exceeds {} limit",
                req.coord, records.len(), MAX_RECORDS_PER_CHUNK);
        }

        // F3 dispatch (CPU path): Near uses the existing greedy-mesher path with
        // full sky/climate/tick treatment. Non-Near uses the simpler
        // `mesh_chunk_downsampled` which emits factored-cell faces and
        // doesn't run the (expensive) greedy merge or climate/tint
        // path. This is intentional: distant chunks don't show climate
        // tinting at recognisable resolution anyway, and the whole
        // point of LOD is "cheap to mesh."
        //
        // v0.23.7 Stage 2b: if a generator is attached AND the tier is
        // Far/VeryFar, take the reverse-regen path that bypasses the
        // chunk's block grid entirely. Same output shape, no block-grid
        // dependency. Near/Mid always go through the materialised path.
        let is_virtual_eligible = matches!(req.lod, LodLevel::Far | LodLevel::VeryFar)
            && req.generator.is_some();
        // v0.30: reaching this CPU greedy path for a Near chunk while the octree
        // GPU mesher is wired means the octree was absent at submit and we're
        // paying the full greedy mesh on the worker thread (the `near` spike). Count it.
        if req.lod == LodLevel::Near && octree_gpu_request_tx.is_some() {
            NEAR_CPU_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        let mesh = if req.lod == LodLevel::Near {
            match (&req.world_model, req.current_tick) {
                (Some(model), Some(tick)) => {
                    mesh_chunk_with_sky_model_and_tick(&req.chunk, &req.sky, model, tick)
                }
                (Some(model), None) => {
                    mesh_chunk_with_sky_and_model(&req.chunk, &req.sky, model)
                }
                (None, _) => mesh_chunk_with_sky(&req.chunk, &req.sky),
            }
        } else if is_virtual_eligible {
            let factor = lod_settings.factor_for(req.lod);
            let gen = req.generator.as_ref()
                .expect("is_virtual_eligible implies generator is Some");
            mesh_chunk_far_virtual(req.coord, gen, factor as i32)
        } else {
            let factor = lod_settings.factor_for(req.lod);
            mesh_chunk_downsampled(&req.chunk, factor as i32)
        };
        let elapsed_us = t0.elapsed().as_micros() as u64;
        mesher_timer.record(req.lod, elapsed_us);

        // r596 (upload Tier 2a/5): quantize to PackedVertex on THIS worker
        // thread; the frame thread's upload becomes a memcpy of these bytes.
        let mut mesh = mesh;
        {
            let (ox, oy, oz) = req.coord.block_origin();
            mesh.pack_in_place([ox as f32, oy as f32, oz as f32]);
        }

        let result = MeshResult {
            coord: req.coord,
            mesh,
            epoch: req.epoch, // r195
            version: req.version_at_submit,
            lod: req.lod,
        };
        if res_tx.send(result).is_err() {
            break;
        }
    }
}

/// v0.24.21-gpu-mesher-prod: drain `gpu_request_rx`, coalesce up to
/// `effective_batch_size()` requests (or wait up to `GPU_BATCH_TIMEOUT`),
/// fire one GPU compute submission, and forward results to `res_tx`.
///
/// Why a dedicated thread? `SparseGpuMesher` is non-trivially stateful —
/// it owns persistent-mapped buffers and a single command buffer — and
/// its `mesh_chunks_batch` already serializes via an internal mutex.
/// Routing all GPU mesh work through a single thread is the simplest
/// way to actually realize the kernel's intended amortization: ONE
/// submit per batch instead of one submit per chunk.
fn gpu_dispatcher_loop(
    rx: Receiver<GpuMeshRequest>,
    res_tx: Sender<MeshResult>,
    mesher: Arc<crate::render::gpu_meshing_sparse::SparseGpuMesher>,
    mesher_timer: Arc<crate::render::mesher_timer::MesherTimer>,
) {
    use crate::render::gpu_meshing_sparse::{ChunkBatchInput, effective_batch_size};
    use crate::world::lod::LodLevel;

    let batch_target = effective_batch_size() as usize;
    log::info!("gpu-mesh-dispatcher: batch target = {}, timeout = {:?}",
        batch_target, GPU_BATCH_TIMEOUT);

    loop {
        // Block-recv for the first request of a batch. Returns Err
        // when the channel is closed — that's our shutdown signal.
        let first = match rx.recv() {
            Ok(r) => r,
            Err(_) => {
                log::info!("gpu-mesh-dispatcher: channel closed, exiting");
                return;
            }
        };
        let batch_start = Instant::now();
        let mut batch: Vec<GpuMeshRequest> = Vec::with_capacity(batch_target);
        batch.push(first);

        // Fill the batch up to `batch_target` or until `GPU_BATCH_TIMEOUT`
        // elapses since we received the first request. We use
        // `recv_timeout` to keep CPU idle between requests rather
        // than spinning. When the channel runs dry (workers haven't
        // produced anything new), recv_timeout returns Err and we
        // dispatch whatever we have.
        while batch.len() < batch_target {
            let elapsed = batch_start.elapsed();
            if elapsed >= GPU_BATCH_TIMEOUT { break; }
            let remaining = GPU_BATCH_TIMEOUT - elapsed;
            match rx.recv_timeout(remaining) {
                Ok(r) => batch.push(r),
                Err(_) => break,
            }
        }

        // Build the batch input view. `inputs` borrows the `records`
        // Vecs from `batch`; we keep `batch` alive through the
        // dispatch call so the borrow is valid.
        let inputs: Vec<ChunkBatchInput> = batch.iter().map(|r| ChunkBatchInput {
            coord: r.coord,
            records: r.records.as_slice(),
        }).collect();

        let t0 = Instant::now();
        let outputs = match mesher.mesh_chunks_batch(&inputs) {
            Ok(o) => o,
            Err(e) => {
                log::error!("gpu-mesh-dispatcher: batch dispatch failed: {} (batch_size={})",
                    e, batch.len());
                // Send back empty meshes for the whole batch so the
                // render thread doesn't wait forever for these chunks.
                // The renderer's stale-mesh re-request logic will pick
                // them up again later.
                for r in &batch {
                    let _ = res_tx.send(MeshResult {
                        coord: r.coord,
                        mesh: ChunkMesh::empty(),
                        epoch: r.epoch, // r195
                        version: r.version,
                        lod: r.lod,
                    });
                }
                continue;
            }
        };
        let dispatch_us = t0.elapsed().as_micros() as u64;

        // Record per-chunk time in the Near bucket. We attribute the
        // batch's total wall-clock cost evenly across its members;
        // that's an approximation (the GPU's per-chunk cost varies
        // by record count) but good enough for the FPS-log "near"
        // line to track GPU-mesh trends.
        let per_chunk_us = if batch.is_empty() { 0 } else { dispatch_us / batch.len() as u64 };
        // r463 (T1 triage): catch the catastrophic mesh hitches seen in log-r462 (near max=889ms).
        if dispatch_us > 100_000 {
            log::warn!("[mesh-stall] octree dispatch {}ms for {} chunks ({}us/chunk) — investigate this chunk batch", dispatch_us / 1000, batch.len(), per_chunk_us);
        }
        for _ in &batch {
            mesher_timer.record(LodLevel::Near, per_chunk_us);
        }

        // Forward results. `outputs` is the same length as `inputs`
        // (and therefore `batch`). Per-chunk failures (output overflow)
        // become empty meshes — a fresh request will retry.
        for (r, o) in batch.iter().zip(outputs.into_iter()) {
            let mut mesh = match o.mesh {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("gpu-mesh-dispatcher: chunk {:?} failed: {}", r.coord, e);
                    ChunkMesh::empty()
                }
            };
            // r596 (upload Tier 2a/5): quantize to PackedVertex HERE, on the
            // dispatcher thread, so the frame thread's upload is a memcpy.
            {
                let (ox, oy, oz) = r.coord.block_origin();
                mesh.pack_in_place([ox as f32, oy as f32, oz as f32]);
            }
            if res_tx.send(MeshResult {
                coord: r.coord, mesh,
                epoch: r.epoch, version: r.version, lod: r.lod, // r195
            }).is_err() {
                // result channel closed — pool dropping.
                log::info!("gpu-mesh-dispatcher: result channel closed, exiting");
                return;
            }
        }
    }
}

/// v0.24.27-octree-mesher: octree-driven equivalent of
/// `gpu_dispatcher_loop`. Reads `OctreeGpuMeshRequest` batches and
/// dispatches to `OctreeGpuMesher::mesh_chunks_batch`. Same
/// batching policy as the sparse dispatcher (target = effective
/// batch size, timeout = GPU_BATCH_TIMEOUT).
///
/// Why a separate loop instead of generic-over-mesher-kind: the
/// two mesher types have different `mesh_chunks_batch` input
/// shapes (`ChunkBatchInput` borrows `&[SurfaceRecord]`,
/// `OctreeBatchInput` owns `Arc<OctreeArray>`). Trying to share
/// code via traits introduces lifetime gymnastics that the small
/// duplication here avoids.
/// v0.24.28-bumpy-foundation: throttle on the batch diagnostic so
/// we don't spam the log at 30+ batches/second steady-state. One
/// log line per 10 seconds is plenty for observing rate trends.
struct DiagThrottle {
    last_log: Instant,
    accum_batches: u32,
    accum_chunks: u32,
    accum_empty: u32,
    accum_nonempty: u32,
    accum_total_verts: u64,
    accum_max_verts: u32,
    accum_invariant_violations: u32,
    last_violation: Option<String>,
}
impl DiagThrottle {
    fn new() -> Self {
        Self {
            last_log: Instant::now(),
            accum_batches: 0, accum_chunks: 0,
            accum_empty: 0, accum_nonempty: 0,
            accum_total_verts: 0, accum_max_verts: 0,
            accum_invariant_violations: 0, last_violation: None,
        }
    }
}

fn octree_gpu_dispatcher_loop(
    rx: Receiver<OctreeGpuMeshRequest>,
    res_tx: Sender<MeshResult>,
    mesher: Arc<crate::render::gpu_meshing_octree::OctreeGpuMesher>,
    mesher_timer: Arc<crate::render::mesher_timer::MesherTimer>,
) {
    use crate::render::gpu_meshing_octree::{OctreeBatchInput, effective_batch_size};
    use crate::world::lod::LodLevel;

    let batch_cap = effective_batch_size() as usize;
    // r476: ADAPTIVE batch sizing. The octree mesh dispatch is a synchronous GPU compute on the
    // SHARED graphics queue (no distinct compute queue on Asahi: "Compute queue ... graphics handle
    // reused"), so a slow batch serializes ahead of the render pass and stalls the frame. On the
    // M2/Asahi the mesher runs ~47ms/CHUNK, so a 4-chunk batch is one ~190ms GPU block that the
    // render present waits out (observed: mesh_q backed up -> FPS 1-10 while moving). We start at
    // batch_cap and SHRINK toward 1 when a dispatch overruns a frame budget — smaller dispatches let
    // rendering interleave between them — and GROW back when dispatches are cheap. Auto-tunes to the
    // GPU: fast GPUs keep big throughput batches; slow ones pin near 1 to minimize per-stall length.
    // UNENDLICH_GPU_MESH_ADAPTIVE=0 pins it at batch_cap (the old fixed behavior).
    let adaptive = std::env::var("UNENDLICH_GPU_MESH_ADAPTIVE")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    let mut cur_batch_target = batch_cap;
    // r476: all of these are now LEVERS (were hardcoded constants). On Asahi the
    // octree mesher can run 38-171ms/CHUNK, so the operator needs to tune the
    // adaptive thresholds and the GPU duty cycle per device without a rebuild.
    let env_u64 = |k: &str, d: u64| std::env::var(k).ok()
        .and_then(|s| s.parse::<u64>().ok()).unwrap_or(d);
    // Dispatch over SHRINK_US -> shrink the batch (let render interleave);
    // under GROW_US -> grow it back (throughput when dispatches are cheap).
    let adapt_shrink_us = env_u64("UNENDLICH_GPU_MESH_SHRINK_US", 33_000);
    let adapt_grow_us   = env_u64("UNENDLICH_GPU_MESH_GROW_US",   12_000);
    // Batch accumulation window (ms). Smaller = lower latency / smaller batches.
    let batch_timeout = Duration::from_millis(
        env_u64("UNENDLICH_GPU_MESH_TIMEOUT_MS", 8));
    // DYNAMIC THROTTLE: after each dispatch, sleep dispatch_time * YIELD_PCT/100
    // so the shared GPU queue is left idle for the render pass. Read LIVE from a
    // global (settable from the Performance page); seed it from the env var once.
    let yield_seed = env_u64("UNENDLICH_GPU_MESH_YIELD_PCT", 0);
    crate::constants::set_gpu_mesh_yield_pct(yield_seed);
    log::info!("octree-gpu-mesh-dispatcher: batch cap = {}, timeout = {:?}, adaptive = {}, \
                shrink_us = {}, grow_us = {}, yield_pct = {}",
        batch_cap, batch_timeout, adaptive, adapt_shrink_us, adapt_grow_us, yield_seed);

    let verify_output = std::env::var("UNENDLICH_OCTREE_MESH_VERIFY")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    if verify_output {
        log::info!("octree-gpu-mesh-dispatcher: UNENDLICH_OCTREE_MESH_VERIFY=1 — \
                    runtime mesh-output invariant checking enabled");
    }

    // v0.24.NEXT+2-gpu-decorate: when set, run the CPU decoration /
    // fluid-surface post-passes (grass tufts, water, lava) on each
    // GPU-meshed chunk before sending the result. The octree GPU mesher
    // emits only solid block faces; these passes restore parity with the
    // CPU mesher for the features it owns. Off by default so the GPU
    // pipeline's behaviour is unchanged until this is validated on
    // hardware (the geometry is purely additive — worst case is
    // misplaced decoration, never corrupted base faces).
    let gpu_decorate = std::env::var("UNENDLICH_GPU_DECORATE")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    if gpu_decorate {
        log::info!("octree-gpu-mesh-dispatcher: UNENDLICH_GPU_DECORATE=1 — \
                    CPU decoration/fluid post-passes enabled for GPU-meshed chunks");
    }

    // v0.24.NEXT+9-overflow-fix: on GPU mesher overflow, re-mesh the chunk
    // on the CPU greedy mesher (no fixed buffer cap) instead of emitting an
    // empty mesh that leaves a hole. Default ON — a filled chunk is
    // strictly better than a missing one. UNENDLICH_GPU_OVERFLOW_FALLBACK=0
    // restores the old empty-on-overflow behaviour (for A/B / debugging).
    let gpu_overflow_fallback = std::env::var("UNENDLICH_GPU_OVERFLOW_FALLBACK")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    if !gpu_overflow_fallback {
        log::info!("octree-gpu-mesh-dispatcher: UNENDLICH_GPU_OVERFLOW_FALLBACK=0 — \
                    overflowing chunks will emit empty meshes (holes)");
    }

    // v0.24.NEXT+10-neutral-tint: feed the GPU mesher neutral climate
    // values (0.5, 0.5) instead of the per-chunk sampled temperature/
    // humidity, collapsing the shader's climate tint to a uniform
    // (≈base) color. Diagnostic + mitigation for the regional magenta
    // foliage: if the magenta is the shader's climate-tint math (which
    // is regional, i.e. climate-driven), this removes it. Default off
    // (byte-identical). UNENDLICH_NEUTRAL_TINT=1 to enable.
    let neutral_tint = std::env::var("UNENDLICH_NEUTRAL_TINT")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    if neutral_tint {
        log::info!("octree-gpu-mesh-dispatcher: UNENDLICH_NEUTRAL_TINT=1 — \
                    feeding neutral climate (0.5,0.5) to the mesher (tint disabled)");
    }

    let mut diag = DiagThrottle::new();
    const DIAG_INTERVAL: Duration = Duration::from_secs(10);

    // v0.24.39-mid-lod: dispatcher now batches by LOD. When a
    // request arrives with a different LOD than the current batch,
    // we stash it in `pending` and dispatch the current batch first;
    // the stashed request becomes the seed of the next batch.
    //
    // This preserves the single-LOD invariant the shader needs
    // (push constants apply to the whole dispatch).
    let mut pending: Option<OctreeGpuMeshRequest> = None;

    loop {
        let first = match pending.take() {
            Some(r) => r,
            None => match rx.recv() {
                Ok(r) => r,
                Err(_) => {
                    log::info!("octree-gpu-mesh-dispatcher: channel closed, exiting");
                    return;
                }
            },
        };
        let batch_lod = first.lod;
        let batch_lod_stride: u32 = match batch_lod {
            LodLevel::Near    => 1,
            LodLevel::Mid     => 2,
            LodLevel::Far     => 4,
            LodLevel::VeryFar => 8,
        };
        let batch_start = Instant::now();
        let mut batch: Vec<OctreeGpuMeshRequest> = Vec::with_capacity(cur_batch_target);
        batch.push(first);

        while batch.len() < cur_batch_target {
            let elapsed = batch_start.elapsed();
            if elapsed >= batch_timeout { break; }
            let remaining = batch_timeout - elapsed;
            match rx.recv_timeout(remaining) {
                Ok(r) if r.lod == batch_lod => batch.push(r),
                Ok(r) => {
                    // Different LOD — stash and flush current batch.
                    pending = Some(r);
                    break;
                }
                Err(_) => break,
            }
        }

        // Build inputs by cloning the Arc<OctreeArray> handles —
        // cheap (atomic refcount), and the OctreeGpuMesher's batch
        // API takes ownership of each input's Arc clone.
        // v0.24.31-version-tracked: include the chunk's content
        // version so the pool can short-circuit redundant uploads.
        // v0.24.33-climate-tint: include per-chunk temperature and
        // humidity for shader-side color modulation.
        let inputs: Vec<OctreeBatchInput> = batch.iter().map(|r| OctreeBatchInput {
            coord: r.coord,
            octree: Arc::clone(&r.octree),
            version: r.version,
            temperature: r.temperature,
            humidity: r.humidity,
        }).collect();

        let t0 = Instant::now();
        let outputs = match mesher.mesh_chunks_batch(&inputs, batch_lod_stride) {
            Ok(o) => o,
            Err(e) => {
                log::error!("octree-gpu-mesh-dispatcher: batch dispatch failed: {} (batch_size={})",
                    e, batch.len());
                for r in &batch {
                    let _ = res_tx.send(MeshResult {
                        coord: r.coord,
                        mesh: ChunkMesh::empty(),
                        epoch: r.epoch, // r195
                        version: r.version,
                        lod: r.lod,
                    });
                }
                continue;
            }
        };
        let dispatch_us = t0.elapsed().as_micros() as u64;

        let per_chunk_us = if batch.is_empty() { 0 } else { dispatch_us / batch.len() as u64 };
        // r463 (T1 triage): catch the catastrophic mesh hitches seen in log-r462 (near max=889ms).
        if dispatch_us > 100_000 {
            log::warn!("[mesh-stall] octree dispatch {}ms for {} chunks ({}us/chunk) — investigate this chunk batch", dispatch_us / 1000, batch.len(), per_chunk_us);
        }
        // r476: adapt the batch target so one dispatch can't stall the render frame for long. The
        // dispatch is synchronous on the shared graphics queue, so its wall time blocks the next
        // render present. Shrink when a dispatch overruns the frame budget (rendering interleaves
        // between shorter dispatches); grow when dispatches are cheap. Heavy streaming batches
        // (>33ms) pin this toward 1; light mostly-air batches (<12ms) grow it back toward batch_cap.
        if adaptive {
            let prev = cur_batch_target;
            if dispatch_us > adapt_shrink_us && cur_batch_target > 1 {
                cur_batch_target -= 1;
            } else if dispatch_us < adapt_grow_us && cur_batch_target < batch_cap {
                cur_batch_target += 1;
            }
            if cur_batch_target != prev {
                log::debug!("octree-gpu-mesh-dispatcher: adaptive batch {} -> {} (last dispatch {}ms)",
                    prev, cur_batch_target, dispatch_us / 1000);
            }
        }
        // r476: dynamic GPU-duty throttle. The dispatch monopolized the shared
        // graphics queue for `dispatch_us`; sleeping a fraction of that here
        // leaves the queue idle so the render pass can land a frame, trading
        // chunk pop-in speed for playable FPS while streaming. yield_pct=100
        // => sleep == dispatch time (~50% mesh duty). Default 0 = no throttle.
        let yield_pct = crate::constants::gpu_mesh_yield_pct();
        if yield_pct > 0 {
            let nap_us = dispatch_us.saturating_mul(yield_pct) / 100;
            if nap_us > 0 {
                thread::sleep(Duration::from_micros(nap_us));
            }
        }
        for _ in &batch {
            mesher_timer.record(LodLevel::Near, per_chunk_us);
        }
        // every DIAG_INTERVAL. Output-sanity check (if enabled)
        // still runs every batch — only the LOG is throttled.
        diag.accum_batches += 1;
        diag.accum_chunks += batch.len() as u32;

        for (r, o) in batch.iter().zip(outputs.iter()) {
            if let Ok(m) = &o.mesh {
                let vc = m.vertices.len() as u32;
                if vc == 0 {
                    diag.accum_empty += 1;
                } else {
                    diag.accum_nonempty += 1;
                    diag.accum_total_verts += vc as u64;
                    if vc > diag.accum_max_verts { diag.accum_max_verts = vc; }
                }
                if verify_output {
                    if let Err(msg) =
                        crate::render::gpu_meshing_octree_cpu_ref::check_mesh_invariants(m, r.coord)
                    {
                        diag.accum_invariant_violations += 1;
                        diag.last_violation = Some(format!("coord={:?}: {}", r.coord, msg));
                    }
                }
            }
        }

        if diag.last_log.elapsed() >= DIAG_INTERVAL {
            let avg = if diag.accum_nonempty > 0 {
                diag.accum_total_verts / diag.accum_nonempty as u64
            } else { 0 };
            log::info!(
                "octree-mesher: {} batches / {} chunks in last {}s: empty={} nonempty={} \
                 avg_verts={} max_verts={}{}",
                diag.accum_batches, diag.accum_chunks,
                DIAG_INTERVAL.as_secs(),
                diag.accum_empty, diag.accum_nonempty,
                avg, diag.accum_max_verts,
                if verify_output {
                    format!(" invariant_violations={}", diag.accum_invariant_violations)
                } else { String::new() },
            );
            if verify_output && diag.accum_invariant_violations > 0 {
                log::warn!(
                    "octree-mesher: {} mesh-invariant violations in last {}s. \
                     Latest: {}",
                    diag.accum_invariant_violations,
                    DIAG_INTERVAL.as_secs(),
                    diag.last_violation.as_deref().unwrap_or("(none)"),
                );
            }
            diag = DiagThrottle::new();
        }

        for (r, o) in batch.iter().zip(outputs.into_iter()) {
            let (mut mesh, run_gpu_decorate) = match o.mesh {
                Ok(m) => (m, true),
                Err(e) => {
                    // v0.24.NEXT+9-overflow-fix: the GPU octree mesher
                    // overflowed its fixed per-chunk buffers (dense foliage
                    // chunks — leaves don't merge, so vertex/index counts
                    // explode). Emitting an empty mesh leaves a visible
                    // HOLE. Instead, re-mesh this one chunk on the CPU
                    // greedy mesher, which has no fixed cap. The chunk is
                    // already threaded in (for decoration), so this is a
                    // direct call. `mesh_chunk_greedy` produces a complete
                    // mesh (solid faces + grass/water/lava decoration baked
                    // in), so we DON'T run the extra decoration pass below
                    // for the fallback.
                    //
                    // v0.24.NEXT-stripe-fix: feed the request's world_model so
                    // the fallback bakes the SAME climate tint as a normal CPU
                    // mesh (mesh_chunk_greedy samples temperature/humidity per
                    // face from it). The GPU request now carries world_model
                    // (added alongside the per-chunk temperature/humidity it was
                    // already computing). Previously this passed None,None ->
                    // an untinted band next to tinted GPU neighbours = the
                    // stripe (docs/MEMORY-PRESSURE.md). Sky stays None for now
                    // (neutral lighting; the GPU request has no Sky and the CPU
                    // tint is the dominant visual diff) — a follow-up if needed.
                    // Default on; UNENDLICH_GPU_OVERFLOW_FALLBACK=0 restores the
                    // old empty-on-overflow behaviour for A/B.
                    if gpu_overflow_fallback {
                        GPU_OOM_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            "octree-gpu-mesh-dispatcher: chunk {:?} overflowed GPU mesher \
                             ({}); CPU-fallback meshing to avoid a hole", r.coord, e);
                        let m = crate::render::mesher::mesh_chunk_greedy(
                            &r.chunk, None, r.world_model.as_ref(), r.current_tick);
                        (m, false)
                    } else {
                        log::warn!("octree-gpu-mesh-dispatcher: chunk {:?} failed: {}", r.coord, e);
                        (ChunkMesh::empty(), false)
                    }
                }
            };

            // v0.24.NEXT+2-gpu-decorate: append the CPU-only post-passes
            // the GPU mesher doesn't emit. Gated on `run_gpu_decorate`
            // (true only for successful GPU meshes — the CPU fallback
            // above already includes decoration, so we must not double it).
            // An all-water "ocean surface" chunk has an empty solid mesh
            // but should still get water tops, so we run the passes on
            // empty-but-valid GPU meshes too. decorate_chunk iterates only
            // the chunk's plant index, so non-plant chunks add ~zero cost.
            // Sky is passed as None (neutral lighting on decoration);
            // threading the per-chunk Sky is a follow-up once `Sky`'s
            // Clone-ability is confirmed (sky.rs not in scope here).
            // Coordinates match: both the GPU mesher and these passes emit
            // world-space vertices offset by block_origin().
            // r496/r505: gate the expensive sub-voxel decoration (dense stringy leaves,
            // grass tufts) to the NEAREST tier only. r496 made leaves ~3-4× denser, which
            // inflates chunk mesh size; bigger meshes feed the M2 upload-pressure stall
            // (HOST_COHERENT memcpy → GPU cache-invalidation, the documented chug). r497
            // gated Far/VeryFar off; r505 also drops Mid (its fine foliage is small at that
            // distance) so near forests keep the full dense look while mid-distance chunks
            // stay light on the upload path. Water + lava surfaces still emit at all tiers.
            // Revert to `<= Mid` for more foliage distance at the cost of bigger mid uploads.
            let decorate_lod_ok = r.lod == crate::world::lod::LodLevel::Near;
            if gpu_decorate && run_gpu_decorate {
                let (ox, oy, oz) = r.coord.block_origin();
                let origin = [ox, oy, oz];
                if decorate_lod_ok {
                    crate::render::mesher::decorate_chunk(
                        &r.chunk, &mut mesh, origin, Some(&r.sky), r.current_tick);
                }
                crate::render::mesher::emit_water_surface(
                    &r.chunk, &mut mesh, origin, Some(&r.sky));
                crate::render::mesher::emit_lava_surface(
                    &r.chunk, &mut mesh, origin, Some(&r.sky));
            }

            // r596 (upload Tier 2a/5): quantize to PackedVertex on the
            // dispatcher thread AFTER decoration (so tufted foliage + its sway
            // tags are included); the frame thread's upload becomes a memcpy.
            {
                let (pox, poy, poz) = r.coord.block_origin();
                mesh.pack_in_place([pox as f32, poy as f32, poz as f32]);
            }

            if res_tx.send(MeshResult {
                coord: r.coord, mesh,
                epoch: r.epoch, version: r.version, lod: r.lod, // r195
            }).is_err() {
                log::info!("octree-gpu-mesh-dispatcher: result channel closed, exiting");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::Chunk;
    use crate::world::lod::{LodLevel, LodSettings};
    use std::time::Duration;

    /// Helper: default LodSettings wrapped in Arc, for tests.
    fn default_settings() -> Arc<LodSettings> {
        Arc::new(LodSettings::default())
    }

    #[test]
    fn pool_processes_a_request() {
        let pool = MeshingPool::new(1, 4, default_settings());
        let chunk = Arc::new(Chunk::empty(ChunkCoord::new(0, 0, 0)));
        let accepted = pool.submit(MeshRequest {
            coord: ChunkCoord::new(0, 0, 0),
            chunk,
            epoch: 0,
            version_at_submit: 0,
            sky: crate::sky::Sky::default(),
            world_model: None,
            current_tick: None,
            lod: LodLevel::Near,
            generator: None,
            octree: None,
        });
        assert!(accepted);

        // Wait briefly for the worker.
        let mut got = Vec::new();
        for _ in 0..50 {
            got = pool.drain_completed();
            if !got.is_empty() { break; }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1, "expected one mesh result");
        assert!(got[0].mesh.is_empty(), "empty chunk → empty mesh");
        assert_eq!(got[0].lod, LodLevel::Near);
    }

    #[test]
    fn pool_processes_multiple_requests() {
        let pool = MeshingPool::new(2, 16, default_settings());
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(0, 0, 0, crate::world::blocks::BlockId::Stone);
        let chunk = Arc::new(chunk);

        for i in 0..8 {
            let coord = ChunkCoord::new(i, 0, 0);
            assert!(pool.submit(MeshRequest {
                coord,
                chunk: Arc::clone(&chunk),
                epoch: 0,
            version_at_submit: 0,
                sky: crate::sky::Sky::default(),
                world_model: None,
                current_tick: None,
                lod: LodLevel::Near,
                generator: None,
                octree: None,
            }));
        }

        let mut total = 0;
        for _ in 0..200 {
            total += pool.drain_completed().len();
            if total >= 8 { break; }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(total, 8);
    }

    #[test]
    fn pool_rejects_when_full() {
        // 1 worker, capacity 2: third submit while the first hasn't been
        // pulled should fail.
        let pool = MeshingPool::new(1, 2, default_settings());
        let chunk = Arc::new(Chunk::empty(ChunkCoord::new(0, 0, 0)));
        // Stuff the queue. Workers may or may not pull immediately, so we
        // just check that we don't accept indefinitely.
        let mut accepted = 0;
        for i in 0..1000 {
            if pool.submit(MeshRequest {
                coord: ChunkCoord::new(i, 0, 0),
                chunk: Arc::clone(&chunk),
                epoch: 0,
            version_at_submit: 0,
                sky: crate::sky::Sky::default(),
                world_model: None,
                current_tick: None,
                lod: LodLevel::Near,
                generator: None,
                octree: None,
            }) {
                accepted += 1;
            } else {
                break;
            }
        }
        assert!(accepted < 1000, "pool should refuse at some point");
    }

    /// F3: a request with non-Near LOD takes the downsampled path,
    /// while Near takes the existing greedy path. Verifies dispatch
    /// by comparing each worker output to a direct call of the
    /// expected mesher on the same chunk.
    ///
    /// This used to compare Far vs Near vertex counts on a fully-
    /// solid chunk and assert Far < Near. That assumption was wrong:
    /// the greedy mesher merges a fully-solid chunk into 6 quads
    /// (24 verts), while the factored-naive far mesher produces
    /// 6 × 8² = 384 quads (1536 verts). Greedy beats factored-naive
    /// on the degenerate fully-solid input — the test had the
    /// inequality backwards.
    ///
    /// The fix: use a generator-produced terrain chunk where greedy
    /// cannot trivially collapse, AND assert dispatch went to the
    /// right mesher by comparing to a direct call. That's the
    /// stronger correctness claim.
    #[test]
    fn pool_dispatches_far_lod_to_downsampled_mesher() {
        use crate::render::mesher::{
            mesh_chunk_downsampled, mesh_chunk_with_sky,
        };
        use crate::world::cpu_generator::WorldGenerator;

        let pool = MeshingPool::new(1, 8, default_settings());

        // Generator terrain chunk — meaningful greedy merges plus
        // detail to lose at LOD::Far.
        let gen = WorldGenerator::new(0x9876);
        let raw_chunk = gen.generate_chunk(ChunkCoord::new(0, 1, 0));

        // Pre-compute the expected outputs by calling the meshers
        // directly. Same sky default the worker sees for Near.
        let sky = crate::sky::Sky::default();
        let expected_near_verts = mesh_chunk_with_sky(&raw_chunk, &sky).vertex_count();
        let expected_far_verts  = mesh_chunk_downsampled(&raw_chunk, 4).vertex_count();

        let chunk = Arc::new(raw_chunk);
        let mk_req = |lod: LodLevel, coord: ChunkCoord| MeshRequest {
            coord,
            chunk: Arc::clone(&chunk),
            epoch: 0,
            version_at_submit: 0,
            sky,
            world_model: None,
            current_tick: None,
            lod,
            generator: None,
            octree: None,
        };

        assert!(pool.submit(mk_req(LodLevel::Near, ChunkCoord::new(0, 0, 0))));
        assert!(pool.submit(mk_req(LodLevel::Far,  ChunkCoord::new(1, 0, 0))));

        let mut results = Vec::new();
        for _ in 0..100 {
            results.extend(pool.drain_completed());
            if results.len() >= 2 { break; }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(results.len(), 2, "expected both mesh results");

        let near = results.iter().find(|r| r.lod == LodLevel::Near).unwrap();
        let far  = results.iter().find(|r| r.lod == LodLevel::Far).unwrap();

        // Dispatch correctness: each worker output must equal the
        // direct call to the expected mesher.
        assert_eq!(near.mesh.vertex_count(), expected_near_verts,
            "Near worker should match mesh_chunk_with_sky output");
        assert_eq!(far.mesh.vertex_count(),  expected_far_verts,
            "Far worker should match mesh_chunk_downsampled(., 4) output");

        // Sanity: on terrain (not solid), the far mesher really does
        // produce fewer vertices than naive at full detail. The
        // greedy mesher's merging means it MIGHT match or beat far
        // on simple chunks, so we don't pin Near vs Far ordering
        // here — that test belongs in F2 where it lives.
        assert!(far.mesh.vertex_count()  > 0);
        assert!(near.mesh.vertex_count() > 0);
    }

    /// v0.23.7 Stage 2b: when a generator Arc is attached AND the LOD
    /// is Far/VeryFar, the worker takes the reverse-regen path. We
    /// prove this by submitting two requests for the same coord:
    /// one with the generator (virtual path), one without (materialised).
    /// The materialised path uses an empty chunk → empty mesh. The
    /// virtual path samples real terrain → non-empty mesh. The
    /// difference IS the dispatch.
    #[test]
    fn pool_takes_virtual_path_when_generator_attached() {
        use crate::world::cpu_generator::WorldGenerator;
        let pool = MeshingPool::new(2, 8, default_settings());
        let empty_chunk = Arc::new(Chunk::empty(ChunkCoord::new(0, 2, 0)));
        let generator = Arc::new(WorldGenerator::new(0xDEAD_BEEF));

        // Same coord, both as Far. First: no generator → materialised
        // path on the empty chunk → empty mesh.
        assert!(pool.submit(MeshRequest {
            coord: ChunkCoord::new(0, 2, 0),
            chunk: Arc::clone(&empty_chunk),
            epoch: 0,
            version_at_submit: 0,
            sky: crate::sky::Sky::default(),
            world_model: None,
            current_tick: None,
            lod: LodLevel::Far,
            generator: None,
            octree: None,
        }));

        // Second: generator present → virtual path → samples real
        // terrain → almost certainly non-empty for a surface chunk.
        // Submit on a different coord so we can distinguish results.
        assert!(pool.submit(MeshRequest {
            coord: ChunkCoord::new(1, 2, 0),
            chunk: Arc::clone(&empty_chunk),  // ignored by virtual path
            epoch: 0,
            version_at_submit: 0,
            sky: crate::sky::Sky::default(),
            world_model: None,
            current_tick: None,
            lod: LodLevel::Far,
            generator: Some(Arc::clone(&generator)),
            octree: None,
        }));

        let mut results = Vec::new();
        for _ in 0..200 {
            results.extend(pool.drain_completed());
            if results.len() >= 2 { break; }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(results.len(), 2, "expected both results");

        let materialised = results.iter()
            .find(|r| r.coord == ChunkCoord::new(0, 2, 0))
            .expect("materialised result");
        let virtual_r = results.iter()
            .find(|r| r.coord == ChunkCoord::new(1, 2, 0))
            .expect("virtual result");

        // Materialised on empty chunk → empty mesh.
        assert!(materialised.mesh.is_empty(),
            "materialised path on empty chunk should produce empty mesh");

        // Virtual on surface coord → non-empty mesh (samples real terrain).
        // This is the dispatch signal: the worker had a chunk that, if
        // meshed materially, would be empty. But it took the virtual path
        // and sampled the generator → got real terrain.
        assert!(!virtual_r.mesh.is_empty(),
            "virtual path on surface coord should produce non-empty mesh");
    }

    /// v0.23.7 Stage 2b: generator on a Near request is ignored.
    /// Near always goes through the materialised path. Verifies we
    /// didn't accidentally enable virtual for Near (which would lose
    /// block-level detail and break editing).
    #[test]
    fn pool_ignores_generator_for_near_lod() {
        use crate::world::cpu_generator::WorldGenerator;
        let pool = MeshingPool::new(1, 4, default_settings());
        let empty_chunk = Arc::new(Chunk::empty(ChunkCoord::new(0, 2, 0)));
        let generator = Arc::new(WorldGenerator::new(0xDEAD_BEEF));

        assert!(pool.submit(MeshRequest {
            coord: ChunkCoord::new(0, 2, 0),
            chunk: empty_chunk,
            epoch: 0,
            version_at_submit: 0,
            sky: crate::sky::Sky::default(),
            world_model: None,
            current_tick: None,
            lod: LodLevel::Near,
            generator: Some(generator),
            octree: None,
        }));

        let mut got = Vec::new();
        for _ in 0..50 {
            got = pool.drain_completed();
            if !got.is_empty() { break; }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1);
        // Empty chunk through materialised Near path → empty mesh.
        // If the worker had wrongly taken virtual path, the result
        // would be non-empty (real terrain sampled).
        assert!(got[0].mesh.is_empty(),
            "Near should use materialised path even with generator attached");
    }

    /// Regression test for the v0.24.2c.4 shutdown-hang: when the
    /// result channel is full and the caller stops draining, dropping
    /// the pool must NOT block indefinitely on `worker.join()`.
    ///
    /// Reproducing the failure mode reliably:
    /// - Single worker (one in-flight request at a time).
    /// - Submit MANY requests (more than result channel can hold).
    /// - Never drain results — the worker fills the result channel
    ///   to capacity, then blocks in `res_tx.send`.
    ///
    /// The submit loop retries on backpressure so we actually get
    /// requests onto the queue (the request channel is bounded too).
    /// Even after we stop submitting, the worker keeps pulling from
    /// the request queue and trying to send results, eventually
    /// blocking when the result channel fills.
    ///
    /// Without the v0.24.2c.5 fix (drop `result_rx` first), the
    /// worker is wedged in `res_tx.send` and never sees the request
    /// channel's closure, so `worker.join()` hangs indefinitely.
    #[test]
    fn drop_does_not_hang_when_result_channel_full() {
        use std::sync::mpsc;
        use std::time::Instant;

        // queue_capacity=4: request channel cap 4, result channel cap 8.
        // Worker is single-threaded so the bottleneck is exactly here.
        let pool = MeshingPool::new(1, 4, default_settings());

        // Submit 32 requests with a tight retry-on-full loop. We want
        // to guarantee that more than 8 requests get processed by the
        // worker, so the result channel goes full and the worker
        // blocks in `res_tx.send`.
        let submit_deadline = Instant::now() + Duration::from_millis(500);
        let mut submitted = 0;
        let mut i = 0;
        while submitted < 32 && Instant::now() < submit_deadline {
            let chunk = Arc::new(Chunk::empty(ChunkCoord::new(i, 0, 0)));
            let accepted = pool.submit(MeshRequest {
                coord: ChunkCoord::new(i, 0, 0),
                chunk,
                epoch: 0,
            version_at_submit: 0,
                sky: crate::sky::Sky::default(),
                world_model: None,
                current_tick: None,
                lod: LodLevel::Near,
                generator: None,
                octree: None,
            });
            if accepted {
                submitted += 1;
                i += 1;
            } else {
                // Request channel full — let the worker drain.
                thread::sleep(Duration::from_millis(1));
            }
        }
        // Bail if we couldn't queue enough — system too slow today.
        // Threshold of 12 is sufficient to guarantee the worker
        // processed more than the result-channel capacity (8) and is
        // now blocked in `res_tx.send`.
        assert!(submitted >= 12,
            "test setup failure: only submitted {} requests", submitted);

        // Wait long enough for the worker to fill the result channel
        // and start blocking. 200ms vs ~1µs per empty-chunk mesh is
        // overkill but cheap.
        thread::sleep(Duration::from_millis(200));

        // Drop the pool on another thread so we can timeout.
        let (done_tx, done_rx) = mpsc::channel();
        let drop_thread = thread::Builder::new()
            .name("drop-tester".into())
            .spawn(move || {
                let t = Instant::now();
                drop(pool);
                let _ = done_tx.send(t.elapsed());
            })
            .expect("spawn drop thread");

        // Pool drop must complete within 2 seconds. With the fix it
        // completes in <10ms (drop result_rx → worker's send errors
        // immediately → loop break → join returns). Without the fix,
        // this hangs indefinitely.
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        match result {
            Ok(duration) => {
                // Sanity: even with the fix, this shouldn't take ages.
                assert!(duration < Duration::from_secs(1),
                    "pool drop took {:?} — fix may have regressed",
                    duration);
            }
            Err(_) => {
                panic!(
                    "MeshingPool::Drop hung — workers couldn't exit. \
                     The fix in v0.24.2c.5 regressed: result_rx must \
                     be dropped before workers are joined."
                );
            }
        }

        // Join the drop thread too — should be instant since the
        // signal already arrived.
        let _ = drop_thread.join();
    }

    /// v0.24.NEXT+2-gpu-decorate: pins the two things the octree GPU
    /// dispatcher's decoration step relies on — (1) `decorate_chunk` is
    /// reachable from this module (the `pub(crate)` exposure works), and
    /// (2) it *appends* tuft geometry to an existing mesh rather than
    /// replacing it. The base GPU mesh is simulated here by an empty
    /// `ChunkMesh`; after decoration of a TallGrass block, it must be
    /// non-empty. If this regresses, GPU-meshed chunks lose their grass
    /// even with `UNENDLICH_GPU_DECORATE=1`.
    #[test]
    fn gpu_decorate_appends_tuft_geometry_to_base_mesh() {
        use crate::world::blocks::BlockId;
        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        chunk.set(8, 8, 8, BlockId::TallGrass);

        let mut mesh = ChunkMesh::empty();
        let before = mesh.vertices.len();
        let (ox, oy, oz) = coord.block_origin();
        crate::render::mesher::decorate_chunk(&chunk, &mut mesh, [ox, oy, oz], None, None);

        assert!(
            mesh.vertices.len() > before,
            "decorate_chunk should append tuft geometry for a TallGrass \
             block; base had {} verts, after decoration {} verts",
            before, mesh.vertices.len()
        );
    }
}
