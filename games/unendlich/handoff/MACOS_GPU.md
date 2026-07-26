# macOS GPU support (Vulkan via MoltenVK)

macOS has no native Vulkan; it runs Vulkan through **MoltenVK** (a Vulkan→Metal
translation layer), which the Vulkan loader exposes as a *portability* driver.

## Done
- **Instance portability** (r174): on `target_os="macos"` the instance adds
  `VK_KHR_portability_enumeration` + the `ENUMERATE_PORTABILITY_KHR` flag, so the
  loader actually exposes the MoltenVK device.
- **Device portability_subset** (r174): the device enables
  `VK_KHR_portability_subset` (spec-required when the device advertises it).
- **Feature negotiation** (r687, `render/context.rs::create_device`): we now
  QUERY `get_physical_device_features2` and enable `drawIndirectCount` (1.2) and
  `shaderDrawParameters` (1.1) ONLY if the device reports them, logging a warning
  otherwise. Blindly enabling an unsupported feature is *the* classic MoltenVK
  `create_device` failure; this makes device creation robust across drivers with
  no change on Linux/desktop (where all features are present).

## Runtime setup (operator, macOS)
1. Install the LunarG Vulkan SDK for macOS (ships MoltenVK + the loader), or
   `brew install molten-vk vulkan-loader vulkan-tools`.
2. Ensure the ICD + loader are found: `VK_ICD_FILENAMES=…/MoltenVK_icd.json`,
   `DYLD_LIBRARY_PATH`/`VK_LOADER…` as the SDK's `setup-env.sh` sets them.
3. `vulkaninfo` should list "MoltenVK" with `VK_KHR_portability_subset`.
4. Build with the same feature set as Linux: `--features
   "render,window,gpu,ui-overlay,audio"` (hot-shaders needs shaderc, which builds
   on macOS via the SDK).

## Still to do (needs a real Mac to iterate)
- **API version**: instance requests Vulkan 1.2. Modern MoltenVK reports ≥1.2, so
  this is fine; on an old loader it could fail. Consider clamping to
  `entry.try_enumerate_instance_version()` (min with 1.2) — deferred to avoid
  touching the Linux path blind.
- **drawIndirectCount fallback**: if MoltenVK reports it UNSUPPORTED, the GPU-
  driven cull path (`vkCmdDrawIndexedIndirectCount`) has no CPU fallback yet, so
  it will warn (r687) then fail at draw time. A non-indirect draw path is the
  real fix for old MoltenVK. Recent MoltenVK supports it, so most Macs are fine.
- **Portability-subset limits**: honour `VkPhysicalDevicePortabilitySubsetProperties`
  (e.g. no wide lines, min vertex-input strides) if any are hit.
- **Retina/scale + surface format**: verify swapchain format + DPI on Metal.
- Test on Apple Silicon (M-series) and Intel Macs.
