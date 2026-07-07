# Running unendlich on macOS via MoltenVK — feasibility spike

**Question:** can the existing Vulkan codebase (today targeting Asahi Linux /
Honeykrisp) also run on macOS through MoltenVK (Vulkan→Metal translation), as a
second dev/test target and a parallelization win?

**Verdict: feasible.** No hard portability blockers were found in the audit. It
needs a small amount of macOS-only instance/device setup, and there is one
*performance* caveat (GPU-driven culling is CPU-emulated under Metal). Nothing
requires a Metal backend; MoltenVK should carry the existing Vulkan path.

This is a paper spike — it audits the actual code against MoltenVK's known
limits. It has not been built on a Mac. The "next steps" are what an actual Mac
build attempt should do.

## What the audit found (evidence from the tree)

Clean — no action needed:
- **No geometry or tessellation shaders.** Metal has neither; their presence
  would be a hard blocker. The shader dir has only `.vert` / `.frag` / `.comp`.
- **No subgroup / ballot ops** in any shader. MoltenVK only maps a subset of
  these to Metal SIMD-group ops, so avoiding them sidesteps a whole risk class.
- **Compute workgroups are small** — the largest is 8×8×8 = 512 threads, under
  Metal's 1024-threads-per-threadgroup limit. No oversized dispatch to split.
- **Formats are mainstream** — rgba8 / rgba16f storage images, a 3D storage
  texture + sampler3D (froxel), D32_SFLOAT depth, R16G16B16A16_SFLOAT color.
  All are supported by MoltenVK on Apple-silicon Metal.
- **Only one device extension is requested** (`VK_KHR_swapchain`), which
  MoltenVK provides via `CAMetalLayer`.
- **Windowing is GLFW**, which creates the Metal surface on macOS itself
  (`glfwGetRequiredInstanceExtensions` returns the metal-surface extension),
  so the surface path needs no code change.

Device features the engine enables: `shaderDrawParameters` (Vulkan 1.1 — MoltenVK
supports it) and `drawIndirectCount` (Vulkan 1.2 — see the caveat below).

## Required macOS-only setup (two items)

These are conditional additions in `render/context.rs`, gated to macOS so the
Asahi/Linux build is byte-for-byte unchanged. They are *required* by the Vulkan
portability rules, not optional tuning.

1. **Instance: portability enumeration.** A current Vulkan loader will not even
   list the MoltenVK physical device unless the instance opts into portability
   enumeration. Add (macOS only):
   ```rust
   // instance extensions: also push
   //   VK_KHR_get_physical_device_properties2 (dependency, usually already present)
   //   VK_KHR_portability_enumeration
   // and set the create flag:
   let info = vk::InstanceCreateInfo::builder()
       .application_info(&app_info)
       .enabled_extension_names(&ext_ptrs)
       .flags(vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR); // macOS only
   ```

2. **Device: portability subset.** If a physical device advertises
   `VK_KHR_portability_subset` (MoltenVK always does), the Vulkan spec *requires*
   it to be enabled at device creation. Add it to `device_exts` when present:
   ```rust
   // when enumerate_device_extension_properties lists "VK_KHR_portability_subset",
   // append it to device_exts before create_device.
   ```
   It is also worth chaining `VkPhysicalDevicePortabilitySubsetFeaturesKHR` into
   the device create `push_next` so any subset-feature requirements are explicit.

GLFW must be able to find the MoltenVK ICD at runtime — i.e. the Vulkan SDK
installed (or `libvulkan` + `libMoltenVK` + the `MoltenVK_icd.json` discoverable
via `VK_ICD_FILENAMES` / `VK_DRIVER_FILES`). The launcher's cached-binary spawn
should pass those env vars through on macOS.

## The one caveat: GPU-driven culling (`drawIndirectCount`)

The chunk renderer issues `vkCmdDrawIndexedIndirectCount` (two call sites in
`world_renderer.rs`) — the GPU writes the visible-chunk count and the GPU reads
it to decide how many draws to issue. On MoltenVK:

- The `drawIndirectCount` feature is exposed *conditionally* (MoltenVK forwards
  its internal capability), so on a **recent** MoltenVK enabling it succeeds and
  the command works. On an **older** MoltenVK the bit may be false, in which case
  enabling it would fail device creation.
- Metal has no GPU-side draw-count parameter, so MoltenVK implements the count
  (and multi-draw indirect generally) by **looping on the CPU** in the driver —
  it reads the count and submits that many individual Metal draws. So the path
  is correct but loses its main benefit: the per-frame chunk draws are
  CPU-encoded one by one, which can become an encode bottleneck with many
  visible chunks.

**Recommendation:**
- Don't hard-require `drawIndirectCount`. Query
  `VkPhysicalDeviceVulkan12Features::drawIndirectCount` via
  `GetPhysicalDeviceFeatures2` and only enable it if true (this also future-proofs
  the Linux path).
- For Mac specifically, expect the GPU-driven path to be CPU-encode-bound. If it
  matters, a Mac-friendlier draw path would batch chunks into fewer draws or cap
  the indirect loop. This is a *performance* follow-up, not a prerequisite to
  booting.

## Other notes

- **Vulkan version:** the instance requests 1.2. Recent MoltenVK reports ≥1.2, so
  this is fine; if a target MoltenVK reports only 1.1 the request would need to
  drop, but that's unlikely on current versions.
- **Validation:** use the Vulkan SDK's validation layers on macOS (they work
  with MoltenVK) — the same `run.sh` discipline applies.
- **No `unsafe` format/memory assumptions** specific to a vendor were found; the
  `find_memory_type` DEVICE_LOCAL search and `OffscreenTarget`/`DepthBuffer`
  patterns are vendor-neutral.

## Next steps (when actually attempting a Mac build)

1. Install the Vulkan SDK for macOS (bundles MoltenVK + loader + layers).
2. Apply the two macOS-only setup items above (cfg-gated to `target_os = "macos"`).
3. Make `drawIndirectCount` opt-in-on-support (query, don't assume).
4. `cargo run` a debug build; fix any validation-layer complaints (most likely
   missing portability bits, which step 2 covers).
5. If it boots: profile the chunk draw path; decide whether the CPU-looped
   indirect draws need a Mac batching path.

**Effort estimate:** booting to a rendered frame is small (the two setup items +
the feature query — an afternoon, mostly iterating on validation messages). The
GPU-driven-culling perf path on Metal is the only potentially large follow-up,
and only if the CPU encode cost shows up in a profile.

I can wire the cfg-gated setup (items 1–3) as a brick whenever you want to try a
Mac build — flagged separately because it's `#[cfg(target_os = "macos")]` code
that won't be exercised by the Asahi build, so it can't be sandbox-validated and
wants a real Mac compile to confirm.
