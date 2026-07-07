# Menu Mockup — Testing Playground Demo

> Part of the menu/sim work — see `MENU_SYSTEM.md` for the overview + status.


A standalone, dependency-free HTML playground for iterating on the
**unendlich** menu aesthetic in a browser, decoupled from the engine build
cycle. Open `menu_mockup.html` directly in any browser — it is fully
self-contained (fonts embedded, no external assets, no network).

This is a design/testing artifact only — not shipped, not wired into the
build. It exists to dial in the *look and motion* before porting the
settled design into the real egui menu renderer.

---

## Versions

Snapshots are frozen as `menu_mockup_vN.html`; `menu_mockup.html` is always
the live working copy.

- `menu_mockup_v1.html` — cone-parallax, deterministic content tint, FLIP
  morph transitions, first glass×tactical Character merge.
- `menu_mockup_v1.2.html` — diagonal-rect toggle fill, centred scale-in,
  transparent item cells, edge-weighted glass + border-fill.
- `menu_mockup_v1.3.html` — blocky touch-sliders, diamond toggle, custom
  TTF fonts, temporal-history morph fix, crosshair inventory, decoupled
  history-diamond hover.
- `menu_mockup_v2.html` — fonts EMBEDDED as data URIs (always load); SVG
  stroke-morph text entry; history screenshot panes; toggle-language
  sliders; blocky terminal caret; diamond-shrink-wrap toggle.
- `menu_mockup_v3.html` — thinner/more-legible font; CUSTOM slider (fill
  wraps a SQUARE knob, push/pull stretch, grayscale-faded right side);
  thin world-name stroke. **Current.**

---

## What it demonstrates (v3)

**Layout & depth**
- **Cone-projection parallax** — the cursor is the apex of a viewing cone;
  layers at fixed depth ratios (`voidZ`, `glassZ`) project from that one
  anchor, so the parallax is coherent by construction. Smoothing lags the
  anchor for weight. Tunable live via the PARALLAX CONE panel.
- **Frosted glass** panels with an **edge-weighted** inner glow (thicker at
  the rim than the centre) and a **global specular** gleam: the biggest
  element on screen is treated as the scene's light origin and a tinted
  highlight rakes across every glass border.
- **Deterministic content tint** — an FNV-1a hash of (page name + slider
  values) drives a "colour-max" accent + page wash, so the look is
  reproducible and the sliders feed it.

**Transitions**
- **Temporal-history FLIP morph** — on navigation the new panel grows out
  of the nearest *recent* box position. Box centres are recorded only while
  a panel is at rest (untransformed), in a 10-second ring buffer, so the
  morph never reads a mid-animation (shrunken) rect. Centre origin, no
  corner pin.
- **Border-fill** — an inset ring animates outer->inner on each navigation
  (approximate in CSS; flagged for a clean egui port).

**Widgets**
- **Custom slider** — the fractal/multi-octave dither FILL wraps a SQUARE
  knob (ends exactly at it); the unfilled right side is a dim hairline with
  the accent faded to grayscale; a push/pull stretch fires on grab; values
  lerp to evenly-spaced detents on release.
- **Toggle** — a `<...>` holder whose 45 degree caps shrink-wrap the diamond
  knob; the infill is a fractal dither filling the whole holder, revealed
  left->right from behind the knob; squish + diamond-bloom on switch.
- **Rune-morph text entry** — typed characters render as SVG line-segment
  strokes; each starts as a bending-rune skeleton and MORPHS into the Latin
  letter by resampling both to a common point count and interpolating
  endpoints per frame (stroke transform/transpose — no blur). Blocky
  terminal caret. (Stroke weight thinned in v3 for legibility.)
- **Inventory/crafting cells** — neutral hollow borders + a sharp inner
  hairline (pixel-art x sharp-edge), clear interiors, no hover-zoom; a
  **crosshair** lights the hovered cell's row+column, propagating outward
  (delay proportional to manhattan distance).
- **History timeline** — each point carries a **screenshot pane** (pixelated
  noise placeholder) above a diamond marker; on hover the inner diamond
  expands slightly while a decoupled border ring expands more and sharpens.

**Glass x tactical Character sheet** — the frosted-glass depth recoloured to
the green/gold tactical reference (RootBender): a gear column (corner-
bracketed slots), profile (gold POWER + diamond stat glyphs), voxel-
character stage, companions, bond pips, resonance hex — on a globally-
aligned 4-column grid.

---

## Fonts

`fonts/build_fonts.py` (fontTools) generates two TTFs from the design
language and the mockup **embeds them as base64 data URIs** (so they load
regardless of where the file is opened — a relative-url version silently
failed when the .ttf wasn't colocated):

- **UnendlichBlocks** — a blocky/voxel Latin game font (5x7 pixel grid,
  51 glyphs). Each filled cell is a slightly inset square, so strokes are
  thin and pixels read separately (legible pixel-art). Used for accent
  headings + nav; body text stays on the system sans.
- **UnendlichGlyphs** — UI/bending marks: filled+outline diamond, `<`/`>`
  chevrons, `[` bracket, rune mark, resonance spark, leaf tick, gem
  cluster.

Rebuild: `python3 docs/design/fonts/build_fonts.py docs/design` (then the
mockup must re-embed the base64 — see the build note in WIRING_STATUS).

---

## Porting to the engine (egui)

Once the look is final, carry into the egui menu renderer:
- the two TTFs -> `FontDefinitions` (highest-impact single step);
- cone z-ratios + smoothing -> menu bg/panel offsets (resolution-independent);
- the slider (fill-wraps-knob + grayscale-faded tail + stretch), the toggle
  (45-cap diamond shrink-wrap + dither), and the crosshair (grid dims known
  at layout -> delay = distance x dt) -> painter draws;
- the temporal-history morph -> a rolling buffer of widget rects;
- the border-fill -> a true outer->inner gradient border with animated inner
  radius (do it properly in the painter rather than the CSS approximation);
- the stroke-morph text entry -> path interpolation in the painter.
