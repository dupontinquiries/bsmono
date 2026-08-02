# simscope — one solver, many physics

A multi-scale simulation stack: power networks, circuits, PCB fields, EUV imaging, RTL.

## The bet

**Power flow and nonlinear FEA are the same solver wearing different physics.** Both are
"find x where F(x) = 0, given that you can evaluate F and solve J·δ = −F". Power flow's F is bus
power mismatch; a structural F is force residual; a device solve's F is current continuity. So
`sim-num::newton` is written once and every layer above it implements two methods.

The second unification is conceptual and matters for learning order: **a transmission line π-model
is a field integral somebody already did.** When you pass `r_ohm_per_km` you are consuming the
output of a 2D field solve on a conductor cross-section. Level 1 computes those numbers instead of
receiving them — which is why it is the right on-ramp from network analysis to FEA.

## Layers

| crate | layer | owns | status |
|---|---|---|---|
| `sim-num` | — | deterministic reductions, compensated arithmetic, f64 kernels, Newton | **built** |
| `sim-power` | L0 | Ybus, polar Jacobian, AC power flow | **built** |
| `sim-field` | L1 | 2D Laplace/Poisson on triangles → C, L per unit length → RLGC | next |
| `sim-pcb` | L2 | stackup, PDN impedance, thermal; consumes `cad-nifty` geometry | planned |
| `sim-litho` | L3 | EUV imaging by Fourier optics; f32-safe, GPU-shaped | planned |
| `sim-rtl` | L4 | logic evaluation; bit-exact, no floating point | planned |

Dependencies point down only. Nothing above `sim-num` may implement its own reduction.

## Precision policy

Decided by measurement on the target hardware, not by preference:

- **Honeykrisp (Apple AGX) reports `shaderFloat64 = false`.** AGX has no fp64 units; this is not a
  driver gap. WGSL has no f64 either, so the `gpu-gemm` path is f32-capped twice over.
- The `true` in a `vulkaninfo` dump is **lavapipe**, a CPU software device. Selecting it for
  compute is strictly worse than writing NEON directly, and it fails silently — correct answers,
  ~1000× slow. Device selection must name and refuse it, not rank on features.
- Mesa's softfp64 does exist and Asahi's GL 4.6 uses it (`GL_ARB_gpu_shader_fp64` is present). At
  1/32–1/64 of fp32 it is comparable to NEON f64 anyway — and both are irrelevant, because sparse
  work is bandwidth-bound and **on unified memory the CPU has the same 800 GB/s the GPU does.**

Therefore: **f64 lives on the CPU, in NEON.** GPU is for f32-safe, compute-dense work — element
assembly, Fourier imaging, Monte Carlo, batch sweeps. `shaderInt64 = true` on Honeykrisp, so GPU
reductions that must be reproducible use fixed-point int64 accumulation, where addition is
associative and the answer does not depend on thread scheduling.

SIMD is the default and costs no precision: NEON `float64x2_t` is IEEE double, two lanes wide,
bit-identical elementwise. Only reductions reorder, which is why `sim-num::sum` fixes ordering
structurally. `-ffast-math`-style reassociation is never enabled — it would break both
reproducibility and the compensated kernels, whose correctness depends on operations *not* being
algebraically simplified.

## Validation

`fixtures/` holds six IEEE cases (9 → 300 buses) generated from **pandapower**, each carrying the
per-unit network *and* its converged solution. That is third-party ground truth, not a
self-consistency check. Regenerate with `tools/gen_fixtures.py` against a pandapower checkout.

Every layer owes an equivalent: L1 against analytic coax and microstrip Z₀ tables, L3 against
known diffraction patterns.


## L0 status

All six fixtures converge from a flat start, 4–5 iterations, observed convergence order 2.0–2.8,
agreeing with pandapower to between 1e-15 and 1e-11 in both magnitude and angle. `cargo run -p
simscope-cli -- fixtures` prints the table.

Two things are load-bearing and worth not undoing:

**The finite-difference Jacobian test.** A wrong Jacobian still converges — linearly instead of
quadratically — so getting the right answer validates nothing about the derivatives. Central
differences on the residual are the only direct check, and the quadratic order in the report is the
behavioural corroboration.

**pandapower's Ybus ships in the fixture.** Our own builder agrees with it off-diagonal to 1e-9 and
in the imaginary diagonal, differing only in real diagonal terms at transformer buses — that is
pandapower's iron-loss conductance (`pfe_kw`), which the MATPOWER branch format cannot express. The
test asserts the *shape* of the disagreement rather than a loose tolerance, so a genuine tap-algebra
error would still fail. Solving uses the reference matrix, which keeps the solver test about Newton
rather than about a transformer model.

### L0 debts

- **Dense LU.** case300 (530 unknowns) takes 287 ms, almost all of it O(n³) factorisation done five
  times. Power flow Jacobians are ~4 nonzeros per row; sparse LU is the fix and this becomes its
  differential oracle.
- **No PV→PQ switching.** The fixtures carry Qmax = Qmin = 0, meaning pandapower ran with
  `enforce_q_lims=False`, so matching it requires *not* enforcing limits. The switching hook exists
  in `System::after_step` and is unused.
- **No sparse Ybus.** Stored dense, O(n²), which caps us near a thousand buses.
- **No DC power flow, no contingency batch.** N-1 screening across thousands of independent solves
  is the workload that actually justifies a GPU, and it is not written.
