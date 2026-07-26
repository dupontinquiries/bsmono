# Bending visual FX (r688) — sim::bending_fx

Pure parameter core from `02_Bending_and_Disciplines/bending_shaders.docx`: the
exact render look of every bending effect, transcribed from the doc's tables.

- `Effect::{ArcCore, ArcPenumbra, MetalActivation, HeldChargeInterior,
  FailureCrack, SteamFlash, LightPathLines}` → `FxParams { color (HDR-linear),
  peak_emissive (×, pre-tonemap, needs FP16), bloom_radius_px (@1080p) }`.
  The **lightning arc** = ArcCore (6.0× pure white, 2px) inside ArcPenumbra
  (1.8× blue-white, 18px), with MetalActivation (2.8× blue, 25px) where it
  energises metal.
- `Discipline::{Stormstrike, HeavyBrine, ThermalPhase, Information}::core_fx()` →
  a tint for a charged held item / weapon of that discipline.
- `Quality::{Ultra,High,Medium,Low}` → sample counts / bloom scale (doc §4 LOD).
7 tests green standalone.

## Wiring seam (render — NOT done here; needs the HDR/bloom pass)
The renderer's emissive+bloom pass should draw each bending effect with
`Effect::params(e)` (colour → emissive target, `peak_emissive` into the FP16
framebuffer, `bloom_radius_px * Quality::bloom_scale()` for the blur kernel). The
existing arc GEOMETRY already exists (`lightning_path`, `bending_stormstrike`
routing); this supplies the missing COLOUR/intensity/bloom so the arc reads as the
doc's white-core/blue-penumbra plasma rather than a flat line. The held-item /
viewmodel renderer should tint a charged item by `Discipline::core_fx()`.
This is HDR-render-pass work — it can't be compiled or eyeballed in-sandbox, so it
wants a careful pass on a real build rather than a blind edit.

## NOT FOUND in the docs (need the user to point me to them)
The request also mentioned **"living weapon design effects"** and **"staffs for
building."** Neither term appears in `bending_shaders.docx` (which has the
lightning arc) nor, under those names, elsewhere in `docs/design_library`
(searched: lightning/living/staff/arc/sentient/weapon). `legendary_items_dragon_
artifacts.docx` covers imbued Tier I–III artifacts + Shrines (incl. an ⚡ Arc Node
Shrine) but not an animated "living weapon" look or a building-staff item. To wire
those faithfully I need the specific doc/section, or a description of the intended
effect — otherwise I'd be inventing rather than referencing documentation.
