# integrator.py — closing the backpressure loop

Worker threads never learn whether their patches landed, so they keep emitting
clean-looking islands. This is the missing feedback: a **ground-truth ledger** of
every returned action and what happened when the integrator applied/built/tested
it, plus a short **digest** prepended to every worker brief next round, led by
what's broken or owed.

## The one rule that makes it work

The `verdict` must come from the integrator **observing** (apply → build → smoke),
never from a worker's self-report. Self-reports are recorded as `unverified` and
shown as suspect — they can't masquerade as `integrated`. If you let workers fill
in their own "good/bad", you've just moved the narration problem up one level.

## Where it plugs into a round

```
end of round N:
  1. for each returned artifact, the INTEGRATOR (one owner) runs:
       integrator.py verify --id <id> --round N --worker <w> \
           --repo <canonical-repo> --patch <out/X.patch> \
           --build "cargo build --features render,window" \
           --test  "bash tests/smoke_<feature>.sh"
     (verify runs the commands and records the OBSERVED apply/build/test result)
     — or record manually if you integrated by hand.

  2. regenerate the digest:
       integrator.py digest --round N+1 --head "$(git -C <repo> rev-parse --short HEAD)" \
           --out round_N+1_state.md
       # add --worker <w> per-thread to pin that worker's own open items first

start of round N+1:
  3. PREPEND round_N+1_state.md to every worker brief (replace the previous
     INTEGRATION-STATE block; don't append — it must stay short).
```

## Why it changes behavior

A worker opening round N+1 now sees, at the top of its brief, the literal state of
its prior work — `BROKEN`, `UNAPPLIED`, `UNVERIFIED`, or `INTEGRATED` — with the
directive to fix/re-land before producing anything new. The signal that was
missing ("the repo still doesn't compile / your patch never landed") is now in
front of the agent that can act on it.

## Design choices against the failure modes

- **Observed, not reported.** Verdicts derive from exit codes; `--claimed`
  entries are flagged `unverified`.
- **Compaction-resistant.** The digest leads with the few actionable items and
  collapses the green history to a count, so it stays short and survives
  summarization. It carries a stable `<!-- INTEGRATION-STATE … -->` marker so the
  coordinator can *replace* the block each round instead of stacking copies.
- **Single writer.** One integrator owns the ledger (append-only `ledger.jsonl`)
  so there are no ledger merge conflicts and one canonical view of repo state.
- **Supersession.** `--supersedes <id…>` retires an action that a later one
  replaced, so stale work stops nagging once it's genuinely obsolete.

## Verdicts

| verdict | meaning |
|---|---|
| `broken` | applied but build/test failed |
| `unapplied` | patch won't apply to HEAD |
| `partial` | builds but not proven wired (smoke skipped / TODOs open) |
| `unverified` | only a worker claim — integrator never observed it |
| `superseded` | replaced by a later action |
| `integrated` | apply + build + smoke all green, merged |

Run `integrator.py status` for the full tableau at any time.
