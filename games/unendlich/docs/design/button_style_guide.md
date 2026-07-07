# Button style guide

Status: r317. The button toolkit lives in `crates/unendlich-ui/src/widget.rs` as a
layered "sandwich" (egui base → unify → button work → toolkit abstraction → toolkit
language). This guide documents the button *classes* we kept after the slider-harness
exploration (r314–r316) and how to use them.

## The layered model (how a button is built)

| layer | what it does | where |
|---|---|---|
| egui base | raw egui allocate / `Sense` / `Painter` | inside layer 2 + the paint pass |
| unify | `interact(ui, size) -> Interaction { rect, resp, hover, down }` — one normalized interaction for any clickable widget | `widget::interact` |
| button work | `geometry()` (sizing/min) + `label_spaced()` (uppercase, thin-space casing) | `widget` |
| abstract | `ButtonSkin { radius, fill, left_bar, border, dither, ink, center, bump }` — a *declarative* look in toolkit terms; no egui drawing | `widget::ButtonSkin` |
| toolkit language | `candidate(i)` (the named skins) + `paint()` (renders any skin) | `widget` |

Adding or tweaking a look = editing a declarative `ButtonSkin` in `candidate()`. No egui
plumbing.

## The kept classes

### Special — the default / main-menu button  (skin 1)
The everyday button. A dark panel with a **left accent bar that grows on hover**, an
uppercase letter-spaced left-aligned label, and a dither that **reveals left→right on
hover**. This is the default (`button_style()` starts at 1), so `unendlich_ui::button`
and the front/title menu render as this.

- Use: `unendlich_ui::button(ui, label, accent)` / `button_sized(ui, label, accent, min)`.
- Engine shims: `tbtn` / `tbtn_sized` (pull accent from the active theme).

### Header  (skin 0)
The quietest bar button — panel + growing left bar, **no dither**. Reads as a header /
section-lead action. Same shape as `dest_button`. Select via the debug slider (style 0)
when you want the calm version.

### Elegant — the enchantment / magical button  (skin 6)
Transparent, hairline-framed, with **faint static top/bottom dither edges** and a soft
dither **infill that follows the cursor** and eases in smoothly with hover (the band is
centered on the pointer x; its width is driven by `animate_bool`, so it tracks the mouse
elegantly). Centered label, no hover bump. Use it for special / magical actions.

- Use: `unendlich_ui::elegant_button(ui, label)` / `elegant_button_sized(ui, label, min)`
  (accent comes from the theme automatically).
- Applied: the **Settings → Controls → rebind-key** button.

### Pulse (experimental — skins 2/3/4)
Center-aligned with a center bump and a random-walk *pulsing* dither fill (time-animated,
decorrelated per button). Three flavors: 2 filled, 3 ghost (accent ink), 4 framed. Kept
on the slider for now; not yet assigned to a surface.

## Removed
Skins 5 (quiet underline), 7 (clean ghost), 8 (header + corners), 9 (duplicate) were cut.
Those slider positions now fall back to the default (special). The **corner-dither**
treatment (old 5/8) is worth reusing on *panels*, not buttons.

## Tokens & feel
- Label: uppercase, thin-space (`\u2009`) letter-spaced, `FontId::proportional(13)`.
- Accent bar: base 3px, grows +9px on hover; alpha 0.7→1.0.
- Hover is always the eased `animate_bool` value; press darkens fills to ~0.85.
- Colors come from the active `Theme` (`bg/panel/panel2/line/ink/accent/...`), never
  hard-coded, so a button is consistent on the dashboard and frosted over the world.

## The live harness
A debug-menu slider ("button style 0–9") drives `set_button_style` / `button_style`, so
any candidate-driven button repaints live for A/B. Picking a class = noting its number;
assigning it = calling the matching toolkit fn (or, for the global default, the slider /
`set_button_style`).

## Next
Extend the same `interact + skin + paint + slider` pattern to TOGGLES, then SLIDERS, then
the PANEL / GLASS surface (where the blur-backdrop + glass-bulge work folds in).
