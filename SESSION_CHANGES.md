# Session changes since the uploaded `c15unified-008` bundle

This document exists for whoever (human or another Claude instance) needs to merge this session's
work into a different copy of the repo. It covers every commit made in this conversation, in
chronological order, with what was verified and how — not just what changed. Where a claim in a
commit message was re-checked against the actual repo state before being trusted, that's noted;
this session held to a standard of re-verifying prior work rather than assuming commit messages
are accurate, and the same scrutiny should apply when merging.

Full commit range: `2d97b6b4..HEAD` in `apps/logscope` terms, but changes span five apps and
`maintenance/tools/`. 21 commits, 25 files, +1230/-76 lines.

## How to get these changes

The exported git bundle (`c15unified-NNN-*.bundle`, latest revision) contains complete history and
can be fetched into any clone that shares ancestry with the original `c15unified-008` bundle:

```
git bundle verify c15unified-XXX.bundle
git fetch /path/to/c15unified-XXX.bundle HEAD:incoming-session-work
git log --oneline main..incoming-session-work   # review before merging
```

Because the bundle carries full history (not a shallow diff), a fetch only transfers what the
target repo doesn't already have — safe to fetch into a repo that already has some of this
session's earlier commits.

---

## Real bugs fixed (behavior was actually wrong)

### `apps/logscope/crates/logscope-egui/src/app.rs` — fake Ollama scan data shipped as real
**Commit `de60b1fc`.** `rescan()` was live production code (not a test fixture) that hardcoded two
fake Ollama hosts with fake model names (`qwen2.5:7b` on a made-up `192.168.1.42`) instead of
calling the real, already-built LAN scanner (`logscope::providers::scan_lan_parallel`,
`probe_host`) that had simply never been wired in. A user's model picker could show servers and
models that don't exist on their network. Fixed by wiring the real scanner, bridging its
`crossbeam_channel` type into the existing `mpsc` channel the app already polls. **Verified**: built
a standalone binary outside the main tree calling the exact same functions, ran it for real —
completes in ~5s, 0 hosts found (correct for a sandbox with no LAN/Ollama server; proves the code
path doesn't hang or panic). Full suite 1319/1319 after this change.

### `apps/logscope/crates/logscope-fan/src/verdict.rs` *(prior to this doc's commit range, but worth knowing)*
Not in this range — see repo history before `2d97b6b4` if auditing further back. Included here only
as context: `Ok(o) if o.status.code() == Some(127)` now maps to `Verdict::None` instead of `Fail`,
so a missing checker binary (e.g. no rustc on a mobile target) doesn't silently poison ranking.

---

## Real features added

### `make_tool` — the model writes and caches its own Rust tools
**Commit `70dd3889`.** New `Tool::MakeTool { name, source }` in `workspace_exec.rs`. Writes a
complete Rust program, compiles with `rustc -O`, content-hash-caches the binary at
`.logscope/tools/<name>/<name>` (identical source on a later call skips recompilation), and on
success registers the tool's slug in *that workspace's own* `run` allowlist (not the global
default) so it's invoked through the existing `run` tool. This codebase has a documented,
test-enforced pattern for adding a tool (8 places must agree — the `Tool` enum, `wire_name()`,
`summary()`, `apply()`'s dispatch, `workspace_exec::RUNTIME_TOOLS`,
`logscope_fan::queen::RUNTIME_TOOLS`, the validator, the JSON parser, the system prompt menu, and a
hand-maintained exhaustive-variant test list). All were updated; the test suite's own safety net
caught one missed site on the first run (`the_worker_whitelist_names_exactly_the_runtime_tools`
plus the exhaustive-variant list), which was then fixed. **Verified**: a real `fn main()` program
compiled, invoked through the actual `run` tool with its stdout checked (not just exit code), a
second identical call confirmed to hit the cache, and a program that fails to compile confirmed to
leave no usable allowlist entry. Full `workspace_exec` suite 135/135.

### `sed` and `tldr` added to the default tool allowlist
**Commit `c0a5dec1`.** Part of "teach it sed and other tools like ripgrep head tail git tldr" — most
of that list (`head`, `tail`, `grep`, `rg`, `find`, `git`, `wc`) was already present; checked before
assuming otherwise. Only `sed` and `tldr` were missing. `sed -i` is a real risk, but the allowlist
gates by exact basename only, never by flag (same as `git reset --hard` / `find -delete` already on
the list) — not a new risk category. **Verified**: installed `tealdeer` in the sandbox specifically
to test; new test exercises the real default allowlist (not one granted for the test) and checks
`sed` actually transforms its input, not just that the binary ran.

---

## Real ported-UI mockups found and fixed (GTK → egui parity audit)

Two of three tabs found via line-count triage (compare every `crates/logscope/src/ui/*_tab.rs`
against its egui counterpart by line count — an extreme shrink ratio is the signal) turned out to
be visual mockups with fake data and no-op buttons standing in for real, already-built backend
logic. One did not get fixed, on purpose — see below.

### `tabs/modify.rs`
**Commit `f01f2931`.** Not fully detailed in this doc (made by an earlier pass in this same
conversation, independently re-verified rather than trusted: line counts checked, and the full test
suite re-run by hand — 1325/1325 confirmed, not just read from the commit message).

### `tabs/rotate.rs` — second mockup
**Commit `abe0f1b0`.** `rotation_tab.rs` (931 lines) → `rotate.rs` (71 lines, 13.1x shrink — the
most extreme of any tab) was initially misreported as having "no egui counterpart" by the triage
script's own naive filename derivation (`rotation_tab.rs` doesn't map to `rotation.rs`, it maps to
`rotate.rs`) — caught by checking rather than trusting the script's output. `config_text()` built a
preview with no connection to a real backend, and "Apply" was a no-op that always reported success.
`logscope::rotation` (`write_logrotate_conf`, `write_systemd_user_units`) and
`logscope::probe::probe` already existed, real and GTK-free. Wired real tier/ownership probing and
real config writes, reporting genuine outcomes instead of an unconditional "Applied". **Verified**:
5 new tests against real files — one writes to an actual temp directory and reads the bytes back to
confirm the real values landed; another forces a genuine `create_dir_all` failure and confirms it
surfaces as failure. Full suite 1330/1330.

### `tabs/bloc.rs` — found, wired, then correctly REVERTED
**Commits `5745b390`, `d50ae110`.** Same mockup shape (fake bloc list, selection that didn't load
the editor). `logscope::bloc_engine::builtin_templates()` looked like the same fix — until wiring it
hit a real compile error: `bloc_engine` is `#[cfg(feature = "bloc")]`, and that feature is
`tree-sitter` + three language grammars. `logscope-egui` depends on `logscope` with
`default-features = false` deliberately. Enabling `bloc` to reach 32 template strings would add real
native-code compilation to every egui build, on a project targeting mobile ARM where build time and
binary size are stated priorities. **Reverted the port rather than ship that tradeoff
unilaterally** — confirmed `cargo check --features gui` clean after reverting. Documented precisely
in `FIELD_NOTES.md` with the actual fix for whoever decides the tradeoff is worth it (split the
tree-sitter-free items into an ungated location).

### `components.rs` — assessed, not a gap
**Commit `d50ae110`.** The last named item in the original parity audit. Every function in it is
either literal GTK widget construction, or solves a problem specific to GTK's retained-mode
architecture (a string-key decode workaround for `gtk4::DropDown`, an idle-callback debouncer for
`glib`'s mainloop) that egui's immediate-mode design doesn't have. Not a porting gap — already
superseded by a different, adequate design. Closes the parity audit this session's own note opened.

---

## Real gaps found, precisely documented, deliberately NOT force-wired

Each of these was a case where forcing a fix blind would have meant making an architecture or
dependency-cost decision that isn't a verification pass's place to make. Each is written up with
enough specificity (exact functions, exact call sites, exact missing pieces) that someone with the
right context can execute quickly rather than re-derive the investigation.

### Vision (image-to-text) — TWO real, unwired implementations, not zero
**Commits `f3927b97`, then corrected in `f24d2115`.** First pass found the delegate path
(`vision_cli.rs` argv builder for llama.cpp's `llama-mtmd-cli`, `media.rs::describe_image` as the
real entry point) has zero callers anywhere in the UI — same waterline as image generation and TTS,
which have the identical problem (the whole `media.rs` subprocess-delegate layer was built and never
given a UI trigger).

**Then a self-correction**, found via a different checker (`unhosted_check.py`) later the same
session: the original claim "gpu_kernels has no native vision path at all... not attempted" was
incomplete — it only checked the `gpu-gemm` crate. The `logscope` crate has a complete,
~1100-line, tested, native CPU-reference ViT vision encoder (`multimodal.rs` + `vision_preprocess.rs`
+ `vision_encoder.rs`) written with llama.cpp's `clip.cpp` as a direct reference for correctness
constants, whose entry point `encode_image()` has a doc comment describing it as already integrated
("the single call the engine makes") — but has zero callers anywhere, in any crate. Corrected the
TODO.md entry in place: there are two real, unwired options (delegate to llama.cpp, or finish the
native path), not one, and whoever wires this should choose deliberately.

**Not wired**: both paths need UI-thread work (a trigger command, off-thread execution via the
existing `mpsc::channel` + `thread::spawn` + poll-per-frame pattern already used at `chat.rs:8093`
for `VerifyJob`) that a verification pass shouldn't force through blind on a 10,000+ line file with
no display to check the result.

### `voiceterm`'s entity-picker disambiguation — complete, never connected
**Commit `5ae09491`.** Found via `wiring_check.py`. `EntityPicker`/`picker_ui` in `detect.rs` is a
complete, tested disambiguation UI (question + candidate rows + "none of these") that correctly
refuses to open on a single match — but the entire subsystem underneath it (`EntityPicker`,
`ResolvedMatch`) has zero references anywhere outside its own file. Whatever currently resolves an
ambiguous spoken entity reference almost certainly picks silently today, which is the exact failure
this subsystem exists to prevent. New `apps/voiceterm/TODO_entity_picker_wiring.md` (matches this
app's own one-file-per-feature convention) with a concrete next-step list. Added an honest doc
comment on the code — deliberately NOT using `wiring_check.py`'s exact suppression phrase ("Not
called yet, and that is not an oversight"), since that phrase means a settled, permanent decision
like the tool's own `keyboard_ui_with_signal` example, and this is an open gap, not a settled one.

### `apps/logscope`'s `debate` panel — audited, narrower gap than the note implied
**Commits `f63aa6a0`, `66162221`.** `debatepanel::ui` IS mounted and fed real state (the original
TODO note's claim that it wasn't checked was itself wrong). Only `debaterun`'s blind-marking half is
genuinely unwired, and the code already self-documents the gap honestly (`blind: false` with a
comment explaining why). Left open: wiring it needs deciding how the fan branch system would track
per-round operator visibility, which it doesn't model today.

### `apps/fao-codec`'s v9 volumetric codec — partial answer to its own decision point
**Commit `e6e88d48`.** The codec's own TODO has a "decision point": encode real video, check for a
coherent image. No real video exists in this sandbox. Added a synthetic end-to-end test (a real
triangle through the full `build → serialize → parse → decode → ray_march` pipeline) — result:
positive, the pipeline's basic wiring works. Explicitly documented as a partial answer: a flat
single-color triangle proves nothing about occlusion, multi-block boundaries, or real scene
complexity, which is what the actual decision point needs.

### `games/unendlich` — two real test failures, diagnosed, deliberately not fixed
**Commit `382c8ddd`.** A `candidate_roots` ordering bug and a `voxelize_boxes` off-by-one, both
diagnosed by hand, both left for someone to decide semantics rather than guessed at.

### `apps/rusty-board` — stale TODO note corrected
**Commit `34966ffc`.** "Deprecation cleanup mostly done, maybe stragglers" was actually **fully**
done — grep confirms zero remaining deprecated GTK4 calls. Corrected the note; couldn't
compile-verify (no fetchable `libgtk-4-dev` in this sandbox — a mirror gap, not a repo issue), but
the grep-confirmed absence is what a clean build would also show.

### `apps/logscope`'s decode-starvation UI seam — stale TODO note corrected
**Commit `d69b4b57`.** A "remaining seam" note described a feature as unbuilt that had, in fact,
already fully landed (with tests) in a prior session. Corrected in place.

---

## Maintenance checker fixes (`maintenance/tools/*.py`)

Five checkers audited this session; real bugs found and fixed in four of them, all independently
verified against real cases (not just read from the diff) before being trusted.

### `aarch64_flags_check.py`
**Commit `de654bbe`.** A bare `"cortex-a7"` substring in `UNSAFE_CPUS` was wrong in both directions
at once: it FALSELY flagged `cortex-a72`/`cortex-a73` (ARMv8-A, same generation as the PinePhone's
`cortex-a53` baseline this checker protects — genuinely safe) while MISSING `cortex-a55` entirely
(ARMv8.2-A, genuinely unsafe, and one of the most common small cores in real ARM SoCs). Verified
against ARM's own architecture pages (cross-referenced, not one source) before writing the fix.
Replaced with the specific confirmed-unsafe names (A55, A75, A76, A77, A78).

### `desktop_check.py`
**Commit `2efd3623`.** Flagged `tadpole-egui` for missing a `.desktop` entry, but that crate has no
binary at all (library-only, no `[[bin]]`, no `main.rs`, no `src/bin/*.rs` — checked all three ways).
Added a `has_binary()` gate. Also fixed two genuine gaps it correctly found:
`apps/ambient-learner`'s egui frontend had no app-id wiring at all (meaning no `.desktop` entry could
ever work on Wayland), and `apps/hatch` was missing its `metainfo.xml`. Both fixed with real files
and code changes; a third real gap (`hatch`'s missing icon SVG) was correctly left alone as genuine
art/design work.

### `shared_survey.py`
**Commit `5f27da24`.** `consumers()` only checked the fixed path `apps/<app>/egui-frontend/Cargo.toml`
— which doesn't exist for `logscope` at all (its real frontend is at `crates/logscope-egui/`). This
silently zeroed out usage counts for every shared crate the single largest, most actively developed
app in the repo depends on. Confirmed by hand: `math-kit`, `noise-kit`, `scene-kit` were all falsely
listed as "unused". Fixed by walking every `Cargo.toml` under `apps/`. The "no consumer" list
dropped from 25 crates to 9 after the fix.

### `wiring_check.py`
**Commit `931ce174`** (made earlier in this session, independently re-verified here rather than
trusted). A false positive where a test-harness helper's closure PARAMETER (typed `&mut Ui` three
layers deep) satisfied the detection regex meant for a function's own first parameter. Fixed by
anchoring the check.

### `reachability.py`
**Commit `824baf9b`.** `strip_comments()` deleted any line starting with `#` — correct for nothing
in Rust (`//` is the only line comment; `#[...]` is an attribute). This silently destroyed
`#[path = "..."]` redirect attributes before the reachability check ever saw them, so a
path-redirected file was invisible to every downstream check. Same underlying file
(`render_meshref_shim.rs`) that `orphan_check.py`'s own `#[path]` fix (from earlier in this session)
covers from a different angle — found independently through this checker's separate logic. Fixed by
checking for the redirect on raw text before comment-stripping runs. Also found and DOCUMENTED (not
fixed) a real performance problem: `orphans()` is O(modules × files × patterns) and doesn't finish
in 5 minutes on `unendlich-engine`'s 1300+ source files — noted in the module docstring so nobody
assumes a clean run on the tree's largest crates that in fact never completed.

---

## What to check before merging

1. **Cargo.lock changes** in `apps/logscope` and `apps/fao-codec` are legitimate — `crossbeam-channel`
   and `dirs` were added as direct dependencies of `logscope-egui` (for `make_tool`'s channel
   bridging and `rotate.rs`'s `~` expansion respectively), and one pre-existing lockfile staleness
   fix (`math-kit` as a `uikit-egui` dependency) was picked up as a side effect of running `cargo
   test` in that workspace for the first time. `lock_check.py` is clean.
2. **`apps/logscope/crates/logscope-fan/src/queen.rs`**: its `RUNTIME_TOOLS` constant is a
   deliberate cross-crate duplicate of `workspace_exec::RUNTIME_TOOLS`, kept honest by a test
   (`the_worker_whitelist_names_exactly_the_runtime_tools`) that fails if they disagree. Both were
   updated together for `make_tool`; verify that test still passes after merging.
3. All five maintenance-checker fixes change *checker behavior*, not application behavior — running
   them post-merge should show fewer false positives, not new failures. If a checker reports
   something new after merging, that's worth a second look rather than an assumed regression in the
   checker itself.
4. Every commit message in this range states what was verified and how (tests run, files
   read, functions called directly) — where a commit says "grepped, not assumed" or "verified by
   running," that check was actually performed in this sandbox, not inferred.
