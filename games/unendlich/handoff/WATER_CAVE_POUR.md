# Water: cave pour + fractional heights (r680)

Playtest: (1) "water bordering caves on the edge of oceans still does not pour to
the side"; (2) "placed water never disperses into half-/quarter-height cells in
all directions like water on open ground should."

## Issue 1 — ocean pours into adjacent caves  ✅ FIXED
Root cause: the dynamic water sim models world COLUMNS (base + depth = free
surface). A column whose floor is below the sim window is "bottomless" (deep
ocean). Since r441, bottomless columns were made fully INERT (`if floor[i]==MIN
{ continue; }`) to avoid manufacturing water — but that also stopped an ocean from
pouring into an adjacent lower cave/cliff gap.

Fix (bin inline sim + `sim::water_sim_core` reference, kept in sync): a bottomless
column is now an INFINITE SOURCE — it SENDS downhill into lower real neighbours
(ocean fills the cave), is not limited by its own window depth (only by
`max_flow`), and in the write-back its own level is HELD (never drains). It still
never RECEIVES (neighbours skip `floor[j]==MIN` targets). Because a bottomless
column's gather surface already equals true sea level, it fills caves TO sea level
and then stops (df→0) — no runaway rise (the r441 fear) and no ocean drain. Two new
`water_sim_core` tests prove it: `bottomless_ocean_pours_into_adjacent_cave_to_sea_
level` and `bottomless_ocean_does_not_flood_higher_land` (12 tests total, green).

## Issue 2 — fractional-height flowing water  ⚠️ NOT fixed (needs rendering work)
Root cause: the terrain write-back is voxel-only — `top = base + depth.round()`.
So a thin spread sheet (depth < 0.5) ROUNDS TO ZERO and vanishes, and water can
only ever exist as FULL-height cells. That's why placed water won't fan out into
half/quarter-height cells: the sim *does* compute fractional depth and spread it,
but the render throws the fraction away, so a thin film either disappears or stays
a blocky full cell.

Real fix = fractional-height water rendering. The column sim already carries a
per-column fractional `depth`; the mesher/surface shader needs to draw the TOP
water cell at a height = the fractional part (like Minecraft's 8 flow levels),
and the write-back should place a partial top cell (or tag it) instead of
`round()`. This is a mesher + surface-shader change (water surface geometry), not a
sim change — a focused follow-up. Until then, keep water in contained basins where
full-cell rendering looks correct.
