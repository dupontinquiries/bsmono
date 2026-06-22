# Arena challenges

Small, self-contained Rust crates with deliberately-broken-but-fixable tests, for
exercising the **Local** backend with a real test gradient (instead of the
closed-form Sim or a heavyweight real project that may not compile under shelter).

Each crate **compiles cleanly** — the failures are one-token logic errors that the
arena's mutation operators can repair:

| crate        | baseline | defects fixable by            |
|--------------|----------|-------------------------------|
| `warmup`     | 2/5=0.40 | tweak-num, flip-op, flip-bool |
| `arithmetic` | 1/4=0.25 | math-op, tweak-num            |

## Use

Point the arena at one of these as the workspace, backend = **Local**, and turn
on **recurrent_steps** (hill-climb) so a bot accumulates fixes across steps:

    workspace = challenges/warmup
    backend   = Local
    fast_clone = off       # these aren't git repos; use a full copy
    recurrent_steps = 12

A clean run should show baseline > 0 and best climbing toward 1.0 as bots repair
the seeded defects — the first time the whole pipeline produces a *real* signal.
