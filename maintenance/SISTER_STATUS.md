# Sister-conversation status — divergence vs. the kits

Snapshot from the 2026-06-27 hand-off (`aaaa_tar.zst`): the latest state of the
five active app conversations, how each relates to the shared kits, and what got
synced into the monorepo.

## Headline

**None of the app conversations have adopted the shared kits yet.** The kits are
newer than the app work, and each conversation has been evolving its app
independently. So the kits and the apps have *diverged*, and the **app→kit
repoints (in this monorepo) are the bridge** that connects them. The plan:

1. sync each monorepo app copy up to its conversation's latest (done — this commit),
2. repoint the apps onto the kits wherever there's a clean consumer (next commit:
   audio-kit + voice-kit),
3. leave the rest as documented follow-ups (below).

The apps remain standalone (no monorepo workspace, no monorepo-specific
adaptations), so syncing was a safe source overlay.

## Per-app

### unendlich — r472 (synced) · USED THE KIT TOOLING ✅
The standout: this conversation took our exported `kitfinder.bundle`, built it
(~26 s, zero-dep), and ran it on its own tree. It surfaced `populate_debug_scene`
(1476 tokens) and `format_workload_summary` copy-pasted across
`unendlich_fullgame.rs` and `unendlich_world.rs`, and that drove real refactors:
- a `WorldSession` abstraction whose `reset()` rebuilds the whole struct, so a
  per-world field can never be forgotten — structurally kills the multi-world
  leak-bug class (r467/r468);
- deduping the two copy-pasted fns into the lib, with a kitfinder re-run confirming
  clone mass dropped 50,372 → 48,188 tokens (r469);
- new blocks + a HighlandSteppe biome (id 28), and porting CPU biomes 19–27 into
  the GPU shader (they weren't generating on GPU before);
- a water sun-glitter shader pass (r472).

FLAG — `unendlich_world.rs` has pre-existing input-API bit-rot (WindowEvent vs
InputEvent, Key vs Keycode, ~lines 1283/1316/1352) and shares most of its
world-setup with `unendlich_fullgame.rs`. The conversation's open question is
whether to **retire it** in favour of fullgame (eliminating the duplication class)
rather than maintain two binaries. Recommend retire.

KIT ANGLE — unendlich's noise/worldgen RNG is exactly the splitmix/LCG that
core-kit's `rng::SplitMix64` now canonicalizes. A future *opt-in* repoint (it
changes the generated sequence, so it's a watched migration, not a blind swap).

### rusty-board — synced · SWIPE FIX LANDED (matches our breadcrumb) ✅
Diagnosed swipe-typing-doesn't-fire as a state-driven misroute: the segment loop
re-derives `dispatch_mode` from `current_layout` and bails to
`dispatch_cardinal_swipe` if that's stale, silently dropping a valid swipe. Fix =
warn-and-continue (trust the category resolved at drag_begin; warn loudly on
disagreement but never drop). Two files (`gestures/boundary.rs`,
`ui/keyboard_view.rs`), now synced. This is a runtime-logic fix and is *separate*
from the historical "good swipe core" breadcrumb (rusty-board commits
876a0fa..7d3efea) — that remains the lead for a future swipe/touch-kit.

KIT ANGLE — voice path repointed onto voice-kit next commit (hallucination filter).

### bo-suite — 2026-06-19 (synced)
Added equal-power-pan + width spatializers (`realtime_mixer`, `channel_routing`)
and a `queue_policy.rs` enqueue-mode abstraction (9 tests). Qt-blocked DSP
validated via standalone rustc harnesses; bo-mux 323/323.

KIT ANGLE — `audio_features.rs` (byte-identical in bo-mux and bo-controller) and
the podcast-diar RMS repoint onto audio-kit next commit. This is the headline
consumer — the 27 K-token duplicate audio-kit was built to absorb.

### voiceterm / web2qml — v0.1.879 (synced)
KIT ANGLE — `separation.rs` RMS repoints onto audio-kit next commit. Its
hallucination filters (`trim_hallucination_loop`, `looks_like_speech`) are embedded
in the large `voice.rs` rather than a module; voice-kit already lifted them (the
union of rusty-board's + voiceterm's), so repointing those is a follow-up — and the
right place to fix the loop-trimmer off-by-one *at source* (the exclusive-bound
case voice-kit currently preserves faithfully).

### snapclone — feed-player (synced, C++/Qt)
UI work: bottom-left mid-call "+" add-people button, contact-picker cue, live-feed
tiling. Not a Rust-kit consumer; net-kit/share-kit are the eventual Rust-side
counterparts if/when the C++→Rust port (backlog C1) happens.

### gitmind — synced
Adds a `check` subcommand (`cmd_check`; gitmind-core `checks`/`native`/`portage`,
`--candidates` lists green commits) plus a new `peek` crate. The synced zip already
contains `cmd_check`; `patches/gitmind-per-commit-check.patch` is kept for
provenance.

## Reconcile follow-up
The overlay sync left a few "only-in-monorepo" files in the non-repoint apps
(unendlich 10, logscope 1, gitmind 1, snapclone 2) — older modules or monorepo
notes. They're harmless (the synced Cargo manifests control what builds), but a
reconcile pass should delete the genuinely-retired ones (e.g. once
`unendlich_world.rs` is retired).
