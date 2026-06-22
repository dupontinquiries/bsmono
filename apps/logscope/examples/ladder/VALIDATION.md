# Validation of the generated ladder (against the real runner)

These scripts were generated in a separate chat from `BOOTSTRAP.md` and validated here by mock-running them
through `logscope-cli` and reading `runner.rs` / `vimmode.rs` / `script.rs`. Two real fixes came out of it;
the scripts run, with the caveats below.

## Fixes this validation produced (already in the code)

1. **Inline comments on command lines now parse.** `:goto 13   # pickup r133b` used to fail (`:goto` parsed
   its *whole* argument as a number → "Unknown" → silently skipped), so every pickup in `10` and `12` —
   the entire point of `12-pickup-stress` — was a no-op. `script.rs` now strips a trailing `# comment` from
   command lines (quote-aware, so a `#` inside a `:fa` template survives).
2. **`:fa` / `:ca` are scoped to the active subtree.** They used to fan *every* leaf in the whole tree, so
   the marathon's "burst then `:goto` back to the main line" did not contain anything — each `:3fa 5` burst
   re-fanned the entire accumulated frontier and the run exploded toward millions of nodes (it timed out).
   Now a burst only widens the leaves under the active node, so `:goto` actually scopes it. After this
   change the marathon completes (~4.6k nodes) and `10`/`12` drop to controllable sizes.

## Mock node counts: predicted vs. actual

| script | predicted | actual (whole-tree fan) | actual (subtree fan, current) |
|---|---|---|---|
| 07-early-features | ~40 | 40 | **40** |
| 08-survival-bending | ~36 | 28 | **28** |
| 09-shaders-mini | ~50 | 50 | **50** |
| 10-perf-threading | ~832 | 1142 | **582** |
| 11-menu-egui | ~1072 | 697¹ | **697**¹ |
| 12-pickup-stress | ~268 | 976 | **196** |
| 99-fullspine-marathon | ~6640 | (explodes / timeout) | **4602** |

¹ `11` and `99` `:open batchwire.chat.md`, which is **not in this folder** (the seed lives with `05`/`06`,
which weren't uploaded). The re-seed step is skipped, so those counts omit the grafted subtree.

## Corrected node-count rules (the real semantics)

- A `>` turn adds **2 nodes** (user + assistant) as a child of the active leaf.
- `:Dfa W` over `L` *active-subtree* leaves adds `L·(W + W² + … + W^D)` **reply-pairs** (×2 nodes), and the
  active subtree ends with `L·W^D` leaves.
- `:Nca` over `L` active-subtree leaves adds `L·N` **nodes** (one assistant child per leaf per round — *not*
  pairs; this is why `08` came in at 28, not 36).
- `:goto M` + `>` adds a 2-node branch off node `M`, which persists as its own leaf.

## `:goto` node ids — the mapping to verify

Nodes are numbered in creation order, 1-based as shown by `:export`. For a linear run of `>` builds,
**build #k's user turn is node `2k−1`, its reply is node `2k`.** So `:goto 13` lands on build #7's user turn.
The ladder's `:goto` targets were computed from a node model that (a) predated the comment+scoping fixes and
(b) for the marathon assumed the explosion, so its large targets (`:goto 421`, `827`, `1243`, …) now point
past the end of the (smaller) tree and **clamp to the last node**. Before any live run that relies on
pickups landing precisely: `model mock` the script, `:export ascii`, and reconcile each `:goto` against the
real ids (the comments name the intended build, so re-pointing is mechanical — `:goto 2k` to attach under
build #k's reply).

## Status per script

- **07, 08, 09, 12, 10** — self-contained (`>` + commands only); run as-is. `10`/`12` use `:goto`; verify the
  pickup ids as above if you care where the fixes attach.
- **11, 99** — need `batchwire.chat.md` (and `99` is a marathon whose `:goto` ids need recomputation). Drop
  the seed in, or remove the `:open` line, before a live run.

## How to run (unchanged)

```bash
./gen-runs.sh range 7-10 model mock subdir ./runs/dry/        # shape-check off-GPU
./gen-runs.sh range 7-12 maxtokens 1536 system '' \
  model ~/models/DeepSeek-R1-Distill-Llama-8B-Q4_K_M.gguf subdir ./runs/ladder/
```
