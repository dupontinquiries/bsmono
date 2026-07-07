//! Shadow bucket timer (c.13-instrumentation).
//!
//! Auxiliary GPU timestamp pool used ONLY when
//! `UNENDLICH_SHADOW_BUCKETS=N` is set, to subdivide the shadow pass's
//! timing into N buckets so we can see whether shadow cost is
//! front/middle/back-loaded across the chunk draw list.
//!
//! # Why a separate pool?
//!
//! The main `GpuTimer` has 5 fixed zones (shadow/main/froxel/cull/total)
//! sized in a pool at construction. Adding bucket markers to it would
//! either require widening MAX_ZONES (wasteful when bucketing is off) or
//! a dynamic resize (complex). This module owns its own small pool
//! created on demand.
//!
//! # When to use
//!
//! Diagnostic only. Forces `UNENDLICH_INDIRECT=0` (per-chunk fallback)
//! since bucketing requires inserting timestamps between specific draws,
//! which an indirect-count draw doesn't allow. Don't enable in
//! production runs — measurement perturbations matter.
//!
//! # Output
//!
//! Per frame, emits a structured event `shadow_buckets` with:
//!   - `chunks_per_bucket`: [N], how many chunks fell into each bucket
//!   - `us_per_bucket`: [N], wall GPU time per bucket
//!   - `total_us`: sum of buckets (≈ what gpu_timer's "shadow" zone
//!     reports for the same frame)

use ash::{vk, Device};

/// Maximum number of buckets we support. 16 is plenty; CSM has 4 cascades,
/// and we usually want 4-8 chunk buckets.
pub const MAX_SHADOW_BUCKETS: u32 = 16;

/// Number of timestamps per bucketed shadow pass: N buckets need N+1
/// boundaries (start, between, end).
pub const MAX_BUCKET_TIMESTAMPS: u32 = MAX_SHADOW_BUCKETS + 1;

pub struct ShadowBucketTimer {
    device: Device,
    pool: vk::QueryPool,
    /// Pool layout: `frames_in_flight × MAX_BUCKET_TIMESTAMPS` queries.
    /// Frame F uses queries [F * MAX_BUCKET_TIMESTAMPS, (F+1) * MAX_BUCKET_TIMESTAMPS).
    frames_in_flight: u32,
    timestamp_period_ns: f32,
    timestamp_mask: u64,
    /// Number of buckets the user requested. May be less than MAX.
    bucket_count: u32,
    /// True if device supports timestamps and we got a valid pool.
    supported: bool,
}

impl ShadowBucketTimer {
    /// Create a bucket timer with `bucket_count` buckets per frame.
    /// `bucket_count` will be clamped to `[1, MAX_SHADOW_BUCKETS]`.
    ///
    /// Returns Ok(None) if the device doesn't support timestamps; the
    /// caller should treat this as "bucketing unavailable" and continue
    /// without it. Returns Ok(Some(_)) on success.
    pub fn new(
        device: &Device,
        timestamp_period_ns: f32,
        timestamp_valid_bits: u32,
        bucket_count: u32,
        frames_in_flight: u32,
    ) -> Result<Option<Self>, vk::Result> {
        if timestamp_period_ns == 0.0 {
            log::warn!("ShadowBucketTimer: device reports timestamp_period=0; bucketing disabled");
            return Ok(None);
        }
        let bucket_count = bucket_count.clamp(1, MAX_SHADOW_BUCKETS);

        let timestamp_mask = if timestamp_valid_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << timestamp_valid_bits) - 1
        };

        let pool_count = frames_in_flight * MAX_BUCKET_TIMESTAMPS;
        let pool_info = vk::QueryPoolCreateInfo::builder()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(pool_count);
        let pool = unsafe { device.create_query_pool(&pool_info, None)? };

        log::info!(
            "ShadowBucketTimer ready: buckets={}, pool_count={} (diagnostic mode)",
            bucket_count, pool_count,
        );

        Ok(Some(Self {
            device: device.clone(),
            pool,
            frames_in_flight,
            timestamp_period_ns,
            timestamp_mask,
            bucket_count,
            supported: true,
        }))
    }

    pub fn bucket_count(&self) -> u32 { self.bucket_count }
    pub fn supported(&self) -> bool { self.supported }

    /// Reset this frame's slice of the pool. Call at the start of the
    /// frame's command buffer recording, before any timestamp writes.
    pub fn reset_frame(&self, cmd: vk::CommandBuffer, frame_idx: u32) {
        if !self.supported || frame_idx >= self.frames_in_flight { return; }
        let first = frame_idx * MAX_BUCKET_TIMESTAMPS;
        unsafe {
            self.device.cmd_reset_query_pool(
                cmd, self.pool, first, MAX_BUCKET_TIMESTAMPS,
            );
        }
    }

    /// Write a bucket-boundary timestamp at the current command-buffer
    /// position. `boundary_idx` ranges 0..=bucket_count (we need N+1
    /// boundaries for N buckets).
    ///
    /// Uses BOTTOM_OF_PIPE so the timestamp fires after all prior
    /// commands have finished GPU execution.
    pub fn mark_boundary(
        &self,
        cmd: vk::CommandBuffer,
        frame_idx: u32,
        boundary_idx: u32,
    ) {
        if !self.supported || frame_idx >= self.frames_in_flight { return; }
        if boundary_idx > self.bucket_count { return; }
        let q = frame_idx * MAX_BUCKET_TIMESTAMPS + boundary_idx;
        unsafe {
            self.device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.pool,
                q,
            );
        }
    }

    /// Read back the bucket boundaries from the previous frame and
    /// return per-bucket microseconds. Returns Vec of length
    /// `bucket_count` (or empty Vec if not yet available).
    ///
    /// Caller passes the frame index that just completed (not the
    /// frame currently being recorded). Errors that look like "not
    /// ready yet" return an empty Vec rather than propagating.
    pub fn collect_frame(&self, frame_idx: u32) -> Vec<u64> {
        if !self.supported || frame_idx >= self.frames_in_flight { return Vec::new(); }
        let first = frame_idx * MAX_BUCKET_TIMESTAMPS;
        let count = self.bucket_count + 1;
        let mut raw = vec![0u64; count as usize];
        let result = unsafe {
            self.device.get_query_pool_results::<u64>(
                self.pool,
                first,
                count,
                &mut raw,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        };
        if result.is_err() { return Vec::new(); }

        // Mask off invalid bits.
        for v in raw.iter_mut() { *v &= self.timestamp_mask; }

        // Compute deltas. Use saturating_sub against wraparound.
        let mut out = Vec::with_capacity(self.bucket_count as usize);
        for i in 0..self.bucket_count as usize {
            let start = raw[i];
            let end = raw[i + 1];
            let ticks = end.wrapping_sub(start) & self.timestamp_mask;
            let ns = (ticks as f64) * (self.timestamp_period_ns as f64);
            let us = (ns / 1000.0) as u64;
            out.push(us);
        }
        out
    }

}

impl Drop for ShadowBucketTimer {
    fn drop(&mut self) {
        // Destroy the query pool. The pool is solely owned by this
        // struct; safe to free here.
        unsafe { self.device.destroy_query_pool(self.pool, None); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_shadow_buckets_constant_is_sane() {
        // CSM has 4 cascades. Even at one bucket per cascade × cascade,
        // we'd need 16. The constant is set there. Bumping it would
        // require expanding MAX_BUCKET_TIMESTAMPS too.
        assert_eq!(MAX_SHADOW_BUCKETS, 16);
        assert_eq!(MAX_BUCKET_TIMESTAMPS, MAX_SHADOW_BUCKETS + 1);
    }
}
