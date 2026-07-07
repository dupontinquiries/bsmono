# toolkit/harness — validation harness

The fix for "the assistant ships blind." Two layers over one ruleset.

- **`validate.py`** — pure-Python static checks (no compiler). The assistant runs
  this in its sandbox before shipping. Catches: GLSL delimiter balance, duplicate
  descriptor bindings, push-constant size, the `chunk.frag` binding + face-UV
  contract, the v0.24.90 winding footgun (re-added `flip_winding` or a flipped
  greedy test → ERROR), and staged `.spv`. Exit 1 on any ERROR.
- **`run.sh`** — orchestrator. Runs `validate.py --real` (which also invokes
  `glslc`/`glslangValidator` if installed = real GLSL→SPIR-V compile), then the
  authoritative `cargo` suite (`shaders_compile`, `mesh_parity`, `octree_parity`,
  the engine winding/contract tests, and the SSR cpu-reference tests once they
  land).
- **`RULESET.md`** — the invariants both layers enforce, including the SSR pass
  contract for the incremental build.

## Run

```
python3 toolkit/harness/validate.py     # static only (assistant + quick local)
bash toolkit/harness/run.sh             # full: static + real compile + cargo tests
bash toolkit/harness/run.sh --quick     # static + real compile, skip cargo
bash toolkit/harness/run.sh --static    # static only
```

## Loop for SSR (and anything else)

1. Assistant writes a change + (for GPU math) a CPU-reference `cargo` test.
2. Assistant runs `validate.py` here → catches structural/contract/footgun errors.
3. Assistant ships the brick.
4. You run `run.sh` → real compile + `cargo test` (incl. the new cpu-ref test) →
   authoritative pass/fail, plus the visual check in-game.
5. Failures name the rule; assistant iterates.

The CPU-reference-test pattern (see `crates/vk-harness/tests/` e.g.
`gpu_meshing_octree_cpu_ref` / `octree_parity`) is how GPU-shader math gets
validated without a GPU — the SSR increments use the same pattern so the math is
checkable in `cargo test`, not only on screen.
