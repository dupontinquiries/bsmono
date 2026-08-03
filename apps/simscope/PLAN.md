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
| `sim-num` | — | deterministic reductions, compensated arithmetic, f64 kernels, sparse+CG, FFT, Newton | **built** |
| `sim-mat` | — | material properties with provenance; every layer draws from it | **built** |
| `sim-power` | L0 | Ybus, polar Jacobian, AC power flow | **built** |
| `sim-field` | L1 | 2D Laplace → C, L, Z0; 2D linear elasticity → stress, strain | **built** |
| `sim-pcb` | L2 | stackups, cross-sections → Z0, eps_eff, delay | **built** |
| `sim-litho` | L3 | EUV aerial imaging by Fourier optics | **built** |
| `sim-rtl` | L4 | logic evaluation; bit-exact, no floating point | next |

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


## L1 status

Coax cross-sections solved on a triangle mesh, matching the closed form to ~0.1% at 6400 nodes,
and yielding **49.84 Ω** for PTFE dimensions chosen to be a 50 Ω cable — with no transmission-line
formula anywhere in the code path. `cargo run -p simscope-cli` prints both tables.

The element integral is the entire "lower-level integrals" gap, and for P1 it is exact: `grad N_i`
is constant over a linear triangle, so `K_ij = eps (b_i b_j + c_i c_j) / 4A` with no quadrature at
all. Everything harder in FEA is this same expression with a quadrature rule where the closed form
used to be.

Verification is layered, weakest to strongest:

1. **Row sums vanish** — a constant potential produces no field, so `K·1 = 0` exactly. Catches most
   sign and index errors in the gradient coefficients.
2. **The patch test** — pin the boundary to a linear field, and the interior must reproduce it to
   roundoff. A code failing this cannot represent a uniform field, which is what every problem
   looks like locally under refinement, so it is a precondition for converging at all.
3. **Analytic coax** — 0.06–0.12% at 6400 nodes.
4. **Convergence rate** — O(h²) in energy, measured radially at fixed angular geometry.
5. **The physical identity** — `LC = με`, so a homogeneous line's phase velocity must be `c₀/√εr`.
   This ties two independent solves together and is sharper than comparing L to a formula.

`L` comes from the vacuum solve (`L = 1/(c₀²C_vac)`), which stays valid for mixed dielectrics
because the magnetic field does not see the dielectric — the reason a microstrip's phase velocity
is neither `c₀` nor `c₀/√εr`.

### L1 debts

- **Structured meshes only.** Annulus and rectangle. Real cross-sections need a mesher, or
  CADscope's SDF via an immersed-boundary formulation — which sidesteps meshing entirely and is
  the more interesting option given what already exists.
- **No microstrip validated.** The mixed-dielectric path is written and untested against published
  Z₀ tables; only homogeneous coax is checked.
- **No conductor loss, no skin effect.** `R` and `G` in "RLGC" are not computed; this is LC only.
- **Jacobi-preconditioned CG.** Fine at this size; a graded or highly refined mesh wants
  multigrid, which is also the form that ports to GPU as a matrix-free operator.


## L2 status

Board cross-sections solved with the L1 machinery unchanged — only the geometry and the material
map differ.

```
  stripline w=0.50mm b=1.0mm       46.94   ref 48.87   eps_eff 4.199   6.83 ps/mm
  microstrip w=0.38mm h=0.20mm     48.28   ref 50.42   eps_eff 3.323   6.08 ps/mm
```

**Stripline is the sharp test.** It is homogeneous, so its effective permittivity must equal the
substrate exactly — 4.199 against 4.200, and 2.999 against 3.000. That is physics rather than a
curve fit, and any leak in the mixed-dielectric path would show there first.

Microstrip is checked against Hammerstad, which is itself a ~1% fit to an *open* geometry while the
solver models a shielded box. The consistent ~4% shortfall is the lid adding capacitance, and
`sweep_box_height` shows it converging upward as the box grows rather than asking anyone to take
that on trust.

### L2 debts

- **Zero-thickness traces.** Finite copper raises capacitance a few percent; the tensor grid can
  represent the sides but does not.
- **Shielded box, not open.** A finite domain cannot represent an open microstrip. The sweep shows
  convergence; a proper treatment wants an absorbing or asymptotic boundary condition.
- **No R or G.** Conductor loss, skin effect and dielectric loss are absent — this is LC, and
  calling the crate's output "RLGC" would be overclaiming.
- **Single trace only.** No coupled pairs, so no differential impedance and no crosstalk, which is
  most of what a signal integrity tool is for. The capacitance *matrix* is the generalisation.
- **Not yet reading `cad-nifty`.** The `%kind: hardware` diagrams carry trace widths and layers in
  millimetres, which is exactly this crate's input. Wiring the two is a path dependency across
  workspaces and is the obvious next join.


## L3 status

EUV aerial imaging by partially coherent Abbe summation. λ = 13.5 nm, NA 0.33 gives a coherent
pitch limit of 40.9 nm; High-NA at 0.55 gives 24.5 nm and costs 2.8× the depth of focus.

```
     pitch      hp    coherent   sigma 0.5      dipole
     49.1n   24.5n      0.9983      0.9284      0.8910    ok
     38.9n   19.4n      0.0000      0.7348      0.8910    ok
```

**The cutoff is the test worth having.** Below `λ/NA` only the zeroth order reaches the wafer, and
one plane wave has uniform intensity — so coherent contrast is not poor, it is *exactly zero*. No
reference data, no fitted constant, no tolerance to argue about. Off-axis illumination shifts the
pupil onto an order that missed, which is why the dipole column still prints at 38.9 nm, and why
dipole illumination exists.

Precision note: this layer is f32-safe by the policy in this document — a bounded feed-forward map
with no ill-conditioned inverse and no long accumulation, ending in a threshold decision. It is
written in f64 because that is free on a CPU; a GPU port drops to f32 without consequence, and the
FFT is the one genuinely GPU-shaped workload in the stack.

### L3 debts

- **Scalar imaging, not vector.** At NA 0.55 polarisation matters materially and is ignored.
- **No mask 3D.** EUV masks are reflective multilayers with real topography; treating them as thin
  transmission functions misses shadowing and best-focus shift, which are first-order effects at
  High-NA.
- **No resist model.** The aerial image is not the printed feature. Acid diffusion, blur and the
  stochastics that dominate EUV are absent — and stochastics is the Monte Carlo workload the
  precision policy flagged as genuinely GPU-shaped.
- **No aberrations.** Only defocus; Zernike terms are not implemented.
- **Periodic-only masks.** `Mask::grating` now *asserts* whole-pixel periodicity rather than
  quietly aliasing, but a real layout needs padding and windowing.


## Materials (`sim-mat`)

Cross-cutting rather than a layer: it sits beside `sim-num` and everything depends on it. Before
it, permittivity lived as a bare closure in `sim-field` and a bare field in `sim-pcb`, and neither
knew anything about conductivity, stiffness or expansion. That works while a layer needs one
property and fails the moment the same FR-4 needs ε for L2, k and α for a thermal solve, and E and
ν for warp.

**A property is not an `f64`.** It carries provenance, reported spread and a temperature window,
and there is no `Default` and no way to build one without saying where it came from. Material data
is where a fabricated number does the most damage: a coarse mesh shows up in a convergence study,
but a modulus 30% off shows up nowhere — every solver downstream propagates it faithfully. Same
rule as `cad-ref` refusing to invent a volume.

**Anisotropy is first-class**, because the materials here are anisotropic in the way that breaks
things. FR-4's through-plane CTE is ~5× its in-plane value, which is what cracks vias at reflow; a
scalar α cannot express the failure mode.

The validation worth having is **Wiedemann–Franz**: in a metal, heat and charge ride the same
electrons, so `k/(σT)` must equal 2.44e-8. That is a cross-check between two *independently
sourced* numbers, which no amount of internal consistency checking can substitute for. Copper and
aluminium pass within 10%. Stainless 304 came out 60% high — and that is the law's domain, not a
bad entry: in a disordered alloy the lattice carries a large share of the heat, so total `k`
exceeds the electronic prediction. Asserting that excess is a stronger test than exempting the
material.

### Nacre, and why it is the least confident entry

~95% aragonite by volume, yet **24× the fracture toughness** of the mineral it is made of, at
comparable stiffness. None of that toughness is in the constituents — it comes from crack
deflection at platelet boundaries, platelet pull-out, mineral bridges and viscoelastic dissipation
in the organic matrix. All interfacial.

Two consequences, both recorded in the library entry rather than left implicit:

- **A bulk continuum model cannot produce it.** Feeding these constants to an elasticity solver
  predicts a stiff brittle ceramic, convincingly. The toughness needs cohesive interfaces, and
  `K_Ic` here should be read as a *target for* such a model rather than an input to one.
- **Dry nacre is brittle.** The organic matrix's dissipation requires water. Any property without a
  hydration state is misleading, and the published scatter is a factor of five.

Effective properties for a microstructure are a solver output, not a lookup — homogenization on a
representative volume element, structurally the same operation as capacitance extraction (apply
unit loads, integrate energy, divide). `Provenance::Computed` exists for exactly those values so
they are never confused with measured ones. Not implemented.

### Materials debts

- **No homogenization.** The RVE driver, periodic boundary conditions and the Voigt/Reuss bracket
  test are the next piece, and the bracket is unusually strong: any effective modulus falling
  outside those bounds is provably wrong with no reference data needed.
- **No elasticity solver yet**, so stiffness data is stored and unused. 2D plane stress/strain on
  triangles is the load-bearing dependency for fracture, impact and homogenization alike.
- **No temperature dependence beyond resistivity.** `k`, `E` and `α` all vary and are stored as
  single values with a validity window.
- **Transverse isotropy only.** Full anisotropy is not modelled.


## Elasticity

Two unknowns per node on the same triangle. The geometric coefficients `b_i`, `c_i` are *identical*
to the Laplace case — they are shape-function gradients either way — and Laplace contracts them
into a scalar while elasticity arranges them into `B`. Assembly, Dirichlet elimination and the CG
solve are reused unchanged, which is the clearest evidence so far that the shared-core bet was
right.

Verified by, in order of strength:

1. **The patch test**, now in two parts: a linear displacement field must be reproduced exactly
   *and* the resulting stress must be uniform across every element. The second half is what catches
   a wrong constitutive matrix, since a bad `D` can still pass a displacement check.
2. **Rigid-body motion produces no stress.** Infinitesimal rotation is the sharp case: `γ_xy =
   −θ + θ` vanishes only if the two off-diagonal terms of `B` carry opposite signs correctly.
3. **Uniaxial tension recovers E and ν** from the solve rather than assuming them, with the free
   lateral edge coming out unstressed.
4. **Plane strain is stiffer than plane stress**, and the two coincide at ν = 0 — an ordering that
   would reverse if the matrices were swapped, and which no single-mode test would notice.
5. **Cantilever convergence.** Constant-strain triangles cannot represent bending curvature, so a
   coarse mesh is too stiff. The test asserts the trend — refinement softens it monotonically
   toward `PL³/3EI` and never overshoots — because asserting the value would be dishonest about a
   known property of the element.

### Elasticity debts

- **Constant-strain triangles.** Poor in bending; the finest mesh tested reaches ~85% of the beam
  solution. Quadratic (T6) elements or incompatible modes are the fix, and both need real
  quadrature — which is where the closed-form element integral finally ends.
- **In-plane isotropy only.** `constitutive_of` uses a transversely isotropic material's in-plane
  constants, which is exact for in-plane loading and silently wrong for anything involving z. A
  full orthotropic `D` is needed before FR-4 warp or nacre stacking can be modelled.
- **No thermal coupling**, so `α` is stored and unused and thermal stress cannot be computed.
- **No body forces, no traction boundary conditions.** Loads are nodal only.
