# echelon-m0-02 cannot be applied — it is an incremental bundle

Recorded rather than half-applied, because a bundle that will not open is easy
to mistake for one that was merged.

## What it is

```
# v2 git bundle
-8047523f14ee64d68a881bb510e01fd9297d29fe feat(ui): hour dial — drag the scene's
                                          time of day (physui Dial), fix button overlap
f470d292e8cfd7d2c8d4531d870e005adecd231b refs/heads/echelon/m0
```

The leading `-` marks a **prerequisite**: this bundle carries only the objects
added since `8047523f`, and that commit is not in this repository. `git fetch`
and `git bundle verify` both refuse it, correctly — applying a delta against a
base you do not have would produce a tree nobody has ever seen.

Nothing named `echelon` exists here: no refs, no directory, nothing in `apps/`
or `games/`. So this is not a case of the base being present under another name.
The 278KB size is consistent with a delta rather than a repository.

## What it contains, read out of the packfile anyway

The pack could not be indexed (29 unresolved deltas, as expected without the
base) but `git unpack-objects` recovered 280 loose objects, enough to read the
commit graph. **11 commits**, most recent first:

```
f470d29  2026-08-08  feat(cadscope,echelon): Mark II models, sprite atlas rendering,
                     animation pipeline
3d3045d  2026-08-02  feat(bsgames): cross-game tournament host; Echelon Invitational I
03ef8c4  2026-08-02  feat(echelon): honest wheel + champion-seatable Deck
e88cda1  2026-08-02  feat(echelon): Phase 2 — serve protocol, Command Deck,
                     Proving Grounds league
0f7952f              feat(echelon): Phase 2 — Rust Command Deck, fog/hold/rally
adc5ac4              feat(echelon,cadscope): decisive bots, engine placement rules,
                     roster sheets, air crowding
404abbd              style(editor-kit): faction-themed consoles replace the paperbound skin
32e5777              feat(echelon,cadscope,editor-kit): pseudo-ground air, ability engine,
                     editor kit, CADscope seed
da4b4e7              feat(echelon): 9-level vertical model — 6 terrain levels, 3 air layers
e3ff752              feat(echelon): heights/ramps, authored maps, tileset palettes
4efc699              feat(echelon): M1 Bevy client + shared echelon-bots crate
```

Trees are unreadable without the base, so the file-level contents are not
recoverable from this bundle alone — only the messages above.

## What it touches, against what is here

| project | in this monorepo |
|---|---|
| `echelon` | no |
| `cadscope` | no |
| `editor-kit` | no |
| `physui` | no |
| `bsgames` | **yes** — `games/bsgames` |

So the delta spans at least four projects that have never been here, plus one
that has. `bsgames` is the only place a merge could even be attempted, and doing
that from an unopenable bundle is not possible.

## What is needed

Either a **full** bundle (`git bundle create … --all`, which carries its own
history and has no prerequisites), or the earlier `echelon-m0-01` that contains
`8047523f`. With the base present this one applies in a single fetch.

Worth noting for future exports: `--all` on a repo this size costs more bytes
and removes an entire class of "cannot apply" — this port's own bundles are
built that way for exactly this reason, and verified on the copy every time.
