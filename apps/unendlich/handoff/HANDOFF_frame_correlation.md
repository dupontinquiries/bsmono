# HANDOFF — wiring `sim::frame_correlation` into the frame loop (coordinator, on-device)

The probe is built + gated (11/11 @1.96). It is **pure** — it needs the bin to feed it one
`FrameSample` per frame and to print `summary()` at shutdown. This is the instrumentation that
turns the r331 lifetime aggregates (which can't tie a 639 ms `main` spike and a 1021 ms
`near`-mesh to the *same* frame) into an attributed, correlated tail.

## Wire-up (bin, ~`unendlich_fullgame.rs` frame loop + shutdown)
1. Construct once: `let mut corr = sim::frame_correlation::CorrelationProbe::new(8.3, 32);`
   (8.3 ms = 120 Hz budget; keep the worst 32 frames). Store alongside the existing timers.
2. Per frame, after the GPU/CPU/mesher timers for the frame are known, fill a sample from values
   already recorded this round and `observe` it:
   ```rust
   use sim::frame_correlation::FrameSample;
   let mut s = FrameSample::new(frame_index);
   s.total_ms        = total_ms;          // from the `total` GPU timer
   s.main_ms         = main_ms;           // `main` GPU timer
   s.cull_ms         = cull_ms;           // `cull` GPU timer
   s.record_ms       = record_ms;         // CPU `record` timer
   s.wait_fences_ms  = wait_fences_ms;    // CPU `wait_fences` timer
   s.near_mesh_ms    = near_mesh_ms_this_frame;   // max near-LOD build this frame (mesher_timer)
   s.draw_count      = draw_count;        // indirect draw count this frame
   s.uploaded_kb     = uploaded_bytes_this_frame / 1024;   // deferred-upload-queue flush size
   s.chunks_meshed   = chunks_meshed_this_frame;
   s.vert_velocity   = camera_vertical_speed.abs();        // blocks/s — the stutter1 hypothesis
   if let Some(cause) = corr.observe(s) {
       // optional: live log only the *worst* frames to avoid spam
       if total_ms > 100.0 { log::warn!("[FRAMECORR] frame {frame_index} {cause:?} {total_ms:.0}ms"); }
   }
   ```
   Most of these are already computed for the existing summaries; `uploaded_kb` comes from the
   mesh-upload path (`mesh_buffer_pool` flush) and `vert_velocity` from the camera controller.
3. At shutdown, print the correlated summary next to the GPU-timer summary:
   ```rust
   for line in corr.summary().lines() { log::info!("{line}"); }
   ```

## What the next M2 run will tell us
The `[FRAMECORR]` lines give, per cause, the spike count, and for the worst N frames the
`main/near_mesh/cull/record/wait` split + `up=…kB`, `draws=…`, `vvel=…`. Concretely this
answers the open questions from the r331 read:
- Are the 600 ms `main` spikes **Upload** (big `up=…kB` that frame) or **GpuMain** (tiny upload)?
- Do the **Meshing** spikes line up with high `vvel` (fast descent → the streaming storm)?
- Is the steady `record` ever the *dominant* cause, or always a passenger behind GPU?

That attribution is the prerequisite for the main-pass / upload-flush fix the coordinator owns
(`froxel_fill.comp` / `chunk.frag` are coordinator-only), and it tells HH-T1 (near-mesh budget)
whether the 1 s near-mesh spikes are velocity-correlated.

## Notes
- `UPLOAD_BURST_KB` (256) is the threshold splitting Upload vs GpuMain; tune from the first run's
  `up=…kB` distribution.
- Zero overhead off the hot path: `observe` is a few comparisons + a bounded insert only on spike
  frames. Safe to leave compiled in (gate behind an env flag if you want it off by default).
