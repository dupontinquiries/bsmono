# read_near, the capacity briefing, and the model-gap HUD

Three additions from the same pass, grouped here because none had any doc coverage and each closes
a gap of the same shape: logic that existed and was tested, with nothing calling it.

## `read_near` — partial reads that actually use the planning that existed for them

Before this pass, `symbolfind.rs` (rank a grep hit as a Definition vs. a mere Use vs. a comment or
string literal) and `filewindow.rs` (merge nearby hits into windows, and say honestly what was NOT
read when a read is partial) were both fully implemented and unit-tested, with no caller anywhere
in `workspace_exec.rs`. A model could `search` for a name and then still had to guess at
`read_range` windows one attempt at a time, or read a whole file, to actually see it.

`Tool::ReadNear { path, name }` connects them:

1. Greps `path` for `name`, classifies each hit with `symbolfind::rank`.
2. Prefers a Definition as the anchor; if none of the hits are a definition (a use-only or
   comment-only search), anchors on every match instead, capped at 20 so a common word can't
   explode into one window per occurrence.
3. Builds a windowed plan with `filewindow::plan`, budgeted to the run's model class
   (`read_harness::budget`).
4. Renders each window through `read_harness::fence` (the same sanitized, provenance-labeled
   rendering every other read tool uses) and appends `filewindow`'s honest partial-read caveat
   when the plan didn't cover everything.

Wired through every place a tool must agree — this codebase enforces that with a test
(`workspace_exec::long_horizon_tests::every_runtime_tool_is_named_in_the_canonical_list`) that
walks one instance of every `Tool` variant against `RUNTIME_TOOLS` and panics if they drift, which
is exactly the class of bug a first attempt at this feature actually hit (see the roster/queen
section of `12-queen-and-workers.md`'s history for how that was caught). The tool is registered in:

* `Tool` enum, `wire_name`, `summary`, `apply` (`workspace_exec.rs`)
* `RUNTIME_TOOLS` in **both** `logscope-egui::workspace_exec` and `logscope-fan::queen` — a parity
  test enforces these two lists never drift apart
* The JSON tool-call parser (`path`/`name` fields, `name` also accepted as `symbol`)
* The prompt menu text for both `ReadPolicy::Full` and `ReadPolicy::Surgical`
* The one-line grammar small models fall back to (`line_calls.rs`):
  `!<nonce> read_near <path> <name>` — `name` takes the rest of the line rather than one token, so
  a multi-word anchor copied from a grep hit isn't silently truncated.

## The capacity briefing — filling the Liaison's missing data, honestly

`docs/12-queen-and-workers.md`'s audit of the Liaison found every operator-facing query
(`briefing`, `what_changed`, `what_is_blocked`, …) fully implemented and tested against
`&ExplorationTree` — with nothing in the app ever constructing one. Inventing branches from
nothing would repeat the exact mistake the Liaison itself exists to refuse ("an explainer that
fabricates a reassuring status is worse than no explainer").

`logscope::capacity_tree` fills that gap with findings the project had already written down —
`CAPACITY.md`, `docs/model-fit.md`, `docs/hardware-matrix.md` — converted into `Branch`es, each
carrying the source doc's own numbers as its `evidence()`, not a paraphrase presented as new
confidence.

* **`RamTier`** — `Mobile8` (≤8 GiB, postmarketOS/Fairphone/PinePhone-class ARM), `Laptop24`
  (≤24 GiB), `Workstation64` (≤64 GiB, M4 Max class), `Server192` (≤192 GiB, M2 Ultra class).
  `RamTier::classify(gib)` maps a real number onto the smallest tier it fits under; anything above
  192 GiB still classifies as `Server192` rather than inventing a tier nothing has measured.
* **`capacity_tree()`** builds ten branches across the four tiers — e.g. Qwen3.6-27B dense marked
  `Succeeded` for `Workstation64` (model-fit.md's own pick for the M4 Max), Qwen3-Coder-Next
  `Succeeded` for `Server192`, GLM-5.2 marked `Failed` with the exact "no quant fits 192 GiB"
  arithmetic from `docs/model-fit.md`, and gemma4/qwen4 marked `Deferred` with their real retry
  conditions (registry support; a real checkpoint download), tagged across all four tiers since
  architecture support isn't RAM-dependent.
* **`physical_ram_gib()`** reads `/proc/meminfo`'s `MemTotal` — Linux only, `None` elsewhere or on
  read failure, rather than guessing. **`detect_tier()`** classifies the real machine, falling back
  to the middle tier (`Workstation64`) when RAM can't be read, and the `:capacity` command says so
  explicitly rather than pretending certainty.
* **`briefing_for_tier()`** orders findings Succeeded → Active → Deferred → Failed (decisions cost
  the most to not know first, ruled-out options last) — the same ordering discipline
  `Liaison::briefing` uses, so the two surfaces read consistently even though this tree is separate
  from whatever `ExplorationTree` a future fan/agent-activity wiring eventually builds.

**`:capacity` / `:cap`** (`vimmode.rs` → `chat.rs`) detects the tier, builds the tree, and prints
the briefing to `self.note`. This is a *separate*, static, hand-curated tree from the live-activity
`ExplorationTree` question `12-queen-and-workers.md` still leaves open — deliberately not
conflated, since the two answer different questions ("what models fit my hardware" vs. "what has
this session's fan/agent activity actually done").

## The model-gap HUD — closing `ModelStatus.arch`

The other half of the same Liaison audit: `ModelStatus` (the *other* input `what_is_blocked_with`
needs) was blocked on `gpu_kernels::Model` computing architecture gaps at load time
(`archfeat::gap_report`) but only ever printing them to stderr — nothing carried the model's own
arch string or those gap lines back into the app.

* **`Model.arch: String`** and **`Model.gaps: Vec<String>`** — captured at load, from the exact
  `archfeat::gap_report` lines already computed (not recomputed elsewhere, so they can never drift
  from what the operator's terminal showed). Both `Model` construction sites (the real GGUF loader,
  and the int8-resident-cache fast path) are updated; the cache path gets empty `arch`/`gaps` since
  that format predates this and doesn't serialize an arch string.
* **`Engine::model_status()`** (behind `wgpu-engine`) converts this into
  `logscope::liaison::ModelStatus { name, arch, gaps }`.
* **`Backend::model_status()`** — new trait method on the `dyn Backend` object `chat.rs` actually
  holds, defaulting to `None` so the llama.cpp/daemon paths correctly report "nothing to say"
  rather than a fabricated "no gaps"; overridden on `Wgpu`.
* **`LoadInfo.model_status`** carries it through the existing one-shot `Evt::Loaded` event (both
  `Cmd::Load` branches — a fresh load and an already-resident model — populate it).
* **`Chat::apply_evt`**'s `Evt::Loaded(Ok(info))` arm posts each gap line to
  `self.hud.observe(format!("{}: {gap}", status.name), "loader")` when the model isn't fully
  supported — reusing the HUD's existing grounding/dedup/provenance machinery rather than adding a
  second display mechanism.

Tested end to end (not just per-piece) in `chat.rs`'s
`a_model_load_with_declared_gaps_reaches_the_hud_and_a_clean_load_does_not`: a model with gaps
produces a HUD line naming the model and the gap; a fully-supported model produces none; and a
backend reporting `None` (no data) also produces none — the three-way distinction the Liaison's
"never invent" rule requires is enforced by the test, not just documented.
