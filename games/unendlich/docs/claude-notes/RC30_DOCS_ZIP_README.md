# Docs zip — companion to RC30-35

Adds two new sub-folders under `docs/`:

- `docs/claude-notes/` — Claude-authored notes (RC readmes,
  investigation memos, parallel-task tracker, **session handoff**)
- `docs/speculative/` — exploratory analyses not yet tied to a
  shipped RC

## Start here when opening the next session

**`docs/claude-notes/session-2026-05-17-handoff.md`** — the
consolidated "what shipped, what to test, what's next" guide.
Read this first.

## Contents

```
docs/
├── claude-notes/
│   ├── README.md
│   ├── session-2026-05-17-handoff.md                                ← START HERE next session
│   ├── PARALLEL_TASKS.md                                            ← live task tracker
│   ├── rc23-berry-bush-prep.md
│   ├── v0.24.64-submit-skip-readme.md                               ← RC30
│   ├── v0.24.65-slack-scheduler-readme.md                           ← RC31
│   ├── v0.24.66-67-biased-walk-and-upload-priority-readme.md        ← RC32
│   ├── v0.24.68-cave-gen-multi-scale-readme.md                      ← RC33
│   ├── v0.24.69-pressure-samples-no-alloc-readme.md                 ← RC34
│   └── v0.24.70-structural-dirty-readme.md                          ← RC35
└── speculative/
    ├── README.md
    ├── tick-sim-bottleneck.md                                       ← 3 findings; F1→RC34, F3→RC35
    ├── upload-scheduler-design.md                                   ← 5-option upload decoupling
    ├── stage-3-implementation-guide.md                              ← Stage 3 hand-off
    ├── compressed-chunk-representation.md                           ← subsurface culling
    └── cave-generation-and-erosion.md                               ← 4 steps; Step 1→RC33
```

## Apply

```bash
cd ~/Code/unendlich-development/unendlich-game
unzip -o ~/Code/unendlich-development/claude-zips/unendlich-rc30-docs.zip
```

Existing top-level `docs/` files are untouched.
