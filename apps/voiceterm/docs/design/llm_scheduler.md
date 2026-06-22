# LLM resource scheduler + lifecycle

Status: **CORE BUILT (v0.1.531), pure parts tested 11/11.** Addresses
the user's report: streaming wanted, but more urgently the Ollama path
"clogs up — endless timeouts and CPU churn, models lock up against
each other, slow loads, no partial state."

## Diagnosis (from the actual code)

Two structural gaps in the current path:
1. `system_info::ollama_query_multi` is a SEQUENTIAL ESCALATION
   CASCADE: try model 1, if the response isn't a "confident" known
   action, escalate to model 2, then 3... Each call is a blocking HTTP
   request with a per-model timeout. One low-confidence query can
   become N cold-model loads run to timeout = N×timeout of work + CPU
   thrash from repeated cold loads. (The existing per-model timeout +
   session blacklist are damage control, not prevention.)
2. FOUR concurrent callers in main.rs (voice intent, block summary,
   context question, chat) hit this path from independent threads with
   NO coordination. They fight over Ollama's limited model memory —
   "locking up against each other."

## The user's key insight: device-relative sizing

"Workstations run ~10 small OR 2-3 large; an 8GB phone runs 2-3 tiny +
1 large — so large/tiny are DEVICE-RELATIVE naming." Correct, and it
forces the resource model: nothing is a hardcoded size tier; a model's
class is DERIVED from its footprint as a fraction of THIS device's
budget. The same 7B model is "large" on a phone, "small" on a
workstation.

## TWO budgets (the probe forced this out)

Calibrating against "10 small OR 2-3 large" revealed a single RAM
number can't capture it — you can hold more models RESIDENT than you
can RUN at once. So:
  - MEMORY budget → how many resident (the 10-or-2-3 rule). Effective
    cost = weights + KV cache + context + headroom (a 5GB-file model
    can cost ~8GB live — why a workstation fits 2-3, not 5).
  - CONCURRENCY budget → how many inferring simultaneously. The GAP
    between resident and running capacity is exactly where "locking up
    against each other" lives. Exceed it → QUEUE, don't thrash. This is
    the anti-lockup fix. **Memory is auto-derived up front; concurrency
    is EMPIRICALLY PROBED on first run** (v0.1.532) — the contention
    point depends on the SoC's memory bandwidth + thermals, not just
    core count, so derive() gives a safe starting underestimate and a
    throughput-knee probe refines it.

## Built (src/llm_scheduler.rs, pure, 11/11)

- ModelCost { mem_mb, slots } — caller-supplied (measured on device
  later; pure now, like mic_scheduler's InstanceCost).
- DeviceBudget { mem_mb, concurrency } + derive(total_ram, cpus):
  auto-derives a safe starting budget (reserve >=40% or 2GB so we never
  swap; concurrency ~half cores, capped 8). Tiny devices saturate to 0
  LLM budget rather than underflow.
- fit_resident / fit_running / classify_size (SmallForDevice /
  LargeForDevice / TooLarge) — device-relative, no tiers.
- plan_admission(budget, in_flight_slots, &[PendingRequest]) ->
  AdmissionPlan { admit, queue }: priority-ordered admission
  (Foreground > Interactive > Background, FIFO within a band), admits
  only what fits the remaining concurrency budget, QUEUES the rest.
  Multi-slot heavy models handled. THE anti-lockup core.
- ConcurrencyProbe + optimal_concurrency (v0.1.532): first-run
  empirical concurrency. next_level() walks 1,2,3... ; the caller
  measures aggregate tokens/sec at each (device-side); the knee
  detector stops at the level past which throughput gains drop below
  MIN_CONCURRENCY_GAIN (plateau) or go negative (thermal regression).
  result() is the concurrency to adopt. Pure policy; only the timing
  is device-side. Probe result should be PERSISTED so it runs once.

## Still to do (sequenced)

1. (live) A single-owner request gate in main.rs: the four callers
   submit to one queue; a worker pulls per plan_admission, so nothing
   bypasses the scheduler. Replaces the per-caller free-for-all.
2. (live) Replace the escalation cascade: one bounded budget per
   logical query; keep the confidence check but don't let it spend
   N×timeout. Route to the already-warm model first (warmth policy is
   testable; the keep_alive call is the live part).
3. (pure, testable) In-flight COALESCING: identical pending prompts
   share one result (extend llm_cache from done→in-flight).
   ✓ BUILT v0.1.539 (src/llm_coalesce.rs, registry 7/7). Pure Registry
   (submit→Compute/Attach, complete→fan-out count) + a thin live
   condvar layer (submit/publish/wait_for) for the actual cross-thread
   wait. Lookup order at the call site: cache (done) → coalescer
   (in-flight) → compute. REMAINING: wire into the four LLM callers
   (live — needs the request gate in main.rs).
4. (pure, then live) STREAMING: the TokenSink + SSE/line parsers
   (Anthropic format already validated against the wire spec) layer
   onto an admitted request — token-by-token UI.
5. (device) Measure real ModelCost (mem + slots) per model on M2 /
   Qualcomm / workstation to replace the placeholders.

## Device-only / deferred
The HTTP read loop, llama FFI token callback, Ollama keep_alive call,
and the cost measurement need the running build + hardware. The
scheduling/admission/classification/parsing LOGIC is pure and verified
in-sandbox (real cargo test, 121 total).
