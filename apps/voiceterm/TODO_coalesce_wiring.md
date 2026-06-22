# llm_coalesce wiring contract (module ready, NOT yet wired live)

Purpose: when N identical LLM prompts are in flight at once, run the
backend ONCE and fan the result to all N callers. Built pure + tested;
the live condvar layer is now deadlock-hardened (v0.1.556).

## API
- `submit(prompt) -> (Admission, hash)`
  - `Admission::Compute` → YOU run the backend, then MUST call exactly
    one of `publish(hash, result)` or `cancel(hash)`.
  - `Admission::Attach` → call `wait_for(hash)` or
    `wait_for_timeout(hash, dur)` to collect the shared result.
- `publish(hash, result) -> u32` — deliver result, wake waiters.
- `cancel(hash)` — computer FAILED/errored before publishing; wakes
  waiters with None so they fall back. (v0.1.556)
- `wait_for(hash) -> Option<String>` — block until publish/cancel.
- `wait_for_timeout(hash, dur)` — bounded; None on timeout. (v0.1.556)
- `is_in_flight_global(hash) -> bool`.

## THE SAFETY CONTRACT (this is what bites if ignored)
Every `Compute` admission MUST be terminated by `publish` OR `cancel`.
If the compute path can return early on error (it can — LLM calls time
out), the error path MUST call `cancel(hash)`, or attached waiters hang.
Pattern for the live caller:

```
let (adm, h) = llm_coalesce::submit(&prompt);
match adm {
    Admission::Attach => {
        // someone else is computing the identical prompt
        match llm_coalesce::wait_for_timeout(h, Duration::from_secs(60)) {
            Some(r) => return r,
            None => { /* fall through to compute ourselves */ }
        }
    }
    Admission::Compute => {}
}
// compute path — note BOTH arms terminate the hash:
match backend_query(&prompt) {
    Ok(r)  => { llm_coalesce::publish(h, &r); r }
    Err(e) => { llm_coalesce::cancel(h); format!("[error: {e}]") }
}
```

A panic in the compute path would skip both → wait_for_timeout is the
backstop (returns None after the timeout instead of hanging forever).

## Where to wire (4 live LLM callers, per the original audit)
The llm_query / llm_query_async / chat paths in main.rs. Wire chat_send_
async FIRST (most likely to get duplicate rapid sends). Each needs the
submit/publish/cancel bracket above. NEEDS the running build to verify
the fan-out actually shares (two identical sends → one worker log).

## Verified (v0.1.556, in-sandbox, REAL run)
- cancel wakes a blocked waiter with None (no hang) — thread test.
- publish delivers result to waiter — thread test.
- wait_for_timeout bails ~at the timeout, doesn't hang — thread test.
- pure registry: 7 prior tests. Total suite 147/147.
The condvar layer ran with real threads in the sandbox — this is one of
the few live-ish paths that IS verifiable here, and it passed.
