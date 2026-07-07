# unendlich toolkit — brick launcher + analytics dashboard

Two tools:

1. **`apply_and_test.sh`** — the launcher. Extracts every brick zip in
   dependency order, tags each in git, runs its tests, and logs which tag
   applied and passed.
2. **`unendlich-dash/`** — the native egui telemetry/analytics dashboard.

## 1. Launcher: apply_and_test.sh

Put all the downloaded `unendlich-*-flat.zip` files in one folder, then:

```bash
# defaults: ZIPS_DIR=~/Code/unendlich-development/claude-zips
#           REPO=~/Code/unendlich-development/unendlich-game
#           BRANCH=rc29-adaptive-uploads  FEATURES=render,gpu
./apply_and_test.sh
```

For each brick it: unzips into the repo, `git commit` + `git tag brick/<tag>`,
runs `cargo test --release --lib -p unendlich-engine --features render,gpu <filter>`,
and records PASS/FAIL. Bricks are applied in dependency order so every
intermediate state compiles. A missing zip is skipped, not fatal.

Output goes to `brick_apply_log.txt` in the repo, framed with
`==== VERSION <tag> START/END ====` markers — the same markers the dashboard
segments on, so you can open that log in the dashboard and see per-tag results
("what tag is applying").

Useful overrides:

```bash
GIT_SYNC=0 ./apply_and_test.sh        # apply + test, no git at all
GIT_PUSH=0 ./apply_and_test.sh        # commit + tag locally, don't push
FEATURES=render ./apply_and_test.sh   # narrower feature set
ZIPS_DIR=~/Downloads ./apply_and_test.sh
```

### git sync

`apply_and_test.sh` pulls before and pushes after by default. To do it by hand
instead (with `GIT_SYNC=0`), use:

```bash
./git_sync.sh pull     # git fetch + pull --rebase origin <branch>
./apply_and_test.sh    # (with GIT_SYNC=0)
./git_sync.sh push     # git push origin <branch> + --tags
```

Each brick becomes a commit and a `brick/<tag>` tag, so the remote history shows
exactly which tag introduced what.

## 2. Analytics: unendlich-dash

```bash
cd unendlich-dash
cargo run --release
```

Then **Load sample** for the bundled 2-run demo, or point the path fields at a
real `run_summaries.txt` + `events.jsonl` (or drag-drop them). You can also load
`brick_apply_log.txt` from the launcher run, since it shares the VERSION framing.

Pinned to egui 0.34 / eframe 0.34 / egui_plot 0.35 (MSRV 1.92). Linux deps and a
wgpu→glow fallback are noted in `unendlich-dash/README.md`. It's compile-blind
code; if the egui API drifted, paste the first `cargo` errors and they're quick
fixes.
