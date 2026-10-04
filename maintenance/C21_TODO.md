# C21_TODO — open threads from the c21 session

Same convention as `C9`–`C20UNIFIED`: every item says how far it actually got. Session base
`7b2633a5e` (the `c9-001-fp5-eyes-thin-bar-gaze` bundle's HEAD, branch
`feat/gpu-deltanet-carry-integration-test`), all commits by the repo's last author with the
`Claude-Session` trailer. Shipped as a zip of `git format-patch` files (the chat upload cap is
20 MB; a full bundle is 422 MiB) and an incremental bundle on `7b2633a5e`. Per-commit detail is in
the messages; this file is the index, the "verified how", and what is open.

The operator's order was **logscope, then the apps, then the games**, with "performance, bughunt,
feature rollout and TODOs all good". Everything here ran on **lavapipe** (llvmpipe) in a 1-core,
3 GB, ~20 GB-disk sandbox. **No number in this session is a hardware speed.** The commits that
change what runs on the GPU say so and name the ledger row that decides them.

---

## 1. logscope (Ornith-1.5 / Qwen3.5 hybrid on the wgpu engine)

- `986941adc` **fused decode kernels on the V2 word-per-thread pattern** — RMSNORM_MATVEC_Q8,
  MATVEC_Q8_0_ADD and SWIGLU_MATVEC_Q8_0_ADD get V2Wg128's loop (every layer projection, not just
  the LM head). Default ON, `LOGSCOPE_FUSED_V2=0` restores the originals. `--gate-fused-v2`: 75
  kernel cases vs V1 and a CPU reference, plus a whole-model half that REFUSES to pass unless all
  three kernels were dispatched under both levers. **Open: the first M2 ledger row** —
  `./loop.sh bench fusedv2` against `LOGSCOPE_FUSED_V2=0` decides whether the default stays.
- `294bda73e` the fused norm+matvec "bit-identical" claim corrected (1.9e-6; it never was).
- `1de10afa3` **`--bench` counts instead of quoting**: dispatches/submits per token, prefill tok/s
  through `prefill_chunked`, bytes the engine reads, and per-kernel GPU timestamps
  (`LOGSCOPE_GPU_TIMESTAMPS=1`). **Open:** `LOGSCOPE_GPU_TIMESTAMPS=1 kernel-validate --bench
  Ornith-1.5-9B-Q4_K_M.gguf 64 512` on the M2 — the per-kernel table says where the token goes.
- `4f6dbddbd` `f9e1267ea` `fc0436784` `5355d2284` `c96a1f8f5` **tool calling and templates**:
  the taint gate scans every tool-call shape the parser reads; the grammar and the gate agree on
  where the run token goes (the fence); the agent prompt teaches the XML-parameter grammar
  Ornith/Qwen3.5/Qwen3-Coder are trained on (`LOGSCOPE_TOOL_SYNTAX=xml|json`); a reply that only
  closes a think block is reasoning up to the close, `enable_thinking` reaches the Jinja template
  (`LOGSCOPE_THINKING=0`); the agent transcript reaches the engine rendered in the model's chat
  template (`agent_render.rs`, `LOGSCOPE_AGENT_TEMPLATE=0`). **Open:** a real agent run on the M2
  with Ornith — every piece is unit-tested, none has met the model.
- `6585fb8eb` **IQ2_XS** (ggml type 17) reads, checked against gguf-py 0.19.0. `545a0fefb`
  **IQ4_XS held natively** (4.25 bpw resident, decoded in the matvec, widened to int8 on the GPU for
  prefill; fused q‖k‖v, gate‖up and the DeltaNet in-projection included; `--gate-iq4xs`;
  `LOGSCOPE_IQ4XS_NATIVE=0`). **Open:** the gated q‖gate row-gather and the MoE expert stacks still
  convert to int8 on load; `--bench <Ornith-IQ4_XS.gguf> 64 512` with and without the lever.
- `d8a35e236` `--gate-deltanet-carry` recovered (the tag `delta-carry-proven` was never an
  ancestor of HEAD); passes, max |Δ| 4.77e-7.
- `1bed58e0c` **dupscan ratchet 393 → 422, audited** — red on arrival with the c20 landing (the
  hive crate's vocabulary); all 29 are two ideas under one name. The pair to watch: fan `enlist::
  Capability` (what a machine offers) vs hive `matrix::Capability` (what a box measured).
- **The gates that read FAIL on lavapipe, classified** (`run-all-gates.sh` now carries the list):
  `714c35ab1` `--gate-q4resident` was a stale gate (it handed the loader a budget
  `gpu_buffer_budget_uma` halved below q4); `8225f2870` `--gate-qkv`'s end-to-end "bit-exact" was
  never true on a per-shape tile policy, now rel ≤ 1e-5, and `--gate-matvec` reports the subgroup
  rows as informational where the engine's self-check already distrusts the kernel;
  **`54b2bb8d1` a real kernel bug** — the subgroup matvec's partials array overflowed at tile 8 on
  any subgroup narrower than 16 (lavapipe's 8; Intel at SIMD8; a phone's Mali is 16, Adreno 64),
  and the self-check only ever tried tile 4; `0162addd7` `--gate-moe-raw`'s "1e20" was its own
  fixture (random f16 scales to 1e4, three near-identical expert tensors), the file-view path is
  right. Still red: `--gate-q4k` (two tie-straddles) and `gate-bidir-attn` column 1.
- **Not done, deliberately:** chunked-parallel DeltaNet prefill. `DELTANET_STEP_T` runs one
  workgroup per head, sequential over tokens; whether it is the prefill bottleneck is a question
  for the timestamp table above, not for a rewrite. BATCH 0 of `apps/logscope/TODO.md` is done in
  the code (repeat_penalty, the EOG set, the backend init) and its marks are stale.

## 2. apps

- `515f3e2ab` **jotter CLI** — the manifest declared `[[bin]] jotter` since 5780b2ff4 and the file
  was never committed, so every all-targets build of the crate failed. `id`/`devices`/`notes`/
  `pair [--serve]`/`scan [--pair|--save]`/`show [--png]`/`serve`/`sync`. End to end over loopback:
  pair, sync, a stranger refused; a 7-frame note as two mosaic PNGs scanned back byte-identical
  (rqrr reads version-40 symbols at 3 px a module). **Open:** a real camera photo; the LAN-address
  guess (`local_ip()` found no route here).
- Fleet sweep of `apps/` (every root manifest and every `crates/*`; `fleet_build.sh` died at
  studio when the session paused, the rest ran by hand) — the failures it found, all fixed:
  `9836670b5` settingsd's two tests raced on XDG_CONFIG_HOME; `c662dd0b9` fao-codec's damaged-file
  harness found a u64 overflow in `frame_data_end` (3 of 46,330 cases); `1fb7272aa` gitmind-core's
  delta-chunk length became a 4 GiB preallocation when corrupt; `e0160f8cf` studio-app's TCP voice
  test depended on the scheduler; `917e31044` uikit-egui's callout now falls back to a readable
  ink (slate's text is 3.3:1 on its OWN card — a palette-level finding, recorded, not fixed);
  `84950db27` `d3314b2a3` `b6f09d7ec` stale/missing locks (voiceterm's proton-srp tree, uikit-term,
  gaze-trainer); `14fc46790` gaze-trainer's phantom bin; `1db192831` drivemind; `90f12ee62`
  rusty-board's nine `llm-swipe-ffi` arms; `b90a5d5e2` board-compositor's `atspi` feature declares
  the glib it uses (found by checking the non-default configuration — the default one needs
  gtk4-layer-shell, which apt does not carry).
- **Environment, not code** (install before you gate — CLAUDE.md §3 updated): gitmind's cockpit
  tests need `openssh-client openssh-server` and `mkdir -p /run/sshd`; vidkit-player needs
  `libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswscale-dev
  libswresample-dev clang libclang-dev` (then 239 tests pass — its first run in a sandbox);
  card-diagnoser needs `libpcsclite-dev`; rusty-board's default features need gtk4-layer-shell
  (absent); voiceterm's root crate is the Qt frontend (skip).
- **`float_literal_f32_fallback`** (rust-lang/rust#154024, `future_incompatible` → a hard error
  one day, and every egui frontend here would stop compiling together): `54359106d` the tool
  `maintenance/tools/f32_fallback_fix.py` applies rustc's own suggestions and nothing else;
  `295511bdf` `8f4918f7b` `bd79b3016` `710837433` 161 sites suffixed across shared/, apps/ and two
  games. **Open:** `games/echelon/client` (one site, behind a bevy build the sweep's 20-minute
  budget could not finish), `system/`, `tools/`, and any crate whose sites hide under a non-default
  feature.
- **C20UNIFIED §9 (point the damaged-file method at other readers):** done for gitmind
  (serde/text readers, the one find was the store's delta length) and depot (tar+gzip+serde, the
  truncation cases already handled); eink-kit's parsers are defensive text parsing. Not every
  reader rewards the harness; these did not.

## 3. games

- `8cbecdcb1` `f0315e73e` `6f41f203c` `4953d3b6f` **blackstar-rs**: `rig.rs` reads
  `gltf::scene::Transform::Matrix` again (the vocabulary sweep had rewritten a library's enum to
  `the Charge`; the census floor for `Matrix` is 1 with the reason); the joints tests stop sharing
  one fixture file; **a hurt left leg reaches the gait machine** (C20_TODO's "never limps": the
  signed `limp()` was clamped to 0..1 before the sign was read); the station tests store into a
  per-thread sandbox — `cargo test` had been writing a real `blackstar.save` into the crate root,
  the player's own save if the game is run from there. 1792 tests pass.
- `c40d74388` **voxelkit**: the plan's voxel size never reached the rasterisation (every
  half-extent halved at voxel 0.5; the crate held two tests that disagreed about which semantics
  was meant, one of them never green). No consumer in the tree yet.
- `60ae7a3c4` creature-rig: the strike-overlay test read the timer backwards (never green).
- `7b857c9a1` clawbound-battleground's `mapgen_eval` example matches the six new scenery `Item`s.
- `36026f555` four games/shared locks committed.
- `6bc5378c5` **unendlich**: the subgroup cull shader leaves `src/shaders` again (08ffc680f had
  moved it to `shaders_proposed/`; the blackstar-rs rigkit merge 87795e9cd brought it back and
  both `shader_artifacts` tests failed since). `57a73b2ab` **the plant stamp budget is charged
  where a stamp is drawn** — it was charged per block VISITED, before the air-above and
  probability checks, so buried grass and the 55% the probability skips burned it and the
  surface went without tufts (c20's unconfirmed hypothesis in `docs/KNOWN-TEST-FAILURES.md`,
  confirmed). Engine lib: 9501 passed, 2 failed, 19 ignored — the two documented ones (the sag
  test, a soft-block question; the uncalibrated 90K triangle tripwire, worst chunk 155,304 at
  (-1,2,-1) seed 1, terrain-dominated per the c20 breakdown). **Open:** those two, and the
  tripwire wants calibrating against the weathered-top subdivision it keeps finding.
- Games sweep results (same commands as fleet_build.sh): see §5.
- **Not touched:** blackstar-rs's visual/playtest backlog (`TODO.md` §"Found, NOT fixed" needs
  eyes on a screen), unendlich's BACKLOG §B pure cores (octree re-merge, rebuild coalescer,
  `MoistureField::recenter`) — sandbox-verifiable but each is a feature, not a fix, and the
  engine's test crate is a 10-minute, ~3 GB build here (its sweep step hit the 20-minute budget
  with the check included; `cargo test -p unendlich-engine --lib` alone, one job, is the way).

## 4. Repo tooling

- `maintenance/tools/f32_fallback_fix.py` (above). Resolves relative diagnostic paths against the
  WORKSPACE root — the first run on a member crate opened `crates/uikit-egui/crates/uikit-egui/…`.
- The `maintenance/tools/session_capture.sh` hook is installed in this clone; six early commits
  were made from a shell that had not sourced it and carry no notes (`90f12ee6` `c96a1f8f`
  `1db19283` `d8a35e23` `b6f09d7e` `14fc4679`).

## 5. The sweep, as it stands

`apps/`: every root manifest and the four crates no parent workspace covers (hatch-core, the three
rusty-board crates) built and tested; the per-crate `apps/*/crates/*` entries of the apps before
studio were not run separately (their parent workspaces were). Failures found are all fixed above;
`bo-suite` (Qt) and `voiceterm`'s Qt root skipped; `rusty-board` tested without `wayland`.
`games/`: every root manifest and every crate no workspace covers, check + test. Green: blackstar-
demo, blackstar/rust_fsm, bsgames, clawbound (362; `--features app` checks clean too), echelon
(root + 7 members), road-warrior (+core, +shell), skydribble, soldier, speedrace, games/shared ×
13 (after the voxelkit and creature-rig fixes). Red and fixed: blackstar-rs (4 commits, 1792
pass), clawbound-battleground (the example), voxelkit, creature-rig, unendlich (shader + budget;
2 documented failures remain). Not finished here: `echelon/client` (bevy; its test build passed
the sweep's 20-minute budget — check is clean) and `unendlich`'s test build under the sweep's
budget (run by hand afterwards: engine 9501/2/19, the other eight members green).
`shared/`, `system/`, `tools/`: not swept this session.

## 6. Hardware follow-ups for the M2, in the order they decide things

1. `./loop.sh bench fusedv2` vs `LOGSCOPE_FUSED_V2=0` — keeps or drops the fused-V2 default.
2. `LOGSCOPE_GPU_TIMESTAMPS=1 kernel-validate --bench Ornith-1.5-9B-Q4_K_M.gguf 64 512` — the
   per-kernel table; decides whether DeltaNet prefill is worth a chunked kernel.
3. `kernel-validate --gate-iq4xs`, then `--bench <Ornith IQ4_XS> 64 512` with and without
   `LOGSCOPE_IQ4XS_NATIVE=0`.
4. `LOGSCOPE_MATVEC_SG=1` is worth a try now that the subgroup kernel's partials array is right —
   the Asahi guard predates both the packer fix and this one.
5. An agent run with Ornith through the chat tab: tool calls in XML, a `<tool_response>` turn, a
   think block — the pieces are tested apart and never together.
