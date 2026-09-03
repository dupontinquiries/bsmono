# Merge guide — c14unified-026

96 commits on top of `3f1e9e5a`. Written for whoever merges this
elsewhere: what is in it, what depends on what, and which parts carry a
decision rather than a fix.

## Applying

```sh
git bundle verify c14unified-026-panes-collab-and-terminal-input.bundle
git pull  c14unified-026-panes-collab-and-terminal-input.bundle main
```

The self-contained bundle records a complete history and needs no
particular base. An incremental one is also shipped (prerequisite
`3f1e9e5a`, ~250 KB) — convenient, and the one `maintenance/` warns
about, because its prerequisite stops existing if history is ever
rewritten.

## Streams, in dependency order

**1. `shared/pane-kit` (new crate, 38 tests).** The binary layout tree
lifted out of voiceterm's `terminal_workspace`, generic over a payload.
Nothing else moves until this lands. Fixes a bug the original had:
`close_pane_id` left `focused_pane` dangling on non-active tabs.

**2. `apps/voiceterm` + `shared/uikit`.** `terminal_workspace` migrated
onto pane-kit (1386 → 891 lines, all 32 original tests unchanged), plus
`uikit-term` gaining `InputOwner`, `is_widget_chord`, `clears_the_line`
and a `col` on `Selection`.

**3. `apps/aperture`.** `aperture-core::panes` (17 tests) and the new
`aperture-collab` crate (47 tests, RGA CRDT + presence + wire format).
**Built and tested, wired into nothing** — see
`apps/aperture/TODO_panes_and_collab.md` for the measured cost of
wiring it (230 `.editor` sites, and the borrow-splitting is the work,
not the renames).

**4. `apps/voiceterm/egui-frontend`.** The largest stream: seven
input-path bugs, then the discovery that two grids existed and only one
was drawn, then their consolidation and the deletion of the second VT
parser (1,478 lines, 60 tests). All verified on a running app.

**5. `games/road-warrior`.** Terrain rendering, baked ray-traced
lighting, materials, a fixed-timestep accumulator, and an out-of-fuel
softlock fix. See `games/road-warrior/docs/RENDERING.md`.

**6. `games/blackstar-rs`.** Example harnesses repaired (they had not
compiled since the animation work landed, so `cargo test` failed before
running any of 1497 tests), non-finite input guards, and `fold_audit`.

Streams 4–6 are independent of each other and of 1–3.

## Things that are decisions, not fixes

- **`term_grid` and `row_render` are deleted** from voiceterm-egui
  (recoverable at `3b45c851`). If you want the frontend's own VT parser
  back, that is where it is.
- **The blackstar asset question is open.** `fold_audit` says the fleet
  shows a median 62% of its robot in vehicle mode, and that brute-force
  optimisation of the right objective buys seven points. The conclusion
  is that the gap is in the meshes, not the loader. Nothing was changed
  in response.
- **`car_profiles.json` is checked in and unused.** Side-view
  silhouettes extracted from blackstar's car models for road-warrior;
  wiring them replaces the procedural box car, which is an art-direction
  call.
- **Aperture panes/collab wiring**, as above.

## Cross-cutting documents added

| doc | what it is for |
|---|---|
| `docs/agents/HEADLESS_GUI_TESTING.md` | building, running, driving and screenshotting a GUI app here — and the offscreen path that needs no display |
| `games/blackstar-rs/docs/NON_FINITE_INPUTS.md` | a survey of NaN/degenerate-input handling across nine modules, five of which had defects |
| `games/road-warrior/docs/RENDERING.md` | why the lighting is baked and the assumption that makes it valid |
| `games/road-warrior/BACKLOG.md` | menus, LAN via bsgames, procedural maps, with the constraints already known |
| `apps/aperture/TODO_panes_and_collab.md` | the measured cost of wiring panes and collab |
| `CLAUDE.md` §8 | two verification rules, both learned by getting them wrong here |

## Two rules worth carrying over

Both are in `CLAUDE.md` and both cost real time here.

**`cargo test --lib` is not the test suite.** It skips `tests/`. A
signature change sat broken in an integration target for fifteen commits
while every run reported green.

**"Nothing references this" is a claim to grep for across the whole
repo.** Two of three modules called unreferenced in one commit had
callers — one reached by `#[path]`, invisible to any search for `use`.

## What was verified how

Most of this session's claims were checked on running software rather
than by reading: a real shell in a real terminal window, a car driving
over a rendered ramp, and offscreen renders of 254 chassis. Several
confident conclusions were wrong until something was looked at. Where a
claim is argued rather than observed, the commit message says so.
