# Drift Playground — two-sided boundary journal (MVP)

A 60-line demo of the one Drift feature that earns its keep first: a journal
that records both sides of a boundary and diffs them to localize wiring breaks.

## Run it

```sh
cargo run                          # drift OFF — the "pure" build
cargo run --features drift-trace   # drift ON — the journal
```

## What you'll see

**Off:** the program runs and the broken delete button silently does nothing.
You get zero signal — the same "no logs at all" symptom that costs a whole
debugging session to reason through.

**On:** at exit, the journal prints

```
✓  create_task:  consumer called, producer serviced        (healthy)
✗  delete_task:  consumer attempted, NO producer serviced  (BROKEN WIRE)
⚠  archive_task: producer serviced, NO consumer attempted  (DEAD SURFACE)
```

The diff between *attempted* (consumer side) and *serviced* (producer side)
is the whole trick:

| set relation         | meaning                                            |
|----------------------|----------------------------------------------------|
| attempted ∩ serviced | healthy boundary                                   |
| attempted − serviced | broken wire — consumer called, nobody answered     |
| serviced − attempted | dead surface — producer ready, nobody called it    |

## Cost when off

The `#[cfg(feature = "drift-trace")]` lives *inside* each macro expansion, so
with the feature off the recording calls and the entire journal module are
stripped before name resolution. The normal build pays nothing and stays pure.

## Honest limit

This is a *runtime* journal: it can only report boundaries that actually
executed. A path you never ran produces no entry. Catching never-run wiring
statically is a separate (harder) pass — the v2 reachability lint.
