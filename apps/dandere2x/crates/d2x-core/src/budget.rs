//! Memory budgets and what they buy.
//!
//! Nothing here allocates; it turns "how much memory may this job use" into
//! reference-ring depths, a block-cache size, and a VRAM figure for the GPU
//! runner's tile/in-flight planning. The default RAM budget follows
//! gpu-gemm's memplan: half of `MemAvailable`, which is the figure that
//! survives contact with the page cache and the encoder's own appetite.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Host RAM the job may hold (references, block cache, frame queues).
    pub ram: u64,
    /// GPU-visible memory for activation buffers. On unified memory
    /// (Apple/Asahi) this is the same pool as `ram`; keep the sum sane.
    pub vram: u64,
}

/// Linux `/proc/meminfo` `MemAvailable`, bytes.
pub fn available_memory_bytes() -> Option<u64> {
    let text = std::fs::read_to_string(Path::new("/proc/meminfo")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// "64G", "512M", "8192" (bytes), "0.5T".
pub fn parse_bytes(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: f64 = num.trim().parse().map_err(|_| anyhow::anyhow!("bad size {s:?}"))?;
    let mult: f64 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "K" | "KB" | "KIB" => 1024.0,
        "M" | "MB" | "MIB" => 1024.0 * 1024.0,
        "G" | "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        u => anyhow::bail!("unknown unit {u:?} in {s:?}"),
    };
    Ok((n * mult) as u64)
}

impl MemoryBudget {
    /// Half of available RAM; a quarter of that for the GPU by default —
    /// `vram` is meant to be overridden on discrete cards.
    pub fn detect() -> Self {
        let avail = available_memory_bytes().unwrap_or(8 << 30);
        let ram = avail / 2;
        MemoryBudget { ram, vram: ram / 4 }
    }
}

/// How a RAM budget is spent per pipeline worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingPlan {
    pub recent_refs: usize,
    pub keyframe_refs: usize,
    /// Bytes for the content-keyed block cache (shared across workers).
    pub block_cache_bytes: u64,
    pub bytes_per_ref: u64,
}

/// Split `budget.ram` across `jobs` workers: 40 % to reference rings, 20 %
/// to the block cache, the rest headroom for decoder/encoder/frame queues.
/// Recent references stop helping past a second or two of frames, so they
/// are capped; retained shot starts take the remainder.
pub fn plan_ring(budget: &MemoryBudget, in_w: u32, in_h: u32, scale: u32, jobs: usize, recent_cap: usize, keyframe_cap: usize) -> RingPlan {
    let jobs = jobs.max(1) as u64;
    let bytes_per_ref = (in_w as u64 * in_h as u64 * 3) * (1 + (scale as u64) * (scale as u64));
    let for_refs = budget.ram * 40 / 100 / jobs;
    let n = (for_refs / bytes_per_ref.max(1)) as usize;
    let recent = n.min(recent_cap).max(1);
    let keyframes = n.saturating_sub(recent).min(keyframe_cap).max(1);
    RingPlan { recent_refs: recent, keyframe_refs: keyframes, block_cache_bytes: budget.ram * 20 / 100, bytes_per_ref }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_bytes("64G").unwrap(), 64 << 30);
        assert_eq!(parse_bytes("512M").unwrap(), 512 << 20);
        assert_eq!(parse_bytes("8192").unwrap(), 8192);
        assert_eq!(parse_bytes("0.5T").unwrap(), 512 << 30);
        assert!(parse_bytes("lots").is_err());
    }

    #[test]
    fn ring_plan_scales_with_budget_and_workers() {
        // 1080p x2: 6.2 MB low-res + 24.9 MB high-res per reference.
        let b = MemoryBudget { ram: 96 << 30, vram: 8 << 30 };
        let p1 = plan_ring(&b, 1920, 1080, 2, 1, 32, 4096);
        assert_eq!(p1.bytes_per_ref, 1920 * 1080 * 3 * 5);
        assert_eq!(p1.recent_refs, 32);
        assert!(p1.keyframe_refs > 1000, "{p1:?}");
        let p4 = plan_ring(&b, 1920, 1080, 2, 4, 32, 4096);
        assert!(p4.keyframe_refs < p1.keyframe_refs / 3);
        let tiny = plan_ring(&MemoryBudget { ram: 64 << 20, vram: 0 }, 1920, 1080, 2, 1, 32, 4096);
        assert_eq!((tiny.recent_refs, tiny.keyframe_refs), (1, 1));
    }
}
