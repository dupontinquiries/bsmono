# Work from other sessions that never reached this repo

Found by reading the last twenty conversations and checking each against the tree. These are
not TODOs — they are work that EXISTS somewhere and is not here. Each needs a bundle from the
session that made it; none can be recovered from this side.

## 1. studio-live: voice call, reverb/delay, effect automation — 17 commits

Session "Collaborative studio workflow for Linux", on top of c17's `85ad685d`. Its own closing
words: *"Bundle not re-exported — say the word and next turn I'll `git bundle` the branch."*
Nobody said the word.

Confirmed absent here: `studio-fx/src/space.rs` (Freeverb reverb + ping-pong delay) and
`ParamRide` / `AutoTarget::BusParam` (effect-parameter automation) appear nowhere in the tree.
`studio-collab/src/voice.rs` DOES exist, so the voice-call work may be partly landed — worth
diffing rather than assuming either way.

Reported green at the time: studio-core 75, studio-collab 97, studio-fx 177+5, studio-audio
109, studio-app 225. Also carries a real fix: `studio-media --no-default-features` didn't
build because the opus `cfg` sat on `mod flac` (`e9b6545c`).

**This is the largest orphan and the one to ask for first.**

## 2. crunch — the SpeedCrunch port — 4 commits

Session "Rust calculator with mobile UI and graphing features", `crunch-on-c17.bundle` against
tip `5db8a71e`. `apps/crunch` does not exist here.

114 tests reported: 92 unit, 10 interaction (real click/drag), 5 snapshot, 7 `uxaudit`. Notable
because it is the only known caller of `uikit_egui::uxaudit` outside the kit — the tool this
session built a loop around. Its live-testing method (xdotool against a real Xvfb+openbox
window) is also not used anywhere in this tree.

## 3. smite2-rust — the wgpu demo with clawbound ancestors

Session "Replacing trademarks in wgpu demo with clawbound characters". Nothing matching
`*smite2*` is in the tree, and `games/clawbound` is here but has no wgpu demo beside it. Was
worked on in `/home/claude`, likely never bundled at all.

## 4. Analysis with no code, worth not re-deriving

* **The AVD / LLM-on-video-decoder review** ("Building LLMs on video decoder hardware"). The
  conclusion was that the plan proposed measuring things this repo has already measured, and
  it named four things here that ARE worth finishing — `zerocopy.rs` (import mmap'd GGUF pages
  as `VkDeviceMemory`, marked draft and unrun), `LOGSCOPE_DIFFUSION_TAU`, the learned
  entropy-decode speculation note in av1tuner, and git-history-as-corpus via gitmind. That is a
  ready-made shortlist nobody has picked up.
* **The SMITE-on-Asahi tuning list** — DXVK/FEX/muvm settings, ordered by expected payoff. Not
  code, but the kind of thing that is expensive to reconstruct.

## 5. Already recovered this session
`aperture-entities` (merged), the logscope `SESSION_CHANGES.md` (merged), vidkit (merged),
unendlich (merged). `card-diagnoser` needs prerequisite `c7339651` — and that hash appears in
the "compositor-maybe-gitmind-work" session, which is where the nfc-kit work lives, so the
re-export has to come from that machine.

---

## Merge attempt log: blackstar-024-delta (2026-09-08)

Attempted, ABORTED, tree left clean. Recorded so the next attempt starts ahead of this one.

49 commits, 32 files, 8 touched on both sides, 3 conflicts:

| file | conflicts | shape |
|---|---|---|
| `games/blackstar-rs/src/main.rs` | 1 | two independent CLI flags (`--vo` vs `--cine`) added at the same spot — **keep both**, resolved cleanly |
| `games/shared/worldhost-unendlich/src/lib.rs` | 3 | **not uniform** — see below |
| `unendlich-engine/src/world/gpu_mesher_vk.rs` | 0 | auto-merged |

**The trap.** The first conflict in `worldhost-unendlich/src/lib.rs` is a doc comment where
their side is strictly better: a `Status (2026-09-02): COMPILES` block and a KNOWN LIMITS
section written after a real compile, which is newer evidence than the prose on this side and
must not be dropped. I applied "keep both" to all three on that basis and it broke the build —
`unclosed delimiter`, because at least one of the other two is inside
`impl<S: VoxelSource> WorldHost for Host<'_, S>` and is CODE, where keeping both sides
duplicates or orphans a brace.

So: resolve the first by keeping both, and read the other two individually. Do not batch them.

`cargo check -p worldhost-unendlich` is the gate — it caught this in one run.

The branch is fetched locally as `refs/remotes/z2-c14unified-024-delta/head`; nothing needs
re-uploading.
