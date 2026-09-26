# rav1d-hybrid: the base for a GPU-assisted AV1 decoder

Base: https://github.com/memorysafety/rav1d at
d3d1cd67059f47803919be8276650e5870c9fd02 (2026-08-14). Not vendored (13 MB of
source); patches against that commit live here and `measure.sh` applies
them.

Why rav1d: a Rust port of dav1d, bit-identical to dav1d on SVT-AV1 streams
(per-frame md5, film grain included), 93% of dav1d's speed in the sandbox.
Its frame-threading mode already splits decoding in two: pass 1 parses a
frame and STORES coefficients, modes and motion vectors; pass 2 reconstructs
from them. That stored state is the CPU -> GPU boundary.

## 0001-stage-cpu-times.patch

Thread-CPU time per task type (entropy pass 1, reconstruction pass 2,
deblock, CDEF, superres, loop restoration, film grain), printed by the CLI
when RAV1D_STAGE_TIMES is set. Output stays bit-exact.

Measured in the sandbox (x86_64, 2-pass mode):

  stage                     720p SVT-AV1 p5    1080p SVT-AV1 + grain
  entropy (pass 1)             71.6%              79.6%
  reconstruction (pass 2)      22.5%              12.1%
  deblock                       1.1%               2.1%
  cdef                          4.8%               1.4%
  film grain                     --                4.8%

Everything after pass 1 -- what a GPU could take -- is 20-28%. The rest is
serial entropy decoding that stays on the CPU in any hybrid. Measure on the
target (M2, Fairphone) with real content before building GPU stages:
`measure.sh FILE.ivf|.webm|.mkv`.
