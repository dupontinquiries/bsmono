# Rule map — what breaks scenes, what doesn't (empirical, scored)

Built with `tools/sweep_matrix.py`: sweep one parameter across many scenes, render WORLD-ONLY, score each vs
SCENE_RULES.md, record score + failure note + weakest metric. This maps the higher-level RELATIONSHIPS — which
knobs are safe, which trigger failures, and how that depends on the scene — so tuning (and the Markov search) can
target the safe regions instead of re-discovering the same washouts. Matrices live in candidates/_matrix/*.csv.
(Absolute brightness is llvmpipe-confounded; the RELATIVE pattern across scenes is the reliable signal.)

## Relationship 1 — time of day (LOGSCOPE_HOUR) × scene  → see hour_heatmap.png
| scene | class | best | fails at | spread |
|---|---|---|---|---|
| Mansion Garden | FRAGILE | 22:00 | 7 (BLACK), 10, 13 (WASHOUT) | 0.87 |
| Reading Room | FRAGILE | 19:00 | 10, 13, 16 (WASHOUT) | 0.45 |
| Atlantis | FRAGILE | 7:00 | (none hard; not_blown dips midday) | 0.21 |
| Mistfall | ROBUST | 13:00 | none | 0.07 |
| Slate | ROBUST | 13:00 | none | 0.08 |
| Folding@home | ROBUST | 13:00 | none | 0.10 |

**Rule:** the breaker is *material brightness*. Bright-material scenes (Mansion marble + foliage, Reading Room
paper/marble + lamps, Atlantis water/caustics) WASH OUT in daylight (hour ~10–16) and Mansion goes fully BLACK at
dawn (hour 7, no self-illumination + low sun). They are safe only in the evening (≈18–23). Dark/matte scenes
(Mistfall stone+water, Slate stone, Folding membrane) don't blow out and are robust at every hour — they actually
peak at midday (more light → more visible structure). So: **bright scenes are time-of-day FRAGILE; dark scenes are
ROBUST.** Genuine bug (not just llvmpipe): Mansion at low sun → black. Everything else is a brightness calibration
best judged on the real GPU, but the *relative* fragility ordering is real.

## Relationship 2 — room scale (LOGSCOPE_TUNE_SCALE) × scene  (at hour 13)
| scene | scale 0.8 → 2.0 | effect |
|---|---|---|
| Mistfall | 0.728 → 0.726 | flat — scale-insensitive |
| Slate | 0.736 → 0.845 | IMPROVES with scale (more menhirs fill the bigger henge) |
| Folding@home | 0.855 → 0.872 | mild improvement |

**Rule:** scaling the room UP does **not** break these scenes — no void/coverage collapse — because the scatter
counts are tied to `map_radius()`/`ms`, so a bigger room spawns more content and stays full. Slate even benefits
(structure rises). This corrects the earlier assumption that bigger rooms go empty: that's only true where the
scatter does NOT track room size. So scale is a SAFE knob for the search to push on density-tracking scenes.

## What this means for tuning + the Markov search
- Constrain the search's HOUR range by scene class: FRAGILE scenes → evening only (avoid sampling washouts /
  blackouts); ROBUST scenes → full range. (Wired into markov_search.py via SCENE_HOUR_RANGE.)
- SCALE + DENSITY are safe-to-push knobs on density-tracking scenes (Slate/Folding/Mistfall) — let the chain raise
  them; they add structure without breaking coverage.
- Genuine fix worth making: an ambient floor so a scene can never render fully BLACK at low sun (Mansion@7).
- Next relationships to map: DENSITY × scene (where does clutter start hurting?), and camera PITCH × scene (how
  much does looking down change coverage/vspread?). Same tool, new `--param`.
