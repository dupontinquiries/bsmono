# Work plan: the five threads, expanded

Your list, written out as specifications rather than shorthand — with what each actually requires, and
what has to be measured before it can be built.

## 1. Qwen3.5-30B and Scout: tok/s

**The observation** was "a mixed bag of compute and submit and sync". That is a profile with three
roughly equal terms and no dominant one, and it is the hardest shape to act on: attacking the largest is
wrong when the largest is a *symptom* of another term, and wrong entirely if the run is already at the
memory roofline.

**Done this turn:** `perfverdict` resolves the three numbers into one named bottleneck — or says the
split is irrelevant because the run is at the bandwidth limit. The roofline check runs *first*, since a
run at the physical limit has no bottleneck to fix and every later branch would name one anyway.

**Next, in order:**

1. **Wire `perfverdict` to the profile print.** It needs `active_bytes` (dense + routed experts, which
   the loader knows) and `gib_per_s` from `--gate-perf`. Without the second it returns efficiency 0 and
   says so rather than guessing.
2. **Re-measure Qwen3-30B after _345.** It fits 48 GiB resident and was streaming for no reason; the
   residency check now exists. The expected change is submits/token dropping by an order of magnitude,
   and if it does not, `perfverdict` will say which term actually binds.
3. **Scout needs `LOGSCOPE_MAX_GPU_GB≈56`** to go resident — 53 GiB of weights against a 48 GiB constant
   and a 47 GiB derived budget. Whether 53 plus KV fits inside the measured 63 is untested.

**The known unknown:** the subgroup matvec is wrong or unsupported on three drivers now. It is guarded
off everywhere it fails, so it costs speed rather than correctness — but the guard means nobody is
measuring what the fast kernel would give.

## 2. Patch mode

**What it should be:** the model emits edits against line-numbered source rather than reprinting files.
`liaison_relay` already implements this — `Edit::{Replace, InsertAfter, Delete}`, `number()`,
`parse_edits()`, `apply()` bottom-up, out-of-range fails loudly.

**The gap is the same as everywhere else this session:** it has no caller in the chat path. Wiring it
means: number the file when it goes into the prompt, parse `@@line kind` out of the reply, apply, and
show the diff rather than the reply.

**The failure to guard:** an edit that applies cleanly to the wrong line is silent. `apply()` failing on
out-of-range is necessary but not sufficient — the stronger check is that the *anchor text* at that line
matches what the model was shown, which `liaison_relay` does not yet do.

## 3. Git history for imported projects

**The trigger:** a workspace imported from a folder containing `.git`. That is a cheap detection and it
should change what the model is offered, not just what it can be asked.

**`repoquery` already exists** and is exactly this: `CommitsTouching`, `CommitsMatching`, `ShowCommit`,
`BlameRange` — structured query shapes, fixed argv, no shell, hostile paths refused rather than
sanitised. Written in _293, **zero callers**.

**What wiring requires:**

1. Detect `.git` at import; set a flag on the workspace.
2. When set, include `FETCH_PROTOCOL` in the liaison's system turn so it knows the shapes exist.
3. Run the argv, wrap the result in `Evidence` — which already carries "this is a SLICE of the
   repository, not its history" so a fetched fragment is not read as the whole record.
4. Use the two-step preview from _295 for large results.

**Why this matters more for an imported project than a new one:** the model's default is to invent
history it cannot see. On a fresh workspace that is harmless; on a project with five years of commits it
produces confident claims about decisions it never read.

## 4. Physical UI

**Recorded as pending from an earlier session:** the physical dock should be a *mirror* of state, not a
third writer to it. Two writers already exist (the picker and the deployment), and _290 fixed a class of
bug caused by exactly that — the write-through in `activemodel` was needed because two sources of truth
disagreed.

**So the specification is a constraint, not a feature:** the dock reads and issues intents; it never
holds state that anything else also holds. Adding a third writer would reproduce the bug the second one
caused.

I do not have enough context on the dock hardware to go further than that this turn, and guessing at it
would be worse than asking.

## 5. Distributed compute (continuing)

`workerkit` decides which duties may cross a network; `trustkit` filters by registration before ranking;
`buildfarm` spreads multi-target checks by measured speed. **All three assume a transport that does not
exist.** OpenDHT integration is the blocker and is a dependency decision rather than a refactor.

Order: transport → discovery → model ranking across peers → menus. Menus last, deliberately: a menu for
a capability that does not exist is what made this look closer to working than it was.

## The pattern worth naming

Four of these five threads are blocked on the same thing — **code that exists, is tested, and has no
caller.** `liaison_relay`, `repoquery`, `hybridsplit`, `tensorplace`, `coresidency`, `cpudecode`,
`prechecks` until this session. The building is not the bottleneck; the wiring is.
