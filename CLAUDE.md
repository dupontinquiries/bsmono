# CLAUDE.md — directives for an agent working in this repo

Read this before running anything. It is short on purpose. Every rule here cost
a session to learn, and most of them present as something other than what they
are.

Orientation, not rules: [NEWCOMER_DEVELOPER.md](NEWCOMER_DEVELOPER.md) for the
build system, [LAYOUT.md](LAYOUT.md) for the taxonomy,
[docs/agents/INDEX.md](docs/agents/INDEX.md) for the doc catalog,
[maintenance/C9_TODO.md](maintenance/C9_TODO.md) for open threads.

---

## 1. Start every session by arming capture

```sh
export CAPTURE_LOG=/tmp/session-capture-$USER.jsonl   # a FIXED path, see below
source maintenance/tools/session_capture.sh
```

**Pin `CAPTURE_LOG` yourself.** The default is `${TMPDIR}/session-capture-$$.jsonl`
and `$$` is the sourcing shell's pid. An agent harness runs every command in a
*fresh shell*, so the default opens a new log per invocation, the post-commit
mark never matches, and each commit gets only its own invocation's commands.
That is visible in this repo's own history: some notes are two lines long.

If a clone has never had the hook installed:
`./maintenance/tools/session_capture.sh --install`

Notes do **not** travel with `git clone`. Fetch them explicitly:
`git fetch origin 'refs/notes/toolcalls:refs/notes/toolcalls'`

Full rationale: [maintenance/SKILL_session_capture.md](maintenance/SKILL_session_capture.md).

## 2. Build with `cargo ailite`, one crate at a time

```sh
maintenance/tools/cargo-ailite check          # not `cargo check`
```

Measured on `shared/crypto-kit`: target directory **71 MB → 41 MB**, cold
10.6 s → 9.7 s. The 42% is the point. **Disk is what fails in this sandbox, not
time** — see [maintenance/SANDBOX_BUILD_LIMITS.md](maintenance/SANDBOX_BUILD_LIMITS.md),
where the same file records that a full disk presents as an LLD crash, a
fifteen-minute hang with no process running, or a build that dies at 5.2 GB
free.

- **One crate, one job.** `CARGO_BUILD_JOBS=1 cargo test -p <crate>`. Parallel
  rustc on 1 core / 3 GB is killed silently. `cargo test --workspace` is fine
  on real hardware and not here.
- **`AILITE_INCREMENTAL=1`** only for a tight edit-check loop; the default off
  is right for build-once-and-move-on.
- **Delete `CARGO_TARGET_DIR`s when done, and watch `df -h /`.** Not optional.
- Save the artifacts instead of losing them:
  `maintenance/tools/ailite-cache save cache.tar /tmp/bd`, then `restore` in the
  next session — verified 8830 ms cold → **39 ms, 0 units compiled**. Restore
  must land at the same absolute path.
- `cargo ailite gate` is for a COLD graph. On a warm tree it measured the same
  as a plain workspace check (340 / 307 / 295 ms). Do not expect it to speed up
  a warm loop.

## 3. Native dependencies block whole crates — use the feature gates

Missing here: `libasound2-dev`, `libssl-dev`, `cmake`, Qt6, GTK4/libadwaita,
any Vulkan ICD, `/dev/dri`. That means a build script fails and takes the whole
crate with it, including headless code that never touches the device.

Known escapes, all default-on so ordinary builds are unaffected:

| crate | flag | blocked by |
|---|---|---|
| `apps/beatspace` | `--no-default-features` | cpal→alsa-sys, opus→audiopus_sys(cmake) |
| `games/blackstar-rs` | `--no-default-features` | cpal→alsa-sys |
| `apps/studio` `studio-media` | `--no-default-features` | opus→libopus via cmake |
| `system/snapclone` `snapclone-crypto` | `SNAPCLONE_SKIP_CBINDGEN=1` | see §4 |

If you meet a new one, **add a default-on feature** rather than working around
it in the shell. Three crates already do this; it is the house pattern.

## 4. Check `git status` after every build

Some build scripts write into the source tree. `snapclone-crypto`'s used to
regenerate a hand-maintained committed header on every build, silently
replacing the `CryptoError_*` alias block the C++ side includes. Fixed, but the
habit is the real defence: a dirty tree after a *read-only-looking* command is
a bug, not noise.

## 5. Verify claims against code, not documentation

This repo's docs go stale in a specific, expensive way: they assert engine
state that has since changed, confidently.

Found in one session: two logscope design notes still described a loader that
takes one architecture, when nine load — **and I recommended work that was
already finished on the strength of them.** A `snapclone-nat` doctest called a
method that does not exist. `LAUNCHER_MERGE.md` had already decided a question
I spent an hour re-deriving.

So: **before acting on a doc's conclusion, check the code it describes.** And
when you write a claim about engine state, pair it with a test that fails when
it stops being true — `archcfg.rs`'s `note_claims` module is the pattern, and
it now covers markdown as well as registry strings.

## 6. Never trust a bundle's file size

A self-contained bundle carries its whole history, so its size is the
*sender's* repo size. Measure what it would actually cost:

```sh
git init --bare /tmp/probe.git
echo /path/to/repo/.git/objects > /tmp/probe.git/objects/info/alternates
cd /tmp/probe.git && git fetch <bundle> 'refs/heads/*:refs/heads/*'
git repack -a -d -l          # --local excludes what the alternate supplies
```

Measured: a 369 MB bundle cost **888 KiB**; a 459 MB one cost 256 MB.

**A bundle that forked BEFORE the history rewrite cannot be merged, only
harvested.** Its asset history is the purge running backwards — merging it
resurrects exactly what `filter-repo` removed. Two of five bundles were in this
state; both were extracted with pathspec-restricted `format-patch` + `git am`,
which keeps the original authorship.

Shas cannot answer "what is new" across a rewrite — the same commit exists on
both sides under different hashes. Compare commit *subjects*.

Budget: 500 MiB, currently ~370. `maintenance/REPO_SIZE.md`, but note it
measured a pre-rewrite clone.

## 7. Run the checkers before committing

```sh
./verify.sh                                # rustcheck/peek/drift over the tree
python3 maintenance/tools/lock_check.py    # committed locks vs their manifests
python3 maintenance/tools/orphan_check.py  # .rs files no `mod` declaration reaches
python3 maintenance/tools/archive_check.py # maintenance/archive/ provenance headers
```

`lock_check.py` exists because three stale lockfiles were found by tripping
over them. A stale lock works perfectly online — cargo resolves the missing
name and rewrites the file — and fails offline, on a clean clone, or in CI.

`orphan_check.py` exists because a `.rs` file with no `mod` declaration is not
compiled, so its tests never run and nothing in the build can contradict it.
Nineteen were found this way, carrying 79 tests; two documents had drifted into
disagreeing about whether one of them existed.

## 7b. Removing a module: archive it, do not just delete it

When a module is superseded, put it in `maintenance/archive/` with the
five-field provenance header `archive_check.py` enforces — see
[maintenance/archive/README.md](maintenance/archive/README.md). Git has the
file either way; what git does not have is the REASON, at a path someone will
grep. A deleted module is findable only by someone who already suspects it
existed.

Do **not** leave it undeclared in `src/` as a compromise. That is the worst of
both: not compiled, not discoverable as archived, and still reading like the
real implementation.

`LAST-KNOWN-GOOD` is the field that matters and the easiest to write
dishonestly. Record what was CONFIRMED, when, and by what — and keep "its tests
passed" distinct from "it ran against real data", because the archive is
exactly where that difference gets lost.

## 8. Commits

- Small, and one concern each. An import and the manifest registration that
  wires it in are **two** commits — `BRANCH_INDEX.md`: "an import that also
  rewires the build makes it impossible to tell later which changes came from
  upstream and which were integration choices."
- Say what was verified and what was not, in the message. "Unverified by
  compilation" is an acceptable state here — GPU, Qt and GTK paths cannot be
  built in this sandbox — but it has to be *said*, or the next session inherits
  a claim it cannot audit.
- Write long messages to a file and use `git commit -F`. Heredocs through a
  `sh -c` wrapper mangle apostrophes and silently truncate.

## 9. Name output bundles `c9-NNN-<summary>.bundle`

```sh
maintenance/tools/export-bundle.sh <summary-slug>
```

NNN auto-increments from the highest already in the output directory. It does
three things by hand otherwise forgotten: `--all` with **no implied base** (an
incremental bundle dies the moment history is rewritten — see
`maintenance/INCOMING_BUNDLE_REBASE.md`, where a prerequisite hash stopped
existing and 81 deltas became unrecoverable); **appends
`refs/notes/toolcalls` explicitly**, because `--all` does not include it and a
clone without it shows every `observed_calls` as null, indistinguishable from a
repo where capture was never on; and refuses a dirty tree or an over-budget
result rather than shipping something you then have to diagnose.
