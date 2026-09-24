# Asahi head start: the shift fault and the 31 GiB ceiling

Two problems on the M2 Ultra under Asahi that are the engine's biggest single-line blockers. This is
where to read in this tree, what is actually proven versus assumed, and where to look in the Asahi stack.
Nothing here is a bug report; it is the material to write one from, by hand.

Facts verified while writing this (2026-09-23): the Asahi kernel's user VA window and page size, from
`AsahiLinux/linux` branch `asahi-wip` (`drivers/gpu/drm/asahi/pgtable.rs`, `mmu.rs`, `file.rs`,
`include/uapi/drm/asahi_drm.h`). Mesa file names below are from memory and marked (verify); the greps
are what to trust.

---

## A. The lane-varying shift returns zero

### What is proven (in this tree)

| claim | where |
|---|---|
| `x << (b*8u)` with `b` varying per lane yields 0 for every lane whose shift is non-zero; `extractBits`/`insertBits` are correct | `crates/gpu-gemm/src/lib.rs:19391–19410` (`BYTE_PROBE`, header comment), `docs/subgroup-unlock.md:20–30` |
| the probe that established it | `crates/gpu-gemm/src/main.rs:4316–4340` (`--gate-bytes`), `src/lib.rs:15442` (`byte_probe`), `:15396–15412` (`id_probe`, `byte_probe_extract/staged/vec4`) |
| the "4-of-16" pattern (three of every four lanes zero) | `src/main.rs:4362–4365`, `src/lib.rs:19164` |
| the workaround, applied to every packer/unpacker | `src/kernels/quant_pack.wgsl:12–19` (`insertBits`), `src/kernels/q4k_dequant.wgsl:7–11`, `src/kernels/q4k_to_q4.wgsl:7–11`, the five `byte_at` helpers at `src/lib.rs:19398, 19419, 19500, 19654, 19751`, and `:19884` |
| three load-time probes were fed corrupt weights because of it | `crates/gpu-gemm/src/store.rs:110–118` |
| the subgroup kernel is gated off by an adapter-name check, not by a measurement | `src/lib.rs:1759` (`conservative_vma = Vulkan && name contains "Apple"`), `:1868–1911` (the gate: `(asahi_vulkan \|\| cfg_off) && !force_sg`, `LOGSCOPE_MATVEC_SG`), `:1962` (`self_check_sg`), `:6455–6458` and `:6540` (autotune skips `V2WgSg` unless `sg_ok`) |
| the kernel the gate hides | `src/lib.rs:1250–1291` (`MatvecKernel::V2WgSg`, `needs_subgroups`), `src/kernels/matvec_q8_0_tiled_v2_wg128_sg.wgsl`, `docs/decode-vs-prefill.md:41–48` |

### What is not proven

* Which Mesa version showed it. The probe ran on the machine in `docs/hardware-matrix.md:37`
  (`7.1.6-400.asahi.fc43` kernel); the Mesa version was not recorded. `rpm -q mesa-vulkan-drivers` on the
  box, and re-run the probe on the newest Fedora Asahi Mesa before anything else: compiler bugs of this
  shape get fixed without anyone telling you.
* Whether the fault is in shift lowering, in the optimizer folding a "uniform" shift, in the scheduler
  reordering, or in instruction packing. The fact that `extractBits` (NIR `ubfe`) is correct while
  `ushr`/`ishl` by a divergent amount is wrong narrows it to the path those two ops take.
* Whether it reproduces outside wgpu/naga. The tree's probe goes WGSL → naga → SPIR-V → Honeykrisp; a
  Mesa report needs the SPIR-V form, so the reproducer must be rebuilt in GLSL (below).

### Minimal reproducer to write by hand

Compute shader, 64 invocations, same shape as `BYTE_PROBE`. GLSL, compiled with `glslangValidator -V`:

```glsl
#version 450
layout(local_size_x = 64) in;
layout(std430, binding = 0) readonly buffer Src { uint src[]; };
layout(std430, binding = 1) writeonly buffer Dst { uint dst[]; };
void main() {
    uint i = gl_GlobalInvocationID.x;
    uint word = src[i >> 2];
    uint sh = (i & 3u) * 8u;                 // lane-varying shift amount
    dst[i]      = (word >> sh) & 0xFFu;      // WRONG on Honeykrisp: 0 for sh != 0 (per --gate-bytes)
    dst[64 + i] = bitfieldExtract(word, int(sh), 8);   // correct
}
```

Feed `src` with bytes 0..63 (byte i = i). Expected: `dst[i] == i` for both halves. Observed (per the tree):
the first half shows `i` only where `i % 4 == 0`, zeros elsewhere; the second half is right. Capture the
compiled shader with the compiler's debug env (see below) so the report can show the instruction stream
rather than a description.

A second variant to include: `uint sh = (gl_SubgroupInvocationID & 3u) * 8u` and a third with `sh` loaded
from a buffer. If the buffer-loaded form is correct and the index-derived form is wrong, the bug is in how
the compiler treats a value it can prove is derived from the invocation id (a "uniform" misclassification),
which is a much smaller search than "shifts are broken".

### Where to look in Mesa (`src/asahi/`)

Directories: `compiler/` (the AGX backend), `vulkan/` (Honeykrisp, files `hk_*.c`), `lib/` (device, BOs,
VA), `genxml/`, `layout/`.

Greps first, file names second:

```
git grep -n "ushr\|ishl\|ishr" src/asahi/compiler/           # where the three shifts are emitted/lowered
git grep -n "ubfe\|bfe\|extr\b\|bfi" src/asahi/compiler/     # the bitfield path that works
git grep -n "divergen\|is_uniform\|uniform" src/asahi/compiler/*.c
git grep -n "shift" src/asahi/compiler/agx_nir_algebraic.py  # algebraic rewrites of shifts (verify name)
```

Expected files (verify with `ls src/asahi/compiler`): `agx_compile.c` (NIR → AGX instruction selection;
the `nir_op_ushr`/`ishl` arms), `agx_optimizer.c` (copy-prop and immediate folding; a folded shift
amount is one candidate), `agx_pack.c` (encoding; an immediate-vs-register operand bit is the other
candidate, and the community's "bit-47" fix in 2026 was exactly an encoding issue), `agx_pressure_schedule.c`
(the scheduler that the 2026 "clustering" patch touched), `agx_register_allocate.c`, `agx_lower_64bit.c`,
`agx_nir_lower_subgroups.c` (verify). Frontend: SPIR-V enters through Mesa's shared `spirv_to_nir`, then
`agx_nir_lower_*` passes; a bug that depends on the shift being derived from the invocation id would live
in those passes or in the optimizer, not in `spirv_to_nir`.

Debug env: the AGX compiler has an `AGX_MESA_DEBUG` variable whose flags include shader disassembly
dumping (grep `AGX_DBG_` in `src/asahi/compiler/agx_compile.h` for the exact names; `shaders` and
`shaderdb` are the usual ones); Honeykrisp has `HK_DEBUG` (grep `HK_DEBUG` in `src/asahi/vulkan/`).
Dump the two kernels, diff the shift path against the bitfield path.

### How to file

Mesa issues: gitlab.freedesktop.org/mesa/mesa, label `asahi`. Required: `vulkaninfo --summary`, Mesa
version, kernel version (`uname -r`, note the `+16k`), the GLSL, the SPIR-V (`spirv-dis`), the dumped
AGX assembly, observed vs expected as a table, and the "buffer-loaded shift is fine / index-derived is
wrong" discriminator if it holds. One issue, one bug. Read the project's contribution and AI policy first
and write it in your own words; the artifacts do the persuading.

---

## B. The ~31 GiB concurrent-buffer ceiling

### What is proven (in this tree)

| claim | where |
|---|---|
| the device will not hold more than ~31 GiB of buffers at once, mapped or unmapped, at slab sizes 1 and 2 GiB | `src/lib.rs:4995–5012` (`MAPPED_CEILING`, the unmapped fallback that did not help), `:11205–11218` |
| it fails with `Failed to allocate BO VMA` | `src/lib.rs:1767`, `:2155`, `:5034`, `:11602` |
| `--gate-residency` held 31 GiB and the next slab failed; `--gate-vram` accepts a single 63 GiB allocation | `docs/hardware-matrix.md:12–20, 95–102`, `src/main.rs:4761` (`--gate-residency`), `:4099` (`--gate-vram`) |
| the page-count hypothesis, unresolved | `docs/hardware-matrix.md:44–52` |
| what it costs: Scout upload-bound at 21 GiB/s, 3.3 tok/s ceiling | `docs/scout-speed-ceiling.md:10–25, 35–44` |
| the budget clamp that encodes it | `src/lib.rs:11199–11218` (`gpu_buffer_budget_uma`, `resident_fits_concurrent`), `LOGSCOPE_MAX_GPU_GB` at `:4118`, `:10472`, `:11469–11476` |

### What the kernel says (verified)

* Page size 16 KiB: `pgtable.rs:39–43` (`UAT_PGBIT = 14`).
* User VA window: `pgtable.rs:63` `UAT_IAS = 39`, so `mmu.rs:100–104` gives
  `IOVA_USER_RANGE = 16 KiB .. 2^39` = **512 GiB**, and `mmu.rs:126` `IOVA_USER_USABLE_RANGE` is that minus
  a reserved page. The VA window is not the 31 GiB limit.
* The kernel carves a *kernel* VMA window **inside** the user range: `file.rs:216` (`VM_USER_RANGE`),
  `:318–319` (`vm_start`/`vm_end` reported to userspace), `:360`, and the `Vm` struct at `:60–80` keeps
  `kernel_range` with a `left_range` and a `right_range` on either side of it. The UAPI documents this in
  `asahi_drm.h:143–168` (`vm_start`, `vm_end`, `vm_kernel_min_size`).

So the leading hypothesis is on the Mesa side: `Failed to allocate BO VMA` is a userspace VMA-heap
failure, and if Honeykrisp initialises its VA heap over only the *left* range below the kernel window
(rather than both sides, or the whole window minus the kernel range), the usable VA is whatever offset
it chose for the kernel window, which would sit exactly where a "31 GiB, regardless of slab size,
regardless of mapping" wall sits. Alternatives to rule out: a per-context limit on BO count or page-table
size (the page-count hypothesis in the tree), and the reported heap size being conservative because GPU
memory is pinned, which `HK_SYSMEM` overrides.

### Experiments that discriminate (no Mesa changes)

1. `HK_SYSMEM=$((150 * 1 << 30)) kernel-validate --gate-residency`. If the ceiling moves, it was the
   reported heap size; if it stays at 31 GiB, it is VA or count, and `HK_SYSMEM` is not the fix.
2. Slab-size sweep at fixed total: 31 GiB in 64 MiB slabs versus 2 GiB slabs. Same total wall ⇒ bytes
   (VA); a lower wall with small slabs ⇒ count/page-table.
3. Read the VA numbers Mesa is given: `vulkaninfo` won't show them, but a small program calling
   `DRM_IOCTL_ASAHI_GET_PARAMS` prints `vm_start`, `vm_end`, `vm_kernel_min_size`; then find where
   Honeykrisp places the kernel window (grep below). If `kernel window start ≈ 32 GiB`, the hypothesis
   is confirmed by arithmetic before any code is read.

### Where to look in Mesa

```
git grep -n "Failed to allocate BO VMA" src/asahi/               # the exact failure site
git grep -n "util_vma_heap_init\|vma_heap" src/asahi/            # how many heaps, over which ranges
git grep -n "vm_kernel_min_size\|vm_start\|vm_end\|kernel_start" src/asahi/   # where the kernel window is placed
git grep -n "HK_SYSMEM\|memoryHeaps\|heapIndex" src/asahi/vulkan/            # the reported heap size and its override
git grep -n "BO_LOW_VA\|LOW_VA\|low_va" src/asahi/               # any second, smaller heap BOs can be forced into
```

Expected files (verify): `lib/agx_device.c` (device init, VM creation, VA heap setup),
`lib/agx_bo.c` (BO create/VA alloc; likely home of the error string), `vulkan/hk_physical_device.c`
(memory heaps, `HK_SYSMEM`), `vulkan/hk_device_memory.c` (`vkAllocateMemory` → BO). Kernel side, if it
comes to it: `drivers/gpu/drm/asahi/file.rs` (`vm_create`, get_params), `mmu.rs` (ranges), `alloc.rs`,
`gem.rs`.

### The write-up, if it is Mesa

This one is likely a request rather than a bug: "the VA heap covers only the sub-window below the
kernel range; on a 192 GB machine that leaves 31 GiB for buffers" is a design limit with an obvious
fix (allocate from both sides, or place the kernel window at the top). Bring the three experiments'
numbers and the arithmetic; it is the kind of report that gets a one-line answer or a one-line fix.

---

## C. Related work already in flight upstream (for context, not to duplicate)

Seen in the llama.cpp Vulkan performance thread in 2026: a Honeykrisp lowering of cooperative matrix
onto the simd_matrix hardware, a "clustering" fix in the pressure scheduler, a "bit-47" immediate-offset
encoding fix (7B prefill 351 → 504 t/s together), and a Mesa MR removing two over-synchronisations that
made every compute dispatch its own submission (per-dispatch fixed cost 8.51 → 4.15 µs). Grep those
threads for MR numbers before writing anything that touches `agx_pressure_schedule.c`, `agx_pack.c`,
or `hk_cmd_dispatch.c` (verify), because the shift fault may already be a side effect of one of them.

## D. Order of operations

1. Record the Mesa version on the box; update to the newest Fedora Asahi Mesa.
2. Re-run `--gate-bytes` with the *old* shift-and-mask form (drop a copy of the pre-`_526` `BYTE_PROBE`
   into `LOGSCOPE_SHADER_DIR`). If it now passes, note the version pair in the ledger as an absence record
   (`looked_for: shift fault / did_not_find / command / scope: mesa <ver>`) and move on to B.
3. If it still fails: build the GLSL reproducer, dump the assembly, write it up (A).
4. Run the three experiments in B; write up whichever the arithmetic supports.
5. Only then `LOGSCOPE_MATVEC_SG=1` with the perf loop: `./loop.sh bench e1-matvec-sg LOGSCOPE_MATVEC_SG=1`
   against a fresh baseline, because the gate's evidence was gathered on the broken fixture and has never
   been re-measured.
