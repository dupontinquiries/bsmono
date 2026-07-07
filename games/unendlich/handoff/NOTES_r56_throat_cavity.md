# r56 — dark throat cavity (open jaw no longer shows background)

## What changed (all in-engine, NO bin change required)
The open mouth used to show the background through the gap when the jaw dropped
(the pixel-peeper's residual ~4px bleed). Fixed by giving the dragon a **dark
throat**:

- `dragon_forge::ForgeBox` gained `mouth: bool` (interior cavity boxes).
- `dragon_forge::ForgeModel` gained `mouth_color: V3` (dark, default `[0.04,0.02,0.03]`).
- `build()` emits two cavity boxes rigged to **`J_HEAD`** (not the jaw), set just
  inside the closed mouth line. Shut, they hide behind the jaw; open, they stay put
  and occlude the gap.
- `world::entity::forge_to_vertices` already colours them: `glow→eye_color`,
  `mouth→mouth_color`, else `body_color`. **So this works with no bin wiring.**

The pixel-peeper sweep now passes at **tol 2** (down from 5) across all seeds ×
gait × anim-time including the full gape — the dragon is watertight to ≤2px.

## Optional bin polish (not required)
- For a fire/breath dragon you can make the throat **glow during breath charge**:
  before building vertices, swap `mouth_color` toward the breath element's emissive
  colour scaled by charge01 (e.g. lerp `mouth_color → eye_color * charge`). Purely a
  colour tweak; geometry is already in place.
- `mouth_color` is per-model, so different species/elements can carry different maw
  colours via the palette if you want.

## New/changed this roll (gated: dev gpu + release render + full lib (only the 4
## known pre-existing failures) + HARNESS GREEN)
- `sim::dragon_forge` — `ForgeBox.mouth`, `ForgeModel.mouth_color`, throat boxes in
  `build()`, `BASE_PUFF` 0.10→0.13 (closes the last extreme-swing leg seam).
- `sim::mob_pixel_peeper` — sweep tolerance tightened 5→2 (proves the mouth closed).
- `world::entity::forge_to_vertices` — `mouth → mouth_color` colour branch.
