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

- ~~No homogenization.~~ Built; see below. Periodic boundary conditions remain the gap.
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

- **Constant-strain triangles are stiff in bending — measured, not assumed.** See the study below.
- **In-plane isotropy only.** `constitutive_of` uses a transversely isotropic material's in-plane
  constants, which is exact for in-plane loading and silently wrong for anything involving z. A
  full orthotropic `D` is needed before FR-4 warp or nacre stacking can be modelled.
- **No thermal coupling**, so `α` is stored and unused and thermal stress cannot be computed.
- **No body forces, no traction boundary conditions.** Loads are nodal only.


## Thermal, and the analogy made concrete

Steady conduction is Laplace with `k` where `ε` was, so `assemble` is reused verbatim. The test
that makes the point is cylindrical: a shell conducts `2πk/ln(b/a)` and a coax line holds
`2πε/ln(b/a)` — the **same integral**. Running both through identical code and asserting
`C/ε == G/k` is a stronger statement than either check alone.

Heat balance is checked as a *reaction* (`K·T` restricted to the boundary) rather than by
differentiating the field and integrating flux. The reaction is exact for the discrete system; the
differentiated flux only converges. For conservation, exact beats convergent.

**Thermal stress**: `σ = D(ε − ε₀)`, where `ε₀ = αΔT` is what the material wanted. Stress arises
only from the part of that the geometry refused. Hence the sharpest test available —
**unconstrained uniform heating produces exactly zero stress** — which is also where the most
common implementation error shows: forgetting to subtract the free strain produces enormous,
entirely plausible-looking stresses in a body that is simply expanding. Full restraint reproduces
`−EαΔT/(1−ν)` to nine figures, and a CTE mismatch generates stress where a matched pair does not.

The FR-4/copper reflow case is now expressible end to end: a 200 K excursion drives a mismatch
strain of 0.011, well past copper's elastic limit — which is why via failure is fatigue cracking
over many cycles rather than a single fracture.

## Mixture bounds (`sim-mat::mixture`)

The acceptance test for a homogenization solver that has not been written yet, deliberately built
first. Voigt (uniform strain) bounds above, Reuss (uniform stress) below, and **anything outside
that bracket is provably wrong** — no reference data, nothing to tune against. Hashin–Shtrikman
narrows it for statistically isotropic mixtures, and the *width* of the bracket is itself the
signal: wide means geometry decides and only a solve will do.

For nacre the bracket is wide — 5% of a soft phase barely moves Voigt but collapses Reuss, because
uniform stress puts all the strain into the compliant phase. And there is a limit stated as a test
so it cannot be quietly forgotten: **these bounds constrain stiffness, not toughness.** Nacre's
modulus is admissible and unremarkable. Its toughness is ~24× either constituent, and no bound of
this kind can reach it, because bounds assume perfectly bonded phases while nacre's toughening is
interfaces sliding, deflecting and bridging. **A homogenization that reproduces nacre's stiffness
proves nothing about its toughness** — which is the clearest statement I can make of why the
cohesive-interface model is the real work.


## CST bending: what the measurement actually says

The earlier note ("~85% of the beam solution") was a single data point from one mesh shape, and
following it up changed two conclusions.

**It converges correctly, at the expected rate.** Cantilever at L/h = 10, tip deflection as a
fraction of the exact 2D answer:

```
   8x2      32 tris    29.8%      rate  —
  16x4     128 tris    62.5%      rate 0.91
  32x8     512 tris    86.7%      rate 1.50
  64x16   2048 tris    96.2%      rate 1.78
 128x32   8192 tris    98.9%      rate 1.76
```

Approaching O(h²) as it should. The "85%" was just the 32×8 row.

**The reference was wrong, and the test was passing for the wrong reason.** A plane-stress solve
converges to the exact *two-dimensional elasticity* answer, which includes shear deflection and
therefore sits **above** Euler–Bernoulli — by 0.78% here. The old test asserted the FEM "must not
overshoot the beam solution" against the Euler value, which is false: a converged solve is supposed
to exceed it. It passed only because CST is stiff enough never to get there, so **a better element
would have failed a correct implementation.** The reference is now Timoshenko.

**Element shape matters more than element count.** At a fixed budget of 1024 elements, trading
length resolution for thickness resolution:

```
  256x2   63.5%      128x4   86.7%      64x8   93.7%      32x16   88.8%      16x32   68.5%
```

Thirty points of accuracy for the same cost, decided entirely by aspect ratio, with a clear optimum
at near-square cells. `Mesh::worst_aspect` now reports this and a regression test guards it — it is
a cheap pre-check that predicts accuracy loss before any solve runs.

### What to do about it, by regime

- **Thick sections, L/h ≲ 20:** CST is adequate. Use near-square elements and refine; 96% at 2k
  elements is fine for most purposes. No code change needed, just meshing discipline.
- **Thin structures — PCB warp, EUV mask distortion:** this is where CST fails and **more elements
  will not save it.** Near-square cells at L/h = 1000 means ~8000 along the length for 8 through
  the thickness, tens of thousands of elements for one cross-section. The right answer is not a
  better 2D element but a **different formulation**: plate or shell elements, whose unknowns are
  mid-surface displacement and rotations and which need one element through the thickness by
  construction. 2D plane-stress continuum is the wrong model for a thin plate regardless of element
  quality.
- **Stress concentration and fracture:** neither helps. Crack-tip fields are singular and need
  local refinement, quarter-point elements, or an enrichment — and the J-integral is evaluated on a
  contour away from the tip precisely to avoid depending on how badly the tip is resolved.
- **T6 quadratic triangles** would fix the first regime handsomely and the second not at all. Worth
  doing when the element integral needs real quadrature anyway — nonlinear materials, curved
  geometry, full orthotropy — rather than for bending alone.


## Laminates — the answer the bending study pointed to

For thin layered structures the fix is not a better element but a different formulation: stop
meshing through the thickness and integrate through it analytically. Classical laminate theory
gives A, B and D matrices relating in-plane strain and curvature to force and moment resultants,
and it solves PCB warp and mask distortion at the level those problems actually live at.

**`B` vanishing for a symmetric stack is the sharp test** — exactly, not approximately. It is why a
symmetric PCB stackup lies flat through reflow and an asymmetric one potato-chips, and it is the
design rule the theory exists to justify. A 1.5 mm FR-4 board with copper on one face bows 0.1 mm
over 100 mm at reflow; balanced copper on both faces removes it entirely.

Two predictions of mine were wrong and the measurements corrected them:

**CLT matches Timoshenko's 1925 bimetal formula to twelve figures**, with no biaxial correction. I
expected a factor of `1/(1−ν)`, since Timoshenko's strip is free to contract laterally and a plate
is not. There is none, because his curvature depends on the moduli *only through their ratio* — the
plate constraint replaces every `E` with `E/(1−ν)` and leaves that ratio unchanged. Confirmed by
giving the two layers different Poisson ratios, which breaks the coincidence as the explanation
requires.

**Stoney's 1/t² is asymptotic, not exact.** Measured across 0.5–2 mm with a 20 µm film the exponent
is 1.70, and asserting 2 there demands a limit hold outside its limit. The test now checks that the
exponent *moves toward* 2 as the thickness ratio improves, reaching 2.00 at a film-to-substrate
ratio of 0.001.

### Laminate debts

- **In-plane isotropic laminae only.** Correct for FR-4 and for a transversely isotropic stack —
  such a material is by definition isotropic in-plane — but ply angles and genuinely orthotropic
  laminae are not modelled, so a fibre-oriented composite cannot be described.
- ~~Uniform temperature only.~~ Closed: `thermal_response_linear` handles `T(z) = T_mid + slope·z`,
  which is the EUV mask case.
- **No moisture, no cure shrinkage**, both of which warp real boards comparably to CTE mismatch.
- **Linear elastic.** Copper yields during reflow, so a plastic laminate model is needed before the
  cyclic via-fatigue question can be answered rather than bounded.


## EUV mask distortion — L3 meeting the mechanics

The imaging model in L3 says nothing about the mask moving, and on a real scanner that is one of
the things limiting overlay. A 6025 blank absorbs what the multilayer does not reflect, at the
*surface*, so the distortion is driven by a **through-thickness gradient** — and symmetry offers no
protection, because balancing a stackup cancels moments from a uniform rise, not from a gradient.

```
  absorbed    dT thru    curvature          bow      overlay
      0.5W     0.105K     4.956e-7      1.43 nm      0.30 nm
      2.0W     0.420K     1.982e-6      5.73 nm      1.20 nm
      5.0W     1.049K     4.956e-6     14.31 nm      3.01 nm
```

Bow becomes overlay through the 6° chief ray, roughly `2·bow·tan(6°)`.

The point the table makes is about **tolerance rather than nominal value**. ULE's CTE is specified
as a band, −3e-8 to +3e-8 /K, and at exactly zero the distortion vanishes identically — the test
asserts that. Nothing is ever exactly zero, and the edge of the band turns 0.42 K into 1.2 nm of
overlay. The material specification is doing the engineering, not the material.

Two structural checks came with it. A homogeneous plate under a linear gradient curves at exactly
`κ = α ΔT / h`, **independent of modulus** — that E drops out is the real check, since a result that
depended on stiffness would mean the thermal moment and the bending stiffness were not consistently
derived. And a symmetric laminate warps freely under a gradient while staying flat under uniform
heating, which is the design distinction stated as a test.


## Homogenization

Three solves: apply each unit macroscopic strain in turn and the volume-averaged stress is the
corresponding column of the effective stiffness. Structurally the same operation as capacitance
extraction — impose a unit load, integrate the response, divide.

`sim-mat::mixture` was written first on purpose, and it did its job: the bracket caught two wrong
expectations of mine before either could become a believed result.

**Kinematic boundary conditions are an upper bound, and the RVE-size convergence is measured**
rather than claimed. Prescribing `u = ε·x` on the whole boundary suppresses the local fluctuation a
real microstructure shows; the result relaxes downward as the RVE contains more repeat units.
Periodic BCs give the right answer at any size and need multi-point constraints — not implemented.

Two findings worth keeping:

**Voigt is not attained by layers along the load, and I expected it to be.** Measured 165 GPa
against 170.7. Kinematic BCs fix displacement only on the *boundary*, so while every layer carries
the same `εxx`, the transverse `εyy` is suppressed at the boundary and free inside — and since
`σxx = Q11 εxx + Q12 εyy`, that interior Poisson contraction pulls the average a few percent below
the uniform-strain value. Voigt is attained only when *both* strain components are uniform.

**The bracket must be applied to stiffness, not to Young's modulus.** The scalar Voigt/Reuss
average of `E` is equivalent to the tensor bound only when the phases share a Poisson ratio. With
aragonite at 0.30 and the organic matrix at 0.40, the extracted `E_eff` came out marginally *above*
the scalar Voigt value while remaining perfectly admissible — because `E` is obtained by
**inverting** `C`, and the inequality does not survive the inversion. The rigorous statement is on
`Q11`.

### Nacre: what this reaches and what it cannot

A brick-and-mortar RVE — offset mineral platelets in a thin compliant matrix — homogenizes to a
stiffness in the measured 60–80 GPa region. That is the most that can honestly be claimed, and the
test asserts the other half explicitly: **every interface in this model is perfectly bonded**, so no
platelet can slide, no crack can be deflected, and nothing can pull out. Those three mechanisms are
what nacre's toughness *is*. The factor of 24 remains unreachable from here by construction, not by
insufficient refinement.

### Homogenization debts

- **No periodic boundary conditions**, hence an upper bound rather than the answer.
- **No cohesive interfaces**, hence stiffness only. This is the piece nacre actually needs.
- **2D plane stress only.** A platelet stack is a 3D arrangement and its through-thickness response
  is not accessible here.
- **No thermal or conductivity homogenization**, though both are the same machinery with the
  Laplace operator instead of elasticity.


## Fracture

The crack-tip field is singular — it goes as `1/√r` — so no finite element resolves it, and a
method that reads stress *at* the tip is measuring its own mesh. The J-integral sidesteps that
entirely: it is conserved, so evaluating it on a contour away from the tip gives the same answer as
one hugging it, and the far field is what the mesh does resolve.

**Path independence is the primary test and needs no reference data.** Compute `J` over several
annuli and they must agree; measured spread is under 5%. If they disagree, the result is not a
J-integral whatever else it happens to match.

Then, against the handbook: `K_I` matches the standard edge-crack solution within 12% at three
crack lengths, and the `√a` scaling holds independently of any handbook constant. An uncracked
ligament releases essentially no energy, which catches a domain integral picking up the general
stress field rather than the singular one — an error that would still produce plausible numbers on
a cracked mesh.

The equivalent domain form is used rather than a line integral, because a contour drawn through a
mesh cuts elements at arbitrary places and samples a piecewise-constant stress field at its worst.
And the crack is modelled as an *unrestrained portion of a symmetry boundary* rather than by
splitting nodes — simpler, and it removes a whole class of connectivity errors.

### What this says about nacre

At nacre's own working strength, aragonite's critical flaw is **0.81 µm** — larger than a platelet
is thick (~0.5 µm). No crack big enough to matter fits inside a single platelet, which is the
nanoscale flaw-tolerance argument: below a critical dimension a brittle solid becomes insensitive
to flaws because theoretical strength is reached before a crack can run.

That is a **size effect, not a material property**, which is exactly why it cannot live in
`sim-mat`'s library and has to be computed. It is also only part of the story — flaw tolerance
explains why the platelets do not break, not why cracks between them are arrested. That second half
is still the cohesive-interface question.

### Fracture debts

- **LEFM only.** No plasticity, so no crack-tip yielding and no J-R curve; for copper or steel this
  bounds rather than predicts.
- **Mode I only.** No mixed-mode decomposition, which needs an interaction integral.
- **No crack growth.** `J` is evaluated on a fixed geometry; propagation needs remeshing or an
  enrichment.
- ~~No cohesive zone.~~ Built; see below. Meshed cohesive *elements* remain the gap.


## Cohesive interfaces — where toughness comes from

LEFM answers "will this crack run" given a toughness measured elsewhere. A cohesive zone answers
the prior question: **where does the toughness come from at all?** The crack is a process zone whose
faces still pull on each other, and the area under the traction–separation curve *is* the fracture
energy. That identity is the central test, checked by integrating the traction the law actually
returns rather than trusting the closed form it was built from.

This is the first genuinely nonlinear problem in the workspace, and it runs on the Newton solver
written at the very start for power flow — which is the shared-core bet paying off on something
that looks nothing like a power network.

Three properties are asserted rather than assumed: the tangent matches a finite difference of the
traction (the same discipline as the power-flow Jacobian, and for the same reason — a wrong tangent
still converges, just slowly); softening makes the tangent **negative**, which is why load control
is useless past the peak; and damage is **irreversible**, so unloading follows a secant to the
origin rather than retracing the envelope. A model that retraced would heal itself under cyclic
load, which is unphysical and flattering.

### Three bugs, and what each looked like

**Damage was driven by the committed history rather than the trial state.** A Newton iteration
probes separations beyond the last committed maximum, and there the law extrapolated the *secant*
instead of following the softening envelope — making the interface far too strong past the peak and
costing the series solve its equilibrium entirely.

**Snap-back is real and is now named.** When the surrounding structure is more compliant than the
interface's softening slope (`σ_max²/2G_c`), there is no stable equilibrium: the interface releases
faster than the structure can absorb, and the true response doubles back in *both* load and
displacement. Following it needs arc-length control, not implemented. The test requires the solver
to **fail loudly** there, because one that quietly returned the nearest converged point would draw
a smooth curve through a discontinuity.

**The Newton tolerance was absolute on a residual measured in pascals.** 1e-12 against ~1e7 is below
machine precision, so a perfectly converged solve reported `MaxIterations` — the residual was
already zero to every available bit. Exactly the same mistake as comparing strain and stress against
one absolute number, and just as invisible.

### What it says about nacre

Aragonite releases 0.6 J/m²; nacre needs over a hundred times that. Pull-out can supply it:

```
   G_pullout  ~  tau · (l/t) · slip
```

At 20 MPa interfacial shear, aspect ratio 10 and a micron of slip, that is ~200 J/m² — the right
order. **The aspect ratio is the amplifier**, linearly, which is why platelets are long and thin.

And the cost is stated: a micron of slip exceeds a platelet thickness, so this is *sliding*, not
stretching. **Nacre is tough because it is allowed to be damaged, not because it is strong** — which
is precisely what the LEFM model could not express, and why this module had to exist.

### Cohesive debts

- ~~No meshed cohesive elements.~~ Built; see below. A brick-and-mortar RVE with interfaces on
  every platelet boundary is the remaining step.
- ~~No arc-length control.~~ Implemented; snap-back is now traced.
- ~~Mode I only.~~ Mixed-mode implemented; see below.
- **Rate independent**, whereas nacre's organic matrix is viscoelastic and its dissipation is
  rate-dependent — part of why hydration matters so much.


## Meshed cohesive interfaces

The law is now in a mesh, which is what lets a *structure* dissipate rather than a point. Node pairs
share a location, one per side, each carrying its own damage history — so a crack runs along the
interface **progressively** rather than the whole surface failing at once. That progressiveness is
the point: it makes dissipated energy scale with crack *area* instead of with load.

**The verification is an energy balance and it is exact to 1e-9.** However the crack advances, the
energy dissipated must equal `G_c` times the length actually separated. No reference data, no
handbook — conservation, the same class of test as the J-integral's path independence. Alongside it:
the peak reaction lands at the cohesive capacity `σ_max · w`, a fully separated interface carries
nothing, dissipation never runs backwards, and an intact interface reproduces bulk stiffness — which
checks the node duplication, the assembly and the force signs at once.

Nodes are duplicated here rather than treating the crack as an unrestrained symmetry boundary the
way `fracture` does, because these two faces must *interact* through a traction rather than simply
be free.

### The solver constraint that shapes this

The tangent is assembled dense and factorised by **LU, not CG**, because once any pair enters
softening the tangent is **indefinite** and CG's assumptions fail outright. That is not a tuning
choice — it is the same reason `sim-power` uses LU while `sim-field`'s Laplace problems use CG, and
it caps the mesh size here. A sparse indefinite factorisation is the fix and is recorded as a debt.

### Remaining before nacre's toughness is computable

- **A brick-and-mortar RVE with interfaces on every platelet boundary**, which is now assembly work
  rather than new physics.
- **Arc-length control**, since a propagating crack in a compliant structure snaps back.
- **Sparse indefinite solve**, to afford an RVE large enough to contain a process zone — and the
  process zone length `E G_c / σ_max²` must fit inside the model or the answer is mesh-dependent.
- ~~Mixed-mode.~~ Done — the law now takes a normal/tangential separation pair. The meshed
  `interface` module still assembles opening only.


## Mixed mode, and pull-out solved rather than estimated

The effective separation is `sqrt(<δ_n>² + β²δ_t²)`. The Macaulay bracket on the normal term is the
physics: closing an interface must not damage it, or a compressed crack would heal by being
squeezed — while sliding damages it either way, which is exactly the case a platelet being dragged
from its socket lives in.

**The test that makes it trustworthy: every proportional path dissipates exactly `G_c`.** Pure
opening, pure sliding, and everything between, at three values of β. A law that quietly became
tougher in shear would flatter any model whose failure is shear-driven — which is precisely nacre's,
so this is the one place a subtle error would be most rewarded and least noticed.

`β` turns out to separate two concerns cleanly, which is what makes it a usable parameter: it sets
how much *sliding* it takes to fail an interface without changing how much *energy* that costs. And
a refactor guard asserts mode I behaviour is bit-identical to before the generalisation.

**Pull-out is now solved rather than estimated.** A platelet bonded along one face, dragged under
displacement control, with the interface as segments each carrying its own damage — so debonding
starts at the loaded end and travels. Two things hold: the work equals `G_c` times the bonded
length, independent of how the debond travelled, and it scales *linearly* with that length. The
second is why a long thin platelet is worth more than a short fat one, and it is now a solved result
rather than the shear-lag estimate in `pullout_energy`.

### Still open before a full nacre RVE

- ~~The meshed `interface` module assembles opening only.~~ Done — the element now carries both
  components with a consistent rotated tangent.
- **Arc-length control**, **sparse indefinite solve**, and a process zone (`E G_c / σ_max²`) that
  fits inside the model — unchanged from the previous entry, and all three are needed together
  before an RVE is affordable and mesh-independent.


## Mixed mode in the mesh

The element now carries both separation components, with the 2×2 material tangent rotated into
global coordinates. That tangent is derived rather than approximated — a secant would converge
linearly and the point of Newton here is the quadratic rate — and it is verified against finite
differences at four separation states including one that is *compressed and sliding*. It comes out
symmetric, which is not a coincidence (the law derives from a potential) and is asserted, because an
asymmetric tangent would quietly destroy the symmetry of the global stiffness.

**Shear and opening dissipate identically**, to 1e-6. An interface is not tougher in one direction
than the other, only harder to separate.

Getting that test to run taught two things, both physics rather than tuning:

**The interface must be the compliant path.** Opening a block is only possible by opening its
interface — but a block shears perfectly well by deforming its own bulk, so with a stiff penalty the
halves simply shear and the interface never slides enough to damage. At the opening test's
parameters the interface was 37× stiffer than the bulk in shear and dissipated essentially nothing.

**And the snap-back condition bites again, from the other side.** At those same parameters the
softening slope and the bulk shear stiffness were within 20% of each other — right at the
instability `cohesive` already names. A gentler peak traction puts the slope an order of magnitude
below the bulk and restores stability. The test now *asserts* that margin before running, so it
fails with a legible message rather than as a convergence mystery.

### Remaining for a nacre RVE

Unchanged and now genuinely the last three: **arc-length control**, a **sparse indefinite solve**,
and an RVE large enough that the process zone `E G_c / σ_max²` fits inside it. All three are needed
together — the first to trace propagation, the second to afford the mesh, the third for the answer
to be mesh-independent rather than merely converged.


## Arc-length control

Newton under prescribed load fails at a limit point — past the peak no load produces equilibrium.
Newton under prescribed displacement fails at a **snap-back**, where the structure releases faster
than it is pulled and the response turns back in displacement too. Both are real behaviours of
softening structures, and the cohesive module was correct to refuse them.

Arc-length makes neither load nor displacement the independent variable: it advances a fixed
distance along the equilibrium path in `(u, λ)` space, so the path can turn however it likes. Rather
than factorise the bordered system, the tangent is solved twice — against the residual and against
the reference load — which reduces the constraint to a **quadratic in one scalar**. Choosing between
its two roots is the entire difficulty: take the wrong one and the solver retraces the path it just
walked, which *looks like convergence*.

Verified four ways: it traces past a limit point where the load factor must come back down; it does
**not** invent a snap-back where none exists; every traced point satisfies equilibrium rather than
merely being where the algorithm stopped; and steps advance by the requested arc length.

Applied to the cohesive interface, the previously-refused case is now traversed, with the definitive
signature asserted — **total displacement decreases somewhere along the path**, which is what a
displacement-controlled testing machine would jump across rather than follow. And tracing an
unstable path does not change what it costs: the traced work still equals `G_c`.

### A pattern worth naming

This was the **third** false non-convergence in the workspace caused by an absolute tolerance on a
dimensional residual — the 1D cohesive pull, the meshed interface, and now the tracer, whose
reference load was unity while its residual was a traction in pascals. Each time it presented as a
solver failure rather than a units error, which is why it kept recurring. **A tolerance is only
meaningful relative to the scale of the quantity it bounds**, and a solver interface should carry
that scale explicitly rather than leaving the caller to remember it.

### Remaining for a nacre RVE

Two: a **sparse indefinite solve** to afford the mesh, and an RVE large enough that the process zone
`E G_c / σ_max²` fits inside it — the latter so the answer is mesh-*independent* rather than merely
converged.
