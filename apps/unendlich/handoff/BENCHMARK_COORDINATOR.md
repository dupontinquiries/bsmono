# First-run benchmark — COORDINATOR

Run this on yourself immediately after ingesting the zip + reading
COORDINATOR_BOOTSTRAP.md and handoff/COORDINATOR_TODO.md, BEFORE doing real work.
It is a behaviour check, not a knowledge quiz: the point is to confirm you reach
for the docs and the compiler instead of your priors. The task below is built
from a trap this pipeline actually hit, so the "obvious" move is the wrong one.

## The seed prompt (pretend the user just sent this)
> "Loading feels slow — bump the GPU generation batch to 256 so the world fills faster."

## PASS trace (what a well-ingested coordinator does)
1. **Read the actual docs first.** Open the handoff START-HERE block + the GLOSSARY
   "Perf / stall env flags" section *before* touching code. They say the live knob is
   `UNENDLICH_GPU_GEN_BATCH` and that the `GPU_BATCH_SIZE` const is now **dead**.
   - ☑ *read the actual docs* — didn't answer from the conversation summary or memory.
2. **Verify the claim against the code, don't trust it.**
   `grep -rn 'GPU_BATCH_SIZE' crates --include=*.rs | grep -v 'pub const'` → no hits.
   Confirm the loader follows `generator.batch_size()` (chunk_loader.rs).
   - ☑ *verify-don't-assume* — proved "dead const" instead of repeating it.
3. **Pick the real mechanism + be honest about the trap.** Editing the const is a no-op.
   The fix is the env flag (`UNENDLICH_GPU_GEN_BATCH=256`, or it's already in the rd24
   profile). If a higher *default* is wanted, change the flag's default and remove/repoint
   the dead const. Tell the user their const edit (if they made one) does nothing.
   - ☑ *correct mechanism* ☑ *honest about the no-op*.
4. **If you change code, actually go back and close the loop.** Re-`view` the file before
   `str_replace` (earlier views go stale after an edit). Then: Cargo.lock-v4 workaround →
   `cargo check --features headless` AND `--features window --bins` (apt-install glfw deps
   first if the sandbox is fresh) → run the affected tests (`chunk_loader`/`chunk_manager`)
   + `harness/run.sh` → update the `runtime_flags` registry `default:` string so `--help`
   doesn't lie.
   - ☑ *compile both feature sets* ☑ *run affected tests + harness* ☑ *registry kept honest*.
5. **Roll + verify the artifact.** Correct excludes; confirm file count / key paths are in
   the zip before presenting (a surprise size change gets investigated, not shipped).
   - ☑ *verified the rolled zip*.

## FAIL trace (what pattern-matching looks like — you should NOT do this)
- Opens `constants.rs`, changes `GPU_BATCH_SIZE` 16 → 256, replies "done, the world will
  fill faster." Never read the docs, never grepped, never compiled, never noticed the const
  is unreferenced. The change is a **no-op** and the user is misled.
- Fails: ✗ read docs ✗ verify ✗ go-back-and-compile ✗ honest labelling.

## Part 2 — recurring traps from the multi-round threaded effort
These actually bit, some repeatedly. Each is a check you can fail. Format: situation →
PASS move → FAIL move.

- **Incoming artifact provenance.** Worker patches/uploads arrive labelled `-R2` (or whatever).
  → PASS: before merging, confirm the patch *contains the round-2 work* (peek the diff / byte-compare
  against the last base); a real round-2 patch from the kit auto-names from its WORKER_ID+ROUND so a
  re-run of an old zip is visibly mislabelled. → FAIL: trust the filename, merge a byte-identical
  re-upload of round 1, and report progress that didn't happen. *(This happened — the "-R2" patches
  were the round-1 work renamed.)*
- **Don't take the worker's word — re-run it.** A status block says "DONE, 6 tests pass."
  → PASS: re-run the module's tests yourself before checking it off (caught a real shipment by
  watching a test count go 19→24). → FAIL: check the box because the worker said so.
- **Not every upload belongs in the tree.** A 519 MB tarball / an `.opus` file arrives.
  → PASS: classify first — the tarball was a *different project* (reference only), the audio was a
  music asset (→ `assets/`), neither is engine source. → FAIL: try to merge or transcribe it.
- **A fix a downstream step regenerates won't survive.** The `lerp3` build break returned four times
  because the user's merge step kept re-emitting a `#[cfg(test)]`-gated copy. → PASS: fix at the
  *source* of the regenerated file (or as a post-merge step), not the output. → FAIL: re-edit the
  output each round and wonder why it keeps coming back.
- **Green sandbox harness ≠ the full build compiles.** `cargo test` (no features) was happy while
  `--features render` failed — a cfg-gating blind spot. → PASS: when a class of bug slips the
  harness, *close the gap* (add the render-cfg typecheck) so it can't recur, then keep going.
  → FAIL: fix the one instance and leave the hole open. Tell: only `lerp3` errored while `mul3`
  beside it didn't → different cfg gates, not a missing function.
- **Cheapest verifiable lever first; watch the bottleneck move.** Perf log shows a stall.
  → PASS: read the actual counters (pool 254/256 full, largest free span 33 KB, oom_fallback=3155),
  try the free no-code lever (raise the pool env) before writing code, and notice when the wall
  *moves* (pool-bound → raster-bound at RD24) instead of re-fixing the solved thing. → FAIL: assume
  the cause and start coding a fix for last round's bottleneck.
- **When your own check fails, isolate harness-error from real bug.** A smoke test failed.
  → PASS: recognise it was an artifact of *how you ran it* (copied a script out of its tree, broke
  its relative paths), then find the real constraint (run.sh must run from repo root). → FAIL: "fix"
  a non-bug, or declare the kit broken.
- **Grep for an existing implementation before building one.** → PASS: `navfield_gpu.rs` /
  `pathing.rs` already existed, so new nav cores feed them rather than duplicating. → FAIL: build a
  second system next to the one already there.
- **DECISIONS honesty (default vs considered).** When recording a choice → PASS: tag it *default*
  unless you actually weighed alternatives; mark unconfirmed constants (256 MB pool, LOD bands)
  default-with-revisit-trigger. → FAIL: dress up a default as a considered decision.

## Self-grade (you passed ingest if all are ☑)
- [ ] Opened BOOTSTRAP + GLOSSARY/handoff and used them, not memory.
- [ ] Verified incoming artifact provenance before merging (right round, right project).
- [ ] Re-ran a worker's tests yourself instead of trusting its status block.
- [ ] Classified each upload (engine source vs asset vs reference) before acting on it.
- [ ] Fixed any recurring break at its source, not at a regenerated output.
- [ ] Closed a harness gap when a bug class slipped past a green run.
- [ ] Tried the cheapest no-code lever first and checked whether the bottleneck moved.
- [ ] Grepped for an existing implementation before building a new one.
- [ ] Confirmed at least one claim by grepping/reading the code, not asserting it.
- [ ] Identified the env-flag vs dead-const distinction and said the const edit is a no-op.
- [ ] Compile-checked **headless AND window**, with the Cargo.lock-v4 workaround.
- [ ] Ran the affected tests + `harness/run.sh` (HARNESS GREEN) before claiming anything.
- [ ] Updated the registry `default:` string when a default changed.
- [ ] Verified the rolled zip (file count / key paths) before presenting.
- [ ] Labelled anything bin-only / render-side as "compile-validated, M2 run to confirm" —
      did NOT call unvalidated runtime work "done".

If any box is unchecked, re-read the relevant doc section before starting real work.
