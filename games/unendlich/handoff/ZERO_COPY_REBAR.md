# r704 — ReBAR zero-copy mesh upload (ash, NOT wgpu)

## Why not wgpu
The engine is already raw `ash`/Vulkan. wgpu is a safe abstraction that HIDES the
memory-type + queue control zero-copy needs — switching would be a multi-month
rewrite that makes zero-copy HARDER. The zero-copy win is an ash memory-type
change, done here.

## The change
`create_persistent_buffer` (mesh_buffer_pool.rs) now prefers
`DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT` (ReBAR / Resizable BAR) for the mesh
vertex/index buffers, mapped persistently. On ReBAR systems + unified-memory iGPUs
this is memory the CPU memcpys into directly AND the GPU reads at full device-local
speed:
- No staging buffer, no vkCmdCopyBuffer, no TRANSFER→VERTEX_INPUT barrier
  (kills the DEVICE_LOCAL_MESH=1 copy stall).
- No PCIe read penalty (kills the DEVICE_LOCAL_MESH=0 slow-read downside).
- `write_at` lands packed verts straight in the buffer the shader fetches.

Fallback: if the ReBAR memory type is absent OR its allocation fails (small BAR
window on an older discrete GPU can't fit the buffer), it falls back to plain
HOST_VISIBLE | HOST_COHERENT — byte-for-byte the old behaviour. So it's safe on all
hardware. Opt out: `UNENDLICH_NO_REBAR=1`. A one-time log line confirms when ReBAR
is active.

## Interaction with the mesh mode flags
This makes the DEFAULT (non-staging) mesh path zero-copy, so you no longer need
`UNENDLICH_DEVICE_LOCAL_MESH` at all — leave it unset. ReBAR gives you device-local
speed WITHOUT the staging copies that mode 1 added.

## Caveats (needs your build to confirm)
- Render module — can't compile/profile in-sandbox; delimiter-checked + reasoned.
- The vertex buffer cap is large (~3 GB). On a ReBAR discrete GPU this now reserves
  that in VRAM instead of system RAM; if it crowds other resources, either lower
  the mesh cap or set UNENDLICH_NO_REBAR=1. On iGPUs DEVICE_LOCAL == HOST_VISIBLE
  (same physical memory) so there's no downside.
- Verify the "ReBAR zero-copy mesh memory ACTIVE" log appears, then check the
  upload stutters + wait_fences during loading — they should drop sharply.

## r707 — ReBAR supersedes the staging path (footgun fix)
CONFIRMED working on user hardware: "mesh_pool: ReBAR zero-copy mesh memory ACTIVE".
BUT: `UNENDLICH_DEVICE_LOCAL_MESH` defaults ON (staging), and in staging mode the
vertex/index buffers go through `create_device_local_buffer` (device-local, no map)
— NOT `create_persistent_buffer` where ReBAR lives. So ReBAR only reached the mesh
buffers with an explicit `DEVICE_LOCAL_MESH=0`; unset or `=1` silently used staging
+ the copy stall. Backwards, since ReBAR strictly dominates staging.

r707: at pool construction, probe `rebar_available()` (any memory type with
DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT). When present (and not NO_REBAR),
force the direct create_persistent_buffer path regardless of DEVICE_LOCAL_MESH:
`device_local_enabled = device_local_requested && !rebar_ok`. Now zero-copy is the
DEFAULT on ReBAR hardware; the flag can't bypass it. Staging remains only for
non-ReBAR GPUs. Logs "ReBAR available — ignoring UNENDLICH_DEVICE_LOCAL_MESH".
User no longer needs any mesh-memory flag.
