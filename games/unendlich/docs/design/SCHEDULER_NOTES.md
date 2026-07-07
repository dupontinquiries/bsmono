# Scheduler ideas (r197) — rollback netcode + tick priority

Ideas that fell out of wiring the DHT join + dual mailbox this turn, for
the rollback-default multiplayer the playtesters will use.

## 1. Input-mailbox as the rollback scheduler's clock
The dual mailbox (mp_dht::Mailboxes) is not just transport — it IS the
rollback scheduler's input queue. Each peer's box holds remote INPUT
packets stamped with the tick they belong to. The scheduler rule:
  - simulate forward optimistically using PREDICTED remote inputs
    (repeat-last-input is the cheap default);
  - when fetching the mailbox yields an input for tick T < current,
    roll back to T, replace the predicted input with the real one,
    re-simulate to now. The engine already has rollback points (r186)
    and the r196 ramp reconciliation to smooth the visual snap.
PRIORITY: fetch + apply remote inputs BEFORE advancing the local tick, so
the rollback depth is minimized (you correct as early as possible).

## 2. Confirmation horizon (bounded rollback)
Track the oldest tick any peer might still send an input for = the
"confirmation horizon." Ticks older than the horizon are FINAL — their
rollback points can be freed and their state checkpointed. This bounds
both memory (only keep rollback points back to the horizon) and the
worst-case rollback distance. The horizon advances as every peer's
mailbox confirms through a tick. A peer that stalls drags the horizon;
cap the rollback window and drop a peer that exceeds it (rejoin via DHT).

## 3. Priority lanes for the local scheduler (player-first)
The r196 operation classes (task_lanes P/C/S) get a PRIORITY ordering
when the frame is behind budget:
  1. player input + local-player physics (never deferred — responsiveness);
  2. rollback reconciliation (correctness; bounded by the horizon);
  3. Class-P bulk (mesh, worldgen, AI) — work-stolen, deferrable;
  4. Class-C clone+apply (plant/leaf lifecycle) — already strided (r190);
  5. cosmetic (particles, distant sway) — first to drop.
Operator's "prioritize player movement then rollback reconciliation"
maps exactly to lanes 1 then 2. The autotuner (r184) already sheds 3-5
under load; this just fixes the ORDER so 1-2 are protected.

## 4. Tick-authority blend for mixed sessions (from tick_authority doc)
In a rollback session every peer is authoritative over its own inputs;
in a client-server session the host is authoritative over state. The
NetMode (r197) selects which scheduler runs. A future BLEND: regional
authority — a peer that owns a region runs the fast local scheduler
there and ships confirmed state to others, who treat it as client-server
for that region. The mailbox topic field already namespaces this
(topic = region id); the scheduler picks rollback vs apply-remote per
topic.

## 5. Deterministic fetch order (replay safety)
When draining a mailbox with inputs from multiple peers for the same
tick, apply them in a FIXED order (sorted by NodeId), so two peers
replaying the same mailbox contents reach the same state. This is the
network analogue of the r190 sorted-chunk-coords determinism and the
r196 map_pure input-order merge. Without it, rollback re-sim could
diverge on input-application order — a desync that looks like a physics
bug. The harness (headless_run) gains a "two-peer replay" mode to gate
this: feed both peers the same input log, assert identical digests.

## Status
Ideas 1-3 are directly buildable on what shipped this turn (Mailboxes,
NetMode, task_lanes, rollback points). Idea 5 is a harness extension.
Idea 4 is the largest (regional authority) and waits for basic rollback
sync to land first. None require new dependencies.
