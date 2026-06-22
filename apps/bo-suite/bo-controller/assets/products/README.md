# Product SVGs

Soft, rounded product silhouettes that theme with the accent color. The
header badge (ProductSilhouette) loads `<productType>.svg` automatically
when present, fading it in with the reveal animation; if absent it falls
back to the hand-drawn Canvas shape.

## Files

Named by device `productType` (must match what `/BeoDevice` reports):

  BeosoundStage.svg     soundbar
  BeosoundBalance.svg   tall column speaker
  BeosoundLevel.svg     portable slab
  BeosoundA5.svg        disc
  BeosoundA9.svg        disc
  BeoplayA9.svg         disc
  BeosoundEmerge.svg    book-spine speaker
  BeovisionEclipse.svg  TV
  BeovisionContour.svg  TV
  BeoplayH100.svg       headphones
  BeoplayHX.svg         headphones
  BeoGrace.svg          earbuds case

## Base templates

The `_*.svg` files (underscore prefix) are the reusable base shapes:
`_earbuds _soundbar _speakers _headphones _disc _column _tablet _spine
_screen`. Each product SVG above is a copy of the matching base. To
restyle a whole form factor, edit the base and re-copy, or just edit the
product file directly.

## Conventions

- 64×64 viewBox, fit into the 50×50 badge with ~7px padding.
- `stroke="currentColor"` / `fill="currentColor"` so the art inherits the
  product accent (the badge sets the color). Keep this — don't hardcode
  colors, or theming + nighttime-warming won't apply.
- Rounded line caps/joins throughout for the soft B&O feel.
- Loaded from disk at runtime (same search path as products.json), NOT
  compile-embedded — so the drawn fallback covers standalone-binary runs.
