# Screen Temperature Bitfield

`ambient-learner` publishes a 2D reconstruction of the screen surface
temperature to a well-known path while running. Any app on the device
can read it. The intended use cases include:

- **Capacitive touch correction in wet conditions.** Water droplets on
  the screen produce false touches at *cold* spots (the droplet pulls
  heat from the surface). Subtracting the cold-spot pattern from raw
  touch data lets the input layer recover real touches.
- **Glove-finger detection.** A finger inside a glove still warms the
  contact patch. A warm spot under an ambiguous touch event suggests
  "this was a finger"; pure cold + touch suggests precipitation.
- **Grip detection.** The hand holding the phone leaves a slow warm
  patch along one side.
- **Pocket / context detection.** Uniformly warm + dark = pocketed;
  cold + bright = sitting on a table.

## Paths

```
$XDG_RUNTIME_DIR/ambient-learner/
├── screen-temps.bin            # binary frame, updated 4 Hz
└── screen-temps.meta.json      # format description, written once
```

If `$XDG_RUNTIME_DIR` is unset, we fall back to `/tmp`. The directory
is created automatically on first publish.

## Binary format

```
Offset   Bytes  Type     Field
0        4      bytes    Magic: ASCII "AMBT"
4        2      u16 LE   Format version (currently 1)
6        2      u16 LE   Grid columns
8        2      u16 LE   Grid rows
10       2      u16 LE   Reserved (zero)
12       8      u64 LE   Timestamp (Unix nanoseconds)
20       4      u32 LE   Generation counter (monotonic)
24       8      u64 LE   Reserved (zero)
32       N*4    f32 LE   Grid cells, row-major, °C
```

For the default 10×16 grid that's 32 + 640 = 672 bytes.

## Coordinate system

- Origin (0, 0) is top-left.
- X increases left-to-right (columns).
- Y increases top-to-bottom (rows).
- Cells are sampled at their centers; cell (c, r) corresponds to
  normalized screen position `((c+0.5)/cols, (r+0.5)/rows)`.

## Atomic updates

We write to `screen-temps.bin.tmp` and `rename(2)` it over the final
path. The rename is atomic at the inode level, so a consumer reading
the file will see either the previous frame intact or the new frame
intact — never a torn frame.

The `generation` counter in the header increments on every write. A
consumer polling faster than our publish rate can read just the
32-byte header, check the generation, and skip the body read when
nothing has changed.

## Generation strategy

Each frame is reconstructed from the device's individual thermal
zones (CPU, GPU, battery, modem, charger, etc.) using a 2D Gaussian
diffusion model. Each sensor has a known on-PCB location and
coupling strength to the screen surface; we sum each sensor's
Gaussian-weighted contribution into the grid.

This is an **approximation**. The reconstruction will tell you "the
top half of the screen is hotter than the bottom" reliably; it
won't tell you the exact temperature of any specific pixel. Use it
for *spatial pattern detection*, not absolute thermometry.

The Gaussian σ defaults to 0.30–0.40 of the screen diagonal per
sensor. Outside the 3σ cutoff we skip the kernel evaluation entirely
to keep the per-frame cost under 20 µs.

## Reading example (Python)

```python
import struct, mmap, time, os

path = f"{os.environ['XDG_RUNTIME_DIR']}/ambient-learner/screen-temps.bin"

with open(path, "rb") as f:
    data = f.read()

magic = data[0:4]
assert magic == b"AMBT", f"unexpected magic {magic!r}"

version, cols, rows, _ = struct.unpack_from("<HHHH", data, 4)
ts_ns, gen, _ = struct.unpack_from("<QIQ", data, 12)

import numpy as np
grid = np.frombuffer(data[32:], dtype="<f4").reshape(rows, cols)

print(f"{cols}x{rows} grid, gen={gen}")
print(grid)
```

## Reading example (C)

```c
#include <stdio.h>
#include <stdint.h>
#include <string.h>

struct __attribute__((packed)) Header {
    char     magic[4];
    uint16_t version;
    uint16_t cols;
    uint16_t rows;
    uint16_t _reserved;
    uint64_t timestamp_ns;
    uint32_t generation;
    uint64_t _reserved2;
};

int main() {
    FILE *f = fopen("/run/user/1000/ambient-learner/screen-temps.bin", "rb");
    struct Header h;
    fread(&h, sizeof(h), 1, f);
    if (memcmp(h.magic, "AMBT", 4) != 0) return 1;

    float grid[h.cols * h.rows];
    fread(grid, sizeof(float), h.cols * h.rows, f);
    fclose(f);

    printf("grid %ux%u gen=%u\n", h.cols, h.rows, h.generation);
    for (int r = 0; r < h.rows; r++) {
        for (int c = 0; c < h.cols; c++) {
            printf("%5.1f ", grid[r * h.cols + c]);
        }
        printf("\n");
    }
}
```

## Reading example (shell, just the header)

```bash
hexdump -C -n 32 $XDG_RUNTIME_DIR/ambient-learner/screen-temps.bin
```

## Permissions and isolation

- The directory and files are owned by your user, mode 0600 / 0700.
- This is per-user-session data; another logged-in user's apps
  can't read your bitfield.
- The data is held in `tmpfs` (no disk writes) and disappears on
  user logout.

## Requesting higher resolution

The published grid size is **adaptive**. By default we publish at a
resolution justified by the information content of the underlying
sensors — typically 6×10 to 10×16 cells. A consumer that wants a
finer grid (say, a touch input filter that maps screen-pixel
coordinates to grid cells) can request it by dropping a hint file
in the publisher directory:

```
$XDG_RUNTIME_DIR/ambient-learner/screen-temps.hints.<your-app>.json
```

Naming convention: `screen-temps.hints.json` or
`screen-temps.hints.<anything>.json`. Use a per-app suffix so
multiple consumers can express their needs independently.

Schema (all fields optional):

```json
{
  "min_cols": 20,
  "min_rows": 32,
  "expires_unix_sec": 1714329600,
  "watchers": [
    { "name": "rain-touch-filter", "pid": 12345 }
  ]
}
```

We use **the maximum** request across all non-stale hint files, then
clamp to `[4..32]` columns and `[6..48]` rows. A request smaller
than our natural resolution is ignored — we never publish *less*
detail than the sensor count justifies.

`expires_unix_sec` lets stale consumers fall out of the rotation.
A reasonable pattern is to refresh your hint file every 5 minutes
with `expires_unix_sec` set 30 minutes ahead; if you crash, the
publisher reverts to its natural resolution within 30 minutes.

The hints directory is the same as the binary file's directory, so
write-then-rename guarantees apply.

### Hint example (Python)

```python
import json, os, time

dir_ = f"{os.environ['XDG_RUNTIME_DIR']}/ambient-learner"
hint = {
    "min_cols": 20,
    "min_rows": 32,
    "expires_unix_sec": int(time.time()) + 1800,
    "watchers": [{"name": "rain-touch-filter", "pid": os.getpid()}]
}

tmp = f"{dir_}/screen-temps.hints.rain-touch-filter.json.tmp"
final = f"{dir_}/screen-temps.hints.rain-touch-filter.json"
with open(tmp, "w") as f:
    json.dump(hint, f)
os.rename(tmp, final)
```

### Removing your hint

Just delete the file when you're done:

```bash
rm $XDG_RUNTIME_DIR/ambient-learner/screen-temps.hints.your-app.json
```

The publisher will fall back to the next-highest active request, or
to its natural resolution if none remain.

### Tradeoffs

| Resolution | Bytes / frame | Use case |
|---|---|---|
| 4×6 (minimum) | 128 | Hand-grip / pocket detection on starved devices |
| 6×10 (small sensor count) | 272 | Hand-grip / pocket detection |
| 10×16 (typical phone) | 672 | Coarse touch correction |
| 20×32 (dense-sensor device) | 2,592 | Fine touch correction, paint-style heat map |
| 32×48 (visualization / research) | 6,176 | Sharp visualization of dense sensor arrays |
| 64×96 (absolute max) | 24,608 | Hard cap; not normally chosen automatically |

Higher resolution doesn't mean more accurate readings — the
underlying input is the same set of point sensors. It does mean
finer interpolation in the output, which is what matters if you're
mapping screen pixels to grid cells.

### How resolution is decided

The publisher chooses a grid size each tick by combining three
signals:

1. **Nyquist limit**: with N sensors and ~1/√N spacing, we can
   resolve features down to 2× the spacing. So `~2·√N` cells per
   dimension is the upper bound the *sensor count* justifies.

2. **Kernel σ limit**: cells smaller than σ/2 are pure interpolation
   artifacts of the Gaussian kernel. So `~2/σ` cells per dimension
   is the upper bound the *kernel width* justifies.

3. **Consumer hints**: any non-stale hint file requesting at least
   `min_cols × min_rows` lifts the floor up to that size (capped
   at the global maximums above).

The natural resolution is `min(Nyquist, σ-limit)`. Consumer hints
override upward only — we never publish *less* detail than the
sensors justify.

Worked examples:

| Sensors | σ used | Nyquist limit | σ limit | Natural grid |
|---|---|---|---|---|
| 4 | 0.35 | ~4 cells/dim | ~5.7 cells/dim | 4×6 |
| 10 | 0.30 | ~6.3 cells/dim | ~6.7 cells/dim | 4×6 |
| 53 (matched) | 0.14 | ~14.6 cells/dim | ~14.3 cells/dim | 8×14 |
| 53 (mismatched) | 0.35 | ~14.6 cells/dim | ~5.7 cells/dim | 3×6 |
| 100 (matched) | 0.10 | ~20 cells/dim | ~20 cells/dim | 12×20 |

The "53 mismatched" row is instructive: if you have lots of sensors
but the kernel is wider than the spacing, the reconstruction is
σ-limited and adding sensors doesn't help. The publisher
automatically chooses the smaller σ when one is supplied externally
(via a per-device layout config).

## Performance

- Publisher updates at 4 Hz. Each publish is ~20 µs of CPU and one
  672-byte write to tmpfs.
- A consumer polling at 60 Hz can read the 32-byte header to check
  the `generation` field and only re-read the body when it changes.

## Caveats

- The reconstruction is **best-effort**. It works well for slow
  thermal patterns (the screen has been warm here for minutes) and
  poorly for fast transients (sudden GPU spikes).
- Per-device tuning helps. The default sensor positions assume a
  generic phone layout; a real device-specific config can place
  the CPU, battery, modem, and charger at their actual PCB
  coordinates for much sharper reconstructions.
- The format version is `1`. We commit to backward compatibility
  for the header layout and won't break magic/version checking.
  If we add fields, they go into the reserved space; if we change
  cell encoding or row order, the version number bumps.
