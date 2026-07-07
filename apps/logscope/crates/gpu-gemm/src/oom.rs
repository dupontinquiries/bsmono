//! oom.rs — detect a GPU allocation failure instead of dying on it.
//!
//! wgpu reports out-of-memory through the device error callback, not a `Result`, so a buffer allocation that
//! the byte budget didn't predict (Honeykrisp BO/VMA exhaustion — the "Failed to allocate BO VMA" crash)
//! surfaces as an uncaptured error that panics the thread. Wrapping the allocation in an OutOfMemory error
//! scope turns that into a value the loader can act on: catch the OOM, clear the arena, drop to the next
//! residency on `memplan::fitting_residencies` (int8 → q4), and retry — self-tuning past the budget guess.
//!
//! This is the primitive that makes the downgrade ladder runtime-driven; the loader wraps its resident-weight
//! upload in [`with_oom_scope`] once wired. Compile-verified against wgpu 29; the OOM branch itself is
//! draft-until-hardware (it only fires on a real GPU that actually runs out).

/// Run `f` inside error scopes that catch a GPU allocation failure, returning `Ok(f())` if none occurred or
/// `Err(reason)` if one did — so the caller can downgrade residency and retry instead of the process dying.
///
/// Catches BOTH flavors, because drivers disagree on how they report a failed buffer allocation: some raise
/// `OutOfMemory`, but Mesa/Honeykrisp logs "Failed to allocate BO VMA", hands back an invalid buffer, and
/// only surfaces a **Validation** error ("Buffer is invalid") when you next use it. Stacking a Validation
/// scope outside an OutOfMemory scope catches whichever the driver chose.
pub fn with_oom_scope<R>(device: &wgpu::Device, f: impl FnOnce() -> R) -> Result<R, String> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let r = f();
    // pop innermost (oom) first; block on each to learn whether it caught anything.
    let oom_err = pollster::block_on(oom.pop());
    let val_err = pollster::block_on(validation.pop());
    match (oom_err, val_err) {
        (None, None) => Ok(r),
        (Some(e), _) => Err(format!("GPU out of memory during allocation: {e}")),
        (_, Some(e)) => Err(format!("GPU allocation failed (validation): {e}")),
    }
}

/// Walk the residency downgrade ladder: try `attempt(fmt)` for each format best-first, wrapped in an OOM
/// scope; on OOM, `reset()` (clear the arena) and try the next. Returns the first format that allocated
/// cleanly + its result, or an error listing what was tried. This is the self-tuning loader loop that makes
/// the GPU-budget guess irrelevant — it finds the largest residency that ACTUALLY allocates on this box.
pub fn load_with_downgrade<T>(
    device: &wgpu::Device,
    ladder: &[crate::memplan::Residency],
    mut reset: impl FnMut(),
    mut attempt: impl FnMut(crate::memplan::Residency) -> T,
) -> Result<(crate::memplan::Residency, T), String> {
    let mut tried = Vec::new();
    for &fmt in ladder {
        reset();
        match with_oom_scope(device, || attempt(fmt)) {
            Ok(t) => return Ok((fmt, t)),
            Err(_) => tried.push(fmt.label()),
        }
    }
    Err(format!("every resident format OOM'd on this GPU (tried: {}); layer streaming or a smaller model is required", tried.join(" → ")))
}
