# CLAUDE.md — directives for an agent working in this repo

Read this before running anything. It is short on purpose. Every rule here cost
a session to learn, and most of them present as something other than what they
are.

Orientation, not rules: [NEWCOMER_DEVELOPER.md](NEWCOMER_DEVELOPER.md) for the
build system, [LAYOUT.md](LAYOUT.md) for the taxonomy,
[docs/agents/INDEX.md](docs/agents/INDEX.md) for the doc catalog,
[maintenance/C20UNIFIED_TODO.md](maintenance/C20UNIFIED_TODO.md) for the newest open threads,
with [maintenance/C20_TODO.md](maintenance/C20_TODO.md) (a DIFFERENT session's: it came with
the blackstar-rs drop), [maintenance/C19_TODO.md](maintenance/C19_TODO.md) and
[maintenance/C18_TODO.md](maintenance/C18_TODO.md) behind them;
[maintenance/C14_TODO.md](maintenance/C14_TODO.md) for the queue before those
(and read its §1 before acting on C13_TODO §19/§27 — it disproves their stated
cause), [maintenance/C13_TODO.md](maintenance/C13_TODO.md) for the queue behind
it, [maintenance/C12_TODO.md](maintenance/C12_TODO.md) behind that,
[maintenance/C11_TODO.md](maintenance/C11_TODO.md) for the queue behind it,
[maintenance/C10_TODO.md](maintenance/C10_TODO.md) for the live egui/GTK parity
queue behind it (edited in place where c11 moved its status lines)
and [maintenance/C9_TODO.md](maintenance/C9_TODO.md) for the larger backlog
behind it (still current — c10 edited its status lines in place).

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

**Install before you gate.** Much of what this section used to call "missing"
is one `apt-get update && apt-get install` away, and c12 verified it by
building the crates that were recorded as unbuildable:
`libasound2-dev`, `libopus-dev`, `cmake`, `xorg-dev`, `libgstreamer1.0-dev`,
`libgstreamer-plugins-base1.0-dev`, `mesa-vulkan-drivers` (lavapipe — there is
no ICD in a fresh sandbox, despite older notes assuming one) and `vulkan-tools`
all install cleanly. `apt-get update` FIRST — the baked lists are stale and bare
installs 404 — and clear the `deb.nodesource.com` entry, which 403s and makes
the whole update look failed.

Still genuinely absent: Qt6, `/dev/dri` (so lavapipe is the only device, CPU
and slow).

**GTK4/libadwaita are NOT absent**, though this section said so until c19:
`libgtk-4-dev` (4.14.5) and `libadwaita-1-dev` (1.5.0) install from apt, gtk4-rs
0.11 / libadwaita-rs 0.9 build against them in about a minute, and a GTK app
runs under `Xvfb` with `GSK_RENDERER=cairo` and can be screenshotted
(`apps/stoat/crates/gui-gtk` was built and looked at this way). Also install
`librsvg2-common`: without its gdk-pixbuf loader most symbolic icons draw as the
missing-image glyph, which reads as an app bug and is not one. What is still
true: no input under bare Xvfb, so touch gestures cannot be exercised. `libssl-dev`/`libudev-dev` are untried since the
list was written; try apt before believing them.

A build script failing still takes the whole crate with it, including headless
code that never touches the device — so the escapes below stay, because they
are right for a host that really lacks the headers. What changed is that here
you can usually verify the DEFAULT configuration instead of only the gated one.

| crate | flag | blocked by | still blocked here? |
|---|---|---|---|
| `apps/beatspace` | `--no-default-features` | cpal→alsa-sys, opus→audiopus_sys(cmake) | no — both apt-installable (c12) |
| `games/blackstar-rs` | `--no-default-features` | cpal→alsa-sys | no — `libasound2-dev` (c12) |
| `apps/studio` `studio-media` | `--no-default-features` | opus→libopus via cmake | no — built with default `opus`, 21 tests (c12) |
| `system/snapclone` `snapclone-crypto` | `SNAPCLONE_SKIP_CBINDGEN=1` | see §4 | yes |
| `games/blackstar-rs` `gamepad` | already off by default | gilrs → libudev-sys | untried |

If you meet a new one, check apt first; if apt does not have it, **add a
default-on feature** rather than working around it in the shell. Three crates
already do this; it is the house pattern.

**And check the NON-default configurations before believing a crate is green.**
`cargo ailite check` builds the default features only. `games/clawbound`'s
`app` feature (winit/wgpu/egui) does compile in this sandbox — it was long
recorded as blocked on rustc >= 1.76, which expired when the toolchain moved to
1.96 — and it was BROKEN for a while because two merged menu commands had no
`app.rs` arms, which no default-feature test could see (`7427ba3a4`).

## 3b. The GPU is an M2 on Asahi — read the stack doc before GPU work

Vulkan here is **Honeykrisp (Mesa) on an Apple M2 Max**, plus llvmpipe, which is
also enumerated. Rust reaches it through `ash` (unendlich) and `wgpu`
(logscope/gpu-gemm). What was measured, the driver faults, the hazards (a hung
dispatch needs a reboot; `bench.sh` refuses while `ollama serve` runs) and the
harness tiers are in [docs/agents/APPLE_M2_GPU_STACK.md](docs/agents/APPLE_M2_GPU_STACK.md).
A GPU timing is only a hardware result if it came from
`games/unendlich/harness/gpu_hw_check.sh` or perf-loop's `bench.sh`; a lavapipe
number proves correctness, not speed.

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
python3 maintenance/tools/eframe_backend_check.py . # eframe backends chosen, not inherited (glow, or wgpu with visible intent)
python3 maintenance/tools/voiceprint_parity_check.py . # voiceterm's fingerprint/compute_mfcc vs shared/voiceprint-kit's copy
python3 maintenance/tools/board_gestures_parity_check.py . # rusty-board's GTK swipe recognizer vs crates/board-gestures' copy
python3 maintenance/tools/cfg_feature_check.py .   # cfg(feature="x") where no manifest declares x
python3 maintenance/tools/aperture_cmdstrip_check.py . # aperture command-strip pins vs the palette labels they resolve against
python3 maintenance/tools/textedit_id_check.py .   # TextEdit with no stable id — the one-character input bug
python3 maintenance/tools/font_install_check.py . # every egui app reaches uikit-egui's fonts::install, or its symbol glyphs are boxes
python3 maintenance/tools/palette_surface_check.py . # an egui app that draws a command palette draws it with command_bar::show (bottom and inverted on a phone)
```

`maintenance/tools/break-test.sh` is for the §5b break test itself. `source` it,
set `BRK_BACKUP` to a copy of the file taken BEFORE any edit, and call
`brk <file> <old> <new> <label> <pkg> <filter>`. It reports four outcomes and
the distinction between them is the point:

* **caught** — the break failed a test, which is the result you want
* **NOT CAUGHT** — nothing tests this
* **DID NOT COMPILE** — inconclusive, and NOT a pass
* **ANCHOR MISS** — nothing was changed, so the run says nothing

A fifth outcome the script CANNOT detect: an anchor that matches inside a
DOC COMMENT. Breaking `<<'GITMIND_UNIT'` reported NOT CAUGHT because the first
occurrence in the file was the comment explaining why the delimiter is quoted,
so the edit was inert. Anchor on something that only appears in code, and
prefer driving the loop from Python over bash — three of four breaks in that
same run were ANCHOR MISS purely from shell escaping of `\n` and nested
quotes.

The third and fourth are why it exists. A hand-rolled `grep -c FAILED` reports
zero failures for a break that did not compile and for one whose anchor never
matched, and both read as "nothing catches this" — that happened twice in one
session. Note also that `error: test failed, to rerun pass ...` is cargo
reporting a TEST failure, so a bare `^error:` match misclassifies a genuine
catch as a build failure; the script matches `error[E...]` and
`error: could not compile` only.

`font_install_check.py` exists because shipping the symbol fonts fleet-wide
(`7098bc4a`) fixed only the apps that CALL `apply_palette`, and switcheroo did
not — it was the one egui app with no `uikit-egui` dependency, so `★`, `☆` and
`↻` rendered as boxes there while working everywhere else. Nothing flagged it;
it was found by reading that app's colours for an unrelated reason. Ten more
crates are in the same state and listed in the checker's `KNOWN_MISSES`.

`voiceprint_parity_check.py` exists because voiceprint-kit is a straight copy
of voiceterm's own fingerprint code (`pub(crate)` widened to `pub`), not a
shared dependency — see `maintenance/C12_TODO.md` §2. Nothing else notices if
one side changes and the other doesn't; the checker fails loudly on drift or
on either function going missing, until voiceterm depends on the kit for
real (C12-2a) and the second copy can be deleted.

`board_gestures_parity_check.py` is the same guard for the same reason, one
layer over: `apps/rusty-board/src/gestures/{swipe,recognizer}.rs` and
`crates/board-gestures/src/{swipe,recognizer}.rs` are 2233 duplicated lines
decoding the keyboard's headline gesture for its two frontends, and the GTK
crate does NOT depend on the shared one despite the egui manifest's comment
saying it does. They AGREE today — the whole diff is the documented
Key->label/swipe_target decoupling — which is why pinning them now is cheap:
the checker starts green for a real reason and the first drift is what it
catches. Drift here does not panic or fail to compile; both sides keep
returning plausible words and slowly disagree about what the user swiped.

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
  compilation" is an acceptable state here — but it has to be *said*, or the
  next session inherits a claim it cannot audit.
- **Check what the sandbox can actually do before claiming it cannot.** This
  rule used to read "GPU, Qt and GTK paths cannot be built in this sandbox".
  That is true of **Qt** (`qmetaobject` needs headers that are not installable)
  and **false of egui/eframe/glow**, which build, link, run against software
  GL, and can be driven with synthetic keystrokes and screenshotted — see
  [docs/agents/HEADLESS_GUI_TESTING.md](docs/agents/HEADLESS_GUI_TESTING.md).
  The claim was written for Qt and generalised; a session inherited it and
  spent sixteen commits reasoning about a UI it could have looked at, then a
  single screenshot overturned a premise four of those commits rested on. One
  `cargo build` is cheaper than the assumption.
- Write long messages to a file and use `git commit -F`. Heredocs through a
  `sh -c` wrapper mangle apostrophes and silently truncate.
- **`cargo test --lib` is not the test suite.** It skips `tests/`, and a
  signature change to a public function will sit broken there while every run
  reports green. A session changed `PtyHost::spawn` to take a required
  argument and reported "1237 tests pass" fifteen times before discovering
  `tests/real_pty.rs` — the only test that drives a real shell — had not
  compiled since. Run plain `cargo test` (all targets) before claiming a count,
  and reach for `--lib` only to go faster mid-loop.
- **A harness that measures runtime behaviour must load its data the way the
  runtime does, and should name the function it mirrors in a comment.** A
  self-consistent harness answering a question nobody asked is the hardest kind
  of wrong to notice: every number it prints is reproducible. In one session a
  fold audit measured the unified prim set instead of the model's own, a
  screenshot tool rendered a pose the game never draws, and the same audit used
  an unoriented, unscaled loader — three separate wrong answers, all internally
  tidy. The sibling harness had the rule written down already.
- **A grep hit pointing at an existing module is worth two minutes before you
  build a parallel one.** `src/foldaudit.rs` — 549 lines, nine tests, wired into
  two callers — appeared in a search result and was not followed up; a second
  audit got written beside it.
- **"Nothing references this" is a claim to grep for across the whole repo.**
  Two of three modules called unreferenced in one commit had callers: one was
  the Qt app's own file reached by `#[path]`, another was used by an
  integration test. Impressions from one directory are not an audit.

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
