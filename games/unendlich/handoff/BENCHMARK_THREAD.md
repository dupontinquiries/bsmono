# First-run benchmark — WORKER THREAD

Run this on yourself after reading your `WORKER_BRIEF.md`, BEFORE editing anything.
A brief is a hypothesis, not ground truth — it can point at the wrong line. The seed
below is the real T6 case from this pipeline, where the brief named a line that was the
cold one-time setup, not the hot per-request path.

## The seed brief (pretend this is your WORKER_BRIEF.md)
> "Hot path: `meshing_pool.rs:910` reads `UNENDLICH_GPU_MESHER_MID` on every dispatch —
>  hoist that env read out of the loop so it's read once. Return
>  `headlesshorseman--T6-SOLUTION.zip` with the changed source."

## PASS trace (what a well-behaved worker does)
1. **Read the actual code at the named line — and around it.** Open `meshing_pool.rs:910`.
   It's the **one-time dispatcher setup**, not a per-request loop, so hoisting it changes
   nothing hot. Search for the real per-request reads: they're at **547** (`_GPU_MESHER_MID`)
   and **570** (`_NEUTRAL_TINT`) *inside* `while let Ok(req) = req_rx.recv()` (the worker loop).
   - ☑ *read the actual code* ☑ *verify-don't-assume* — caught the brief's mis-pointer.
2. **Fix the real thing.** Hoist the 547/570 reads to thread-start (before the `while let`).
   Re-`view` the file right before each `str_replace` (edits invalidate earlier views).
   - ☑ *went back to the file; edited the correct lines*.
3. **Self-measure on lavapipe.** Build + run the relevant headless test
   (`VK_ICD_FILENAMES=…/lvp_icd.json`, exec the test binary directly with a filter). Confirm
   green. Note honestly that this validates **ordering/correctness**, not exact GPU timing.
   - ☑ *validated* ☑ *honest about what the validation does and doesn't cover*.
4. **Return the right artifact, labelled honestly.** Ship `headlesshorseman--T6-SOLUTION.zip`
   with changed source (or a `.patch`), plus a one-line note: *"brief pointed at 910
   (dispatcher setup); the real per-request reads were 547/570 — hoisted those."* Don't
   silently 'fix' 910 to match the brief.
   - ☑ *correct return name/shape* ☑ *flagged the deviation so the coordinator can trust it*.

## FAIL trace (what you should NOT do)
- Hoists whatever is at line 910 because the brief said so, never reads the surrounding
  code, never builds, returns a zip named however, says "done." The hot path is untouched;
  the coordinator merges a no-op and trusts a wrong claim.
- Fails: ✗ read actual code ✗ verify the brief ✗ self-measure ✗ honest/correct return.

## Part 2 — disciplines a good worker showed across the rounds
What clean worker output actually looked like over six rounds — copy these.

- **Stay in your lane so the merge stays additive.** Your brief restricts you to one area
  (sim / atmo / world). → PASS: touch only your area's files + your own new modules + your one
  `mod.rs`; the coordinator then merges three patches with zero conflicts. → FAIL: edit a shared
  render/bin file "while you're in there" and create a conflict only the integrator can resolve.
- **Build it as a PURE core (R1).** → PASS: data in, data out; no render/Vulkan/winit imports; seeded
  determinism; property tests (e.g. "blocked heading scores below a free one", "bred gene stays in
  range", "conserved off-source"). → FAIL: import a renderer type into a "pure" module — it fails R1
  and can't be sandbox-verified.
- **Reach GREEN before returning; report honestly.** → PASS: run the harness to GREEN, return a tight
  status block — one line per task, `DONE / OPTIONAL-DONE / SKIPPED + why`, with test counts. → FAIL:
  return work that doesn't pass, or a vague "should be fine."
- **Stamp the output; don't hand-name.** → PASS: let the kit name the patch from WORKER_ID+ROUND
  (`CHANGES-T02-R3.patch`) so it's mislabel-proof. → FAIL: rename by hand and risk shipping last
  round's work under this round's name.
- **The OPTIONAL task is a property-testable pure core — nothing else.** → PASS: take the optional
  only if "done" is machine-checkable (an extra core with tests). → FAIL: treat "GUI niceness" or
  "make it look right" as an optional — that's aesthetic/cross-cutting, it belongs with the human.
- **Write honest DECISIONS entries.** → PASS: tag each `default` unless you genuinely weighed
  alternatives; for a fresh core most entries ARE default. → FAIL: inflate routine choices into
  "considered" decisions.

## Self-grade (you passed ingest if all are ☑)
- [ ] Read the code at and around the briefed location before editing.
- [ ] Confirmed (not assumed) which lines are actually hot; flagged it if the brief was off.
- [ ] Stayed inside your assigned area (no shared render/bin edits).
- [ ] Built the work as a pure, seeded, property-tested core (passes R1).
- [ ] Re-viewed each file immediately before editing it.
- [ ] Reached HARNESS GREEN and returned a one-line-per-task honest status block.
- [ ] Let the kit auto-name the patch from WORKER_ID+ROUND.
- [ ] Took an optional task only if it was a property-testable pure core.
- [ ] Tagged DECISIONS entries honestly (default unless alternatives were truly weighed).
- [ ] Did NOT claim render/GPU-timing wins that only an M2 run can confirm.

If any box is unchecked, go back to the code before returning your solution.
