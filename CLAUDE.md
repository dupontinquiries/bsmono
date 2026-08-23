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
source maintenance/tools/session_capture.sh
```

The default log is keyed by (user, repo) — any shell sourcing it in the same
repo appends to the same log, so a harness that runs every command in a fresh
`bash -c` still produces complete notes, and a shell's final command is
recovered from a sidecar file at the next source or commit (with `exit:null`,
since its status died with the shell). Pinning `CAPTURE_LOG` to a fixed path
still works and still wins, but is no longer required. The recorder is
bash-only; if the harness shell is `dash` or another `sh`, wrap the work:
`bash -c 'source maintenance/tools/session_capture.sh && <commands>'`.

`./maintenance/tools/session_capture.sh --selftest` simulates exactly that
fresh-shell harness against a scratch repo and must pass; run it if notes
ever look thin.

If a clone has never had the hook installed (or was installed before the
sidecar mechanism existed — the hook is generated, rerun is idempotent):
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

Missing here: `libasound2-dev`, `libssl-dev`, `cmake`, `libudev-dev`, Qt6,
GTK4/libadwaita, any Vulkan ICD, `/dev/dri`. That means a build script fails and takes the whole
crate with it, including headless code that never touches the device.

Known escapes, all default-on so ordinary builds are unaffected:

| crate | flag | blocked by |
|---|---|---|
| `apps/beatspace` | `--no-default-features` | cpal→alsa-sys, opus→audiopus_sys(cmake) |
| `games/blackstar-rs` | `--no-default-features` | cpal→alsa-sys |
| `apps/studio` `studio-media` | `--no-default-features` | opus→libopus via cmake |
| `system/snapclone` `snapclone-crypto` | `SNAPCLONE_SKIP_CBINDGEN=1` | see §4 |
| `games/blackstar-rs` `gamepad` | already off by default | gilrs → libudev-sys |

If you meet a new one, **add a default-on feature** rather than working around
it in the shell. Three crates already do this; it is the house pattern.

**And check the NON-default configurations before believing a crate is green.**
`cargo ailite check` builds the default features only. `games/clawbound`'s
`app` feature (winit/wgpu/egui) does compile in this sandbox — it was long
recorded as blocked on rustc >= 1.76, which expired when the toolchain moved to
1.96 — and it was BROKEN for a while because two merged menu commands had no
`app.rs` arms, which no default-feature test could see (`7427ba3a4`).

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

## 5b. A test that has never failed has not been tested

**Before trusting a new check, break the thing it checks and confirm it
notices.** Costs a minute. This session it caught three checks that were
measuring nothing, and in every case the check was green on arrival, which is
what made it convincing.

* **Fixture-gated tests.** 28 sites in gpu-gemm read a `/tmp/tiny-*.gguf` and
  `return` when it is absent. A test that returns early PASSES, so the suite
  reported 1158 green while the loader, the diffusion path and the MoE routing
  never executed. Generating the fixtures took the same suite from 5.85 s to
  20.35 s — same tests, same result line, three and a half times the work.
* **A validator pointed at nothing.** `wgsl_validate.rs` shipped for months
  with three tests that exercised the VALIDATOR and never the payload. 187
  shaders went unchecked while the tool to check them sat in the same file.
* **An assertion satisfied by the wrong mechanism.** `micsample`'s noise-floor
  tests asserted `keeps == 0`, and keeps is zero whatever the onset threshold
  does, because later gates reject repeated room tone regardless. The whole
  module — 22 tests — passed with the threshold set to 1.02 (fires on
  everything) and to 40.0 (deaf to anything short of a gunshot).

The shape is the same each time: **the pass was guaranteed by something other
than the property being claimed.** Reading the test does not reveal that.
Breaking the code does, immediately.

Three habits fall out of it, in rough order of how often they pay:

1. **Assert on the mechanism you mean.** If the claim is about an onset
   threshold, count onsets — not keeps, which four other gates also control.
2. **Assert a RANGE when both directions are failures.** `>= 5` catches one
   bug; `(2..=5)` catches that bug and its opposite. A floor alone is satisfied
   by a check that has stopped discriminating.
3. **Assert the scan found something.** A structural test that greps sources
   passes every assertion below it when the grep matches nothing. Every scan in
   this tree asserts a floor on its own match count first, and that is why.

And the corollary, which is harder: **a test that fails when you expected it to
pass is usually right.** Twice this session the failing test was correct and my
expectation was the bug — a cosine metric that could not see brightness, and a
claim about adaptation that was simply false. A test written and then deleted
because its claim turned out false is a good outcome, not wasted work; say so
in the commit rather than quietly dropping it.

## 5c. Tests written around a fix say nothing about the fix's side effects

The tests you write while fixing something are about the bug. They pass on a
change that fixes the bug and breaks something else, because nothing in them
looks there. **Before committing a fix, run the whole crate, and ask who the
change takes something AWAY from.**

Three times in one session, all the same shape — a fix that excludes or
redirects a category, and the hole appears wherever that category was the whole
answer:

* `verdict::extract_code` joined every fenced block, so a reply that showed its
  own output failed to compile. Fixed by skipping non-source fences. That broke
  a worker asked to produce a **manifest**, whose only fence is `toml` — it now
  fell through to the raw reply, prose and fence markers included, worse than
  the original bug. Five tests written for the fix all passed.
* The `IQ1_S` native loader path is right about residency — 1.5625 bpw against
  q4's 4.5 — and wrong about every multi-token forward, because nothing
  converts for prefill. It panicked inside `prewarm`, during load.
* `parse_plan` closed at the first fence, losing every step after a quoted
  snippet. Fixed with `rfind`, whose comment claimed over-reach was harmless. It
  reached into the following `` ```tool `` blocks and broke two existing tests
  immediately — tests that were right there and that I had not run.

The habit that catches all three is one question: **what did this change take
away, and who was relying on it?** For an exclusion, that is "who does the
excluded case belong to". For a widening, "what else is now inside the net".

Two cheap mechanics:

1. **Run the whole crate, not the filtered test.** `cargo test -q --lib name` is
   for the edit loop. The commit needs the full suite, and it is where the
   `rfind` mistake surfaced in seconds.
2. **When a fix adds a category test, add its complement.** Skipping non-source
   fences wanted "and what if there are ONLY non-source fences". Bounding a
   region wanted "and what if the region is the whole document".

## 5d. A clean build proves the file compiles, not that your edit is in it

**When an edit and a build run in one command, a failed edit reads as a passing
build.** The edit script raises, the build runs on the unchanged file, and the
last line of output says what you hoped to see.

Three times in one session, all the same:

* removing voiceterm's `rail` panel (`264582dd`) — the anchor did not match, nothing was
  written, `cargo check` said `(empty = compiles)`. It was compiling the file
  that still had `rail` in it.
* probing the egui chrome audit (`92d42abb`) — a `str.replace` whose pattern
  did not match, a success line printed unconditionally, and the unchanged count
  read as evidence that the TOOL was broken. Worse than a wasted build: it
  argued for undoing a correct fix.
* wiring the session clock into logscope (`b300aeaa`) — I anchored a new field on
  `tabbar_open`, which is VOICETERM's field, in a script editing logscope. The
  traceback printed above a `(empty = compiles)` and I read the second line.

The scripts already `assert old in s`. The assertion fired every time. What
failed was reading: a Python traceback and a build result in one output block,
and the eye goes to the familiar line.

Three mechanics, in order of how much they pay:

1. **Do not chain an edit and a build in one invocation.** Run the edit, read
   its output, then build. Two commands, and the failure has nowhere to hide.
2. **Make the edit print what changed** — a line count, the new text, the
   grepped result — so success has a positive signature rather than an absent
   error.
3. **After an edit that should change behaviour, grep for the change** before
   trusting a build. `grep -n 'new_thing' file` is one command and answers a
   different question than `cargo check`.

The generalisation is worth stating on its own: **a green result describes
whatever was actually run, which is not necessarily what you think you changed.**
The same session had `git gc` reporting a corrupt pack from stale output, a
target dir serving a cached poisoned rustc probe, and a fleet script writing
logs to a directory it never created. In each case the output was accurate about
something other than the question being asked.

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
python3 maintenance/tools/aarch64_flags_check.py  # committed CPU flags vs the weakest target
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
