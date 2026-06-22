# 04 — Design explorations

**Speculative.** These are explored directions, not committed spec. They're
preserved here so the thinking isn't lost, but none should be built before the
MVP slices in docs/03. Each is included because it survived the verifier test
(docs/01); the rejected ideas are not here.

## Borrowing from shell and Python — only at boundaries

The trap is "make Rust feel like Python" as a wish list (kwargs, dynamic
typing, untyped exceptions). Those fight Rust's grain and don't sit at a
boundary, so they're scope creep on Drift itself. **Refuse them.**

The filter: which Python/shell strengths are *boundary* strengths? Both
languages win because they're permissive at seams — and a seam is Drift's
domain. So keep the ergonomic win at the boundary, then add the journal so the
permissiveness is *recorded* instead of silent.

Two that pass:

- **`pipe!(a | b | c)`** — shell pipelines are a chain of boundaries; every `|`
  is a process edge where data shape can drift. A macro that wires
  `std::process` *and* journals what crossed each junction is the two-sided
  journal applied to a pipeline (stage N is producer to stage N+1's consumer).
- **`dig!(payload.user.email)`** — Python's dynamic dict access maps to the
  schema↔code boundary, where shape is owned by someone else. Returns an
  `Option` *and* records the assumed path, so a missing field becomes a journal
  entry ("consumer assumed `user.email`; payload didn't provide it") instead of
  a silent `None`.

## Drift in an interpreter, not baked compilation

The interpreted model may be the *truer* Drift. Compiled Drift checks claims
against a frozen world (the build already happened). An interpreter mediates
every boundary crossing at runtime, so:

- the journal needs **no macros** — every call is observable by construction;
- the claim becomes a **live, mutable value** you can query, revise, and
  re-reconcile mid-session — "managed evolution" made literal.

```
drift> contract create_task { returns: terminal, latency: <50ms }
drift> wire ui.add_button -> create_task
drift> run
       ✓ create_task: serviced 12ms, terminal
drift> # backend author redefines the surface, live:
drift> contract create_task { returns: pending | terminal }
       ⚠ ui.add_button still assumes `terminal`; create_task may now emit `pending`
         (contract drifted mid-session — the spinner-never-terminates bug,
          caught at edit time)
```

Honest cost: an interpreter sees only what executes (no compile-time skeleton),
so the static reachability check has no interpreted equivalent — you can't
observe a path you didn't run. Mediating every call is fine at boundaries (not
hot loops). Both pressures point at the same shape: **Drift's natural home is a
thin interpreted boundary shell that orchestrates compiled components** —
interpreted at the seams, compiled in the bodies. In that shell, `pipe!` isn't
a macro, it's a first-class live construct.

## Making code cheaper for an AI (and human) to read

The premise "AIs waste effort matching braces" is half right. An LLM doesn't
scan for `}` — braces are cheap tokens. The real costs are: (a) **long-range
scope matching** (knowing what a `}` closes means attending back to an opener
far away, less reliable as nesting grows), and (b) **you can't know what a
block does until you've read all of it.** Significant whitespace (the Python
move) helps neither and might hurt. The lever is front-loading effect so a
block can be taken on credit.

Two ideas, both **derived, never hand-authored** (authored hints rot and
violate the "boundaries only" rule):

- **Named closers** (free, formatter-inserted): `} // fn download_model`,
  `} // for task in tasks`. Restating what's closing means a reader never has
  to attend back to the opener to know what just ended. Modest, zero burden.
- **Escape-flagging + derived outline** (the real lever): what makes a block
  impossible to skip is an *escaping effect* — control flow leaving the block
  (`return`, `?`, `break 'outer`, `panic!`), state leaving it (non-local
  mutation), or time leaving it (`.await`, blocking I/O). These are the
  in-function analog of boundaries. A block with no escapes is pure in the way
  that matters — its effect *is* its summary, skip the body. So
  `cargo drift outline` renders each function/block as a one-line effect
  summary (`pure`, `mutates self.tasks`, `early-returns on Err`,
  `suspends at .await`), bodies collapsed; the reader descends only where an
  escape is relevant. This changes *reading order* — summaries first, bodies on
  demand — which cuts how much a model must hold in context far more than any
  delimiter change. Escapes are syntactically detectable, so it costs zero
  authoring.

The deepest version of the question isn't "what syntax helps a model read" but
"what lets it read in the right order and granularity" — a derived structure
layer over ordinary braces, not a change to the braces.
