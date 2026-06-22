# Partial test — replaying a real build conversation through logscope

A first, compile-gradeable slice of the "feed a repo + its own conversation back through
logscope" idea, using the **game-engine** domain (`unendlich-engine`) rather than the
data-oriented logscope repo.

The arc replayed is the conversation's opening task: *"batch-wire the 1K+ orphan modules
into bin `unendlich-fullgame`"* (commits r112→r113, landed by r116c). The real answer is a
`service_registry_batch.rs` that wires 19 tick-shaped stateful services into a
`ServiceHost` with neutral-default constructors and panic-free ticks — so the task
decomposes cleanly into one sub-task per adapter, which is exactly what `:fill` is for.

## Files

- `batchwire.chat.md` — the seed: the real ask + the registry contract + one worked adapter
  (`fabric`), primed so the assistant is ready to fill.
- `service_registry_batch.skeleton.rs` — the target: all 19 real constructors preserved,
  18 `tick` bodies holed to `todo!()` (each with a one-line hint of the API it should call).
- `05-batchwire-fill.logscope` — single `:fill` pass. ~40 nodes, ~10 min. The compile test.
- `06-batchwire-explore.logscope` — `:fill` then a fan-out round. ~250 nodes, ~1 h. The
  exploration run; scale W/D for a marathon.

## How to run

```bash
# build the headless runner once (from the logscope-suite repo):
cargo build --release --bin logscope-cli --no-default-features --features wgpu-engine

# 1) shape-check off-GPU first (confirms branch shape + node counts, no model time):
./gen-runs.sh range 5-6 model mock subdir ./runs/dryrun/

# 2) calibrate on 05 with the real model, read seconds/nodes for per_gen:
./gen-runs.sh range 5-5 maxtokens 1536 model ~/models/DeepSeek-R1-Distill-Llama-8B-Q4_K_M.gguf \
              system '' subdir ./runs/cal/

# 3) size 06 from the measured per_gen, then run it.
```

## Node math

| script | stages | reply-pairs | ≈ nodes | ≈ wall @15s |
|---|---|---|---|---|
| 05 | user turn + `:fill`(18) + `:assemble` | ~20 | ~40 | ~10 min |
| 06 | `:fill`(18) + `:1fa 6` (18×6) | ~125 | ~250 | ~1 h |
| 06 → `:2fa 6` | `:fill` + 18×(6+36) | ~775 | ~1,500 | ~6 h |
| 06 → `:3fa 6` | `:fill` + 18×(6+36+216) | ~4,660 | ~9,300 | ~a day |

Use the **measured** per_gen, not 15 s — calibrate on 05 first.

## How to grade ("see how it compiles")

1. Drop the assembled file in at
   `crates/unendlich-engine/src/sim/service_registry_batch.rs` and:
   ```bash
   cargo check --release -p unendlich-engine --features render
   cargo test  --lib service_registry          # registry_wires_and_ticks_all_services
   ```
2. Diff the model's output against the ground-truth (the real committed file — I have it and
   can drop it in as `service_registry_batch.GROUND_TRUTH.rs` if you want an automatic diff).
3. **Watch the known-hard holes.** The conversation itself records that a generator produced
   wrong adapters for two of these, hand-fixed afterward:
   - `fluid` — generated version **indexed out of bounds**.
   - `moisture_field` — generated version **violated grid preconditions**.
   Plus the two with real control flow: `projectile` / `summon` must detect expiry from the
   step return and re-construct via `fresh()`. These four are the interesting failure
   surface for an 8B model — the "different surprises" a game engine throws vs. data code.

## Provisional — validate before a real run

These scripts were written from the grammar **table in BOOTSTRAP.md**, not from the logscope
source. Before a real (non-mock) run, confirm in the logscope-suite chat against
`crates/logscope-egui/src/runner.rs` (`run_command`), `src/script.rs`, and `src/decompose.rs`:
- `:fill <path>` accepts a file path and holes on `todo!()` / `unimplemented!()` as assumed;
- `:open` on a `.chat.md` leaves the active leaf where the next `>` / `:fill` should attach;
- `:1fa 6 'tmpl'` parses as D=1, W=6 with `{lastmsg}` substitution.

If the code disagrees with the table, the code wins — adjust and re-run the mock pass.

---

# The arc ladder (07 → 99)

Several basic arcs, a few medium, one long marathon — all replayed from the **real
commit spine** (`r116c..r286`, 60 builds), so the task sequence is the engine's actual
build history rather than invented work. Built to probe context fragmentation and pickup
across length.

| script | tier | ≈ nodes | ≈ wall @15s | what it stresses |
|---|---|---|---|---|
| `05-batchwire-fill` | basic | 40 | 10 min | compile gate (the anchor) |
| `07-early-features` | basic | 40 | 10 min | short linear context + small widen |
| `08-survival-bending` | basic | 36 | 9 min | widen + shallow deepen |
| `09-shaders-mini` | basic | 50 | 12 min | wider fan |
| `06-batchwire-explore` | medium | 250 | 1 h | fill + fan over the holes |
| `12-pickup-stress` | medium | 268 | 1.1 h | **many long-distance pickups** |
| `10-perf-threading` | medium | 832 | 3.5 h | depth + one pickup + widen/deepen |
| `11-menu-egui` | medium | 1,072 | 4.5 h | **mid-run re-seed (context break)** + fan |
| `99-fullspine-marathon` | big | 6,640 | ~28 h | the whole spine; knob to ~2 days |

## How the real history maps onto fragmentation / pickup

- **Pickup = the fix commits.** The spine's 15 fix builds (`r116c`, `r133b`, `r141b`,
  `r151c`, `r157b`, `r176b`, `r180b`, `r182b`, …) are development literally jumping back to
  patch an earlier state. Each becomes a `:goto <earlier node>` followed by the fix turn, so
  the model must reconstruct context from a distant node, not the current frontier.
  `12-pickup-stress` does nothing but this; the marathon sprinkles it throughout.
- **Fragmentation = fan bursts + re-seed.** `:fa`/`:ca` split the tree into many divergent
  contexts off a shared KV prefix; the marathon fires a widen+deepen burst on every heavy
  build (shaders / ecology / sim / perf), then `:goto`s back to the main line so length
  compounds in controlled bursts instead of running away. `11-menu-egui` and the marathon
  also `:open` the batchwire seed mid-run — an unrelated subtree grafted in — to see whether
  the graph picks the spine back up cleanly afterward.

## Marathon length knob

`99-fullspine-marathon` defaults to a HEAVY block of `:3fa 5` then `:2ca`, landing ~a day.
Change that one block to scale the whole run:

- `:2fa 5` + `:2ca` → ~1,250 nodes (~5 h)
- `:3fa 5` + `:2ca` → ~6,640 nodes (~a day)  ← default
- `:3fa 6` + `:3ca` → ~12k+ nodes (~2 days)

The basic/medium arcs are independent — run any subset. Calibrate `per_gen` on `05` first;
all wall-clocks above assume 15 s and should be recomputed from the measured number.

## Provisional notes specific to the ladder

- **`:goto` node ids are computed assuming generated replies are numbered sequentially from
  1** (fan-generated nodes included). If the runner numbers differently, the ids will be off
  — do a `model mock` pass, `:export ascii`, and reconcile the `:goto` targets against the
  real ids before a live run. The comments name the intended target build so re-pointing is
  mechanical.
- Node counts assume `:Nca` deepens every leaf linearly (`+L·N` pairs) and `:Dfa W`
  multiplies (`L·ΣW^d`, new `L=L·W^D`), and that a `>` turn attaches once to the active leaf.
  Confirm these against `vimmode.rs` (count/arg parsing) and `runner.rs`.
- After `:fill`, the counts treat the frontier as re-collapsed to one spliced leaf. If
  instead the H hole-replies stay as separate leaves, a following `:fa` multiplies by H and
  counts grow accordingly — check `decompose.rs` / `:assemble` behavior.
