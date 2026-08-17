# Two launchers arrived, and only one can be the launcher

Two incoming branches both add a fullscreen Plasma app launcher — an egui
shell plus a QML plasmoid — and both were written on 2026-08-16 from the same
base commit (`71163e4e`). Merging both would leave the monorepo shipping two
launchers that do the same job, register separate manifest entries, and each
claim `primary = true` for their own port.

Both are real work and both pass their tests on this host:

| | `incoming/launcher` → `apps/launcher` | `incoming/hatch2` → `apps/hatch` |
|---|---|---|
| commits | 5 | 10 |
| insertions | 9,168 | 9,602 |
| tests here | 36 (egui-frontend) | 42 core + 24 egui = 66 |
| headless core | none — `desktop.rs`/`icontheme.rs` live in the frontend | `crates/hatch-core`, std-only |
| plasmoid | `plasmoid/` + config UI | `qml/` + `packaging/` |

Neither dominates, which is why this is written down instead of decided by
whoever merged first.

## What each one has that the other does not

**`launcher` has the presentation work.**

* `icontheme.rs` — real freedesktop lookup: `index.theme` parsing, directory
  size-matching, the `Inherits` chain, hicolor fallback. Nine tests run
  against this build host's actual Adwaita and hicolor trees.
* `anim.rs` — a headless tick-based `Board` driving slide-in and restack, so
  filtering and restack convergence run under `cargo test` with no display.
  This is the animation system the `fullscreen-launcher` QML docs describe
  (eight icon styles, tile-geometry slide-in patterns).
* SVG rasterized at the tile's on-screen size via resvg, rather than at the
  icon's often-tiny intrinsic viewBox.
* A plasmoid config surface — `main.xml` plus `ConfigAppearance.qml` — which
  is what makes the animation styles user-settable.

**`hatch` has the architecture.**

* A separate std-only `hatch-core` crate: `.desktop` scan, `Exec` spawn, icon
  resolve, fuzzy rank, favorites and recents through config-kit. This is the
  headless-core-then-shell seam the rest of the monorepo uses, and it is why
  hatch has 42 tests that need no egui at all.
* Single-instance with a poke-file toggle (`hatch-egui --toggle`), which a
  launcher bound to Meta actually needs.
* A resident process that notices installs — directory stamps and the cache
  they invalidate.
* Node tests for the plasmoid's `tools.js` and slide geometry: the QML logic
  tested off the phone.
* Favorites seeded from Plasma rather than synced to it — a deliberate call,
  documented in its commit.

## Recommendation

Take **hatch as the base** and port launcher's presentation work onto it.

The reason is not test count. It is that hatch's split matches what this
monorepo is for: a headless core that runs under `cargo test` with no
display, and shells over it. `launcher` puts `.desktop` parsing and icon
theming inside the egui frontend, so a second shell — the plasmoid — cannot
reuse any of it, which is the exact problem the fleet-wide port strategy
exists to avoid.

Ported in this order, each step independently testable:

1. ~~`launcher/icontheme.rs` → `hatch-core/icons.rs`.~~ **Done.** Ported whole
   with its nine tests, as `hatch-core/src/icontheme.rs`, and `icons::resolve`
   now tries it first. The grid walk stays behind it as the fallback for
   themes that ship no `index.theme` — the spec implementation cannot resolve
   those at all, so replacing rather than layering would have lost a case.
   hatch-core: 42 → 53 tests.
2. `launcher/anim.rs` (the `Board`) → `hatch-core`, or a sibling crate if it
   pulls anything egui-shaped. It is already headless and tick-based, so it
   belongs beside the model rather than in a shell.
3. The plasmoid config surface — `main.xml` and `ConfigAppearance.qml` — onto
   hatch's `qml/`, so the animation settings have somewhere to live.
4. `apps/launcher` is then deleted, and its manifest entries with it. One
   launcher, one `primary = true`.

## Do not

Merge both branches and rename one. The monorepo would carry two `.desktop`
scanners, two icon-theme resolvers and two plasmoids, and the next person to
touch a launcher bug would have to find out which one is installed before
they could reproduce it.

## Status

**hatch is merged** (`8444e17b`), clean, 42 core + 24 egui tests passing on
the merged tree. Step 1 of the port is done. Steps 2-4 — the animation Board,
the plasmoid config surface, and deleting `apps/launcher` — remain.

`incoming/launcher` stays as a ref until they are, because it is still the
only copy of that work:

```
git log incoming/launcher     # apps/launcher
git log incoming/hatch2       # apps/hatch
```

Neither is merged into `main`. The base commit both fork from is
`71163e4e`, which `main` is well ahead of, so whichever lands will be a real
merge rather than a fast-forward.

## Also in the same upload, not yet assessed

* `fullscreen-launcher/` — QML sources and two design docs (icon animations,
  tile geometry) for the Plasma plasmoid. Source material for step 3 above
  rather than a separate thing to merge.
* `studio-trap/` — a DAW, `apps/studio`, and it DOES fetch here. I wrote the
  opposite in this file's first commit: `git bundle verify` printed a base ref
  and I read "requires" as "is missing" without checking, when
  `git cat-file -t` on that ref answers `commit` in one command. Corrected
  the same day, but the lesson is that "requires" in that output names a
  prerequisite, not a failure.

  Three bundles, one lineage: `studio-trap-work` (6 commits) is an ancestor
  of `autotune-bundle-02` (23), and `studio-session.bundle` inside the v2 zip
  has the identical tip `f47ca49e` — a duplicate, not a third branch. All
  three fork from `3e6b64d9`, a commit this repo has.

  Fetched as `incoming/trap`, `incoming/trap-autotune`, `incoming/trap-session`.
  The tip adds twelve crates under `apps/studio/crates` — core, engine, audio,
  fx, io, live, media, command, collab, copilot, app, egui — so it is a
  separate product from the launchers, not a competing implementation, and
  merging it is an independent decision from the one above. Its own
  assessment still to do.

* `studio-session.mbox` — an mbox alongside the bundles, unexamined.
