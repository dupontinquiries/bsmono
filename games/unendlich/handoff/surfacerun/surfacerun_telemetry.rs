// surfacerun_telemetry.rs — SUB-STEP 1 (shadow-telemetry) for the SurfaceRun
// far-render rollout. Runs the REAL upload_repr policy (classify/project_residency,
// copied verbatim with the real constants) over a chunk-set modeled by LOD-band
// shell area, calibrated to the attached run log. Pure; rustc --test-free main.
//
// Calibration (from the run log): octree_per_chunk=164 KB; UNENDLICH_LOD_BANDS=12,24,48
// (Near r<=12, Mid 12-24, Far 24-48, VFar >48); observed worst_col_runs max=14 (< the
// policy's 24 cap, so noise rarely forces octree); srun_per_chunk~5-6 KB.
// Assumptions (stated, conservative): 3% edited, 12% uniform, 2% noisy per band;
// octree_bytes_each held flat at 164 KB for ALL octree chunks (overcounts uniform-near,
// so the routed octree figure is an upper bound — conservative).

#![allow(dead_code)]
const CHUNK_SIZE: i32 = 32;
const COLS: usize = (CHUNK_SIZE as usize) * (CHUNK_SIZE as usize); // 1024

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
enum LodLevel { Near = 0, Mid = 1, Far = 2, VFar = 3 }

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ChunkStats { lod: LodLevel, edited: bool, uniform: bool, worst_col_runs: u8, total_runs: u32 }

#[derive(Copy, Clone)]
struct UploadPolicy { surface_run_from: LodLevel, procedural_from: LodLevel, max_worst_col_runs: u8, max_total_runs: u32 }
impl Default for UploadPolicy {
    fn default() -> Self { UploadPolicy { surface_run_from: LodLevel::Mid, procedural_from: LodLevel::Far, max_worst_col_runs: 24, max_total_runs: (COLS as u32) * 8 } }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum UploadRepr { Octree, SurfaceRun, Procedural }

fn too_noisy(s: &ChunkStats, p: &UploadPolicy) -> bool {
    !s.uniform && (s.worst_col_runs > p.max_worst_col_runs || s.total_runs > p.max_total_runs)
}
fn classify(s: &ChunkStats, p: &UploadPolicy) -> UploadRepr {
    if !s.edited && s.lod >= p.procedural_from { return UploadRepr::Procedural; }
    if s.lod >= p.surface_run_from && !too_noisy(s, p) { return UploadRepr::SurfaceRun; }
    UploadRepr::Octree
}
fn srun_bytes(s: &ChunkStats) -> usize { if s.uniform { 1 } else { COLS + s.total_runs as usize * 2 } }

#[derive(Default, Clone, Copy)]
struct Proj { octree_chunks: u32, octree_bytes: u64, srun_chunks: u32, srun_bytes: u64, proc_chunks: u32 }
fn project<I: IntoIterator<Item = ChunkStats>>(it: I, p: &UploadPolicy, oct_each: u64) -> Proj {
    let mut r = Proj::default();
    for s in it {
        match classify(&s, p) {
            UploadRepr::Octree => { r.octree_chunks += 1; r.octree_bytes += oct_each; }
            UploadRepr::SurfaceRun => { r.srun_chunks += 1; r.srun_bytes += srun_bytes(&s) as u64; }
            UploadRepr::Procedural => { r.proc_chunks += 1; }
        }
    }
    r
}

// --- chunk-set model: count chunks per LOD band by disc-area shell out to radius R ---
fn band_of(r: f64) -> LodLevel {
    if r <= 12.0 { LodLevel::Near } else if r <= 24.0 { LodLevel::Mid } else if r <= 48.0 { LodLevel::Far } else { LodLevel::VFar }
}
fn chunks_in_band(r_in: f64, r_out: f64, radius: f64) -> u32 {
    let lo = r_in.min(radius); let hi = r_out.min(radius);
    if hi <= lo { return 0; }
    (std::f64::consts::PI * (hi * hi - lo * lo)).round() as u32
}
// emit representative stats for a band given its count and the fractions
fn emit_band(out: &mut Vec<ChunkStats>, lod: LodLevel, n: u32) {
    let uniform = (n as f64 * 0.12) as u32;
    let edited  = (n as f64 * 0.03) as u32;
    let noisy   = (n as f64 * 0.02) as u32;
    let typical = n.saturating_sub(uniform + edited + noisy);
    for _ in 0..uniform { out.push(ChunkStats { lod, edited: false, uniform: true,  worst_col_runs: 0,  total_runs: 0 }); }
    for _ in 0..edited  { out.push(ChunkStats { lod, edited: true,  uniform: false, worst_col_runs: 6,  total_runs: 2560 }); }
    for _ in 0..noisy   { out.push(ChunkStats { lod, edited: false, uniform: false, worst_col_runs: 30, total_runs: 3000 }); } // > cap → forced octree
    for _ in 0..typical { out.push(ChunkStats { lod, edited: false, uniform: false, worst_col_runs: 6,  total_runs: 2560 }); }
}
fn chunkset(radius: f64) -> Vec<ChunkStats> {
    let mut v = Vec::new();
    emit_band(&mut v, LodLevel::Near, chunks_in_band(0.0, 12.0, radius));
    emit_band(&mut v, LodLevel::Mid,  chunks_in_band(12.0, 24.0, radius));
    emit_band(&mut v, LodLevel::Far,  chunks_in_band(24.0, 48.0, radius));
    emit_band(&mut v, LodLevel::VFar, chunks_in_band(48.0, radius.max(48.0), radius));
    v
}

fn mb(b: u64) -> f64 { b as f64 / (1024.0 * 1024.0) }

fn main() {
    let policy = UploadPolicy::default();
    let oct_each: u64 = 164 * 1024;
    let pool_mb = 256.0;
    println!("radius  chunks |  ALL-OCTREE (today)  |  WITH SurfaceRun routing                 | fits 256MB pool?");
    println!("        loaded |  octree_pool_MB       |  octreeMB  srunMB  proc%  (oct/srun/proc) |");
    println!("  ------------------------------------------------------------------------------------------------");
    for &radius in &[30.0f64, 48.0, 64.0, 96.0] {
        let set = chunkset(radius);
        let n = set.len() as u32;
        let routed = project(set.clone(), &policy, oct_each);
        let baseline_mb = mb(n as u64 * oct_each);          // every chunk an octree (current behavior)
        let routed_oct_mb = mb(routed.octree_bytes);
        let routed_srun_mb = mb(routed.srun_bytes);
        let proc_pct = 100.0 * routed.proc_chunks as f64 / n as f64;
        let fits_today = if baseline_mb <= pool_mb { "yes" } else { "NO — overflows → OOM fallbacks" };
        let fits_routed = if routed_oct_mb <= pool_mb { "yes" } else { "no" };
        println!("  r={:<4.0} {:>6} |  {:>8.0}  [{}] |  {:>7.0}  {:>6.1}  {:>4.0}%  ({}/{}/{}) | today:{}  routed:{}",
            radius, n, baseline_mb, if baseline_mb<=pool_mb {"ok"} else {"OVER"},
            routed_oct_mb, routed_srun_mb, proc_pct,
            routed.octree_chunks, routed.srun_chunks, routed.proc_chunks, fits_today, fits_routed);
    }
    println!("\n  note: r=30 ≈ your run log's RD-flag=10 (loaded≈2903). 'today' = every loaded chunk forced to octree.");
}
