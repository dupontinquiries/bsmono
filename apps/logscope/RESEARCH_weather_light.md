# Research: weather systems + leaf light-scattering (for the mind-palace renderer)

Scoped to what's implementable in our **software/GL forward renderer** (no compute shaders, simple vertex/fragment GLSL + the `texgen` material array). Three asks: ocean weather (Atlantis), canopy weather + light, and leaf light-scattering.

## 1. Leaf light-scattering — the Canopy "glowing silhouette" effect
**Finding.** Leaves scatter light *mostly forward*; backscatter is negligible (Habel, Kusternig & Wimmer, *Physically Based Real-Time Translucency for Leaves*, EGSR 2007). The visually dominant effect is **transmission**: a leaf lit from behind glows on the viewer's side. Full BSSRDF/diffusion-dipole models exist but are overkill here.

**Practical approach (cheap, one extra term in the fragment shader, MOSS material only):**
- Add a **back-light transmission** term. With sun direction `L`, normal `N`, view `V`:
  `trans = pow(clamp(dot(V, -L), 0, 1), p) * wrap(N, L)` where `wrap = clamp((dot(N,L)+w)/(1+w), 0,1)` softens the terminator.
  `col += leaf_albedo * sun_col * trans * thickness_tint` (green-biased, brighter at thin leaf tips).
- Drive it by the **overhead sun**: where the sun lines up behind a leaf toward the eye, that leaf lights up — exactly the "beams align to form a glowing silhouette" you described. The text-cells on the Canopy panel can ride the same term so they read as *backlit* rather than emissive.
- Pairs with the deferred shadow pass: leaf self-shadowing + this transmission = the dappled depth.

## 2. Ocean weather (Atlantis / any water)
**Finding.** Two standard families:
- **Tessendorf FFT** (2001; Horvath 2015 directional/swell spectra; TMA/JONSWAP). Industry standard — wind speed + direction parameterise a wave spectrum; an inverse FFT yields displacement + normal + **foam from the Jacobian** (wave steepness). Needs compute/FFT — heavy for us.
- **Gerstner / Rankine waves** — sum of a few directional waves (water particles in elliptical orbits), wind-driven. `O(N)` per vertex but only need 3–6 waves. **This is our pragmatic path.**

**Practical approach:**
- Displace the water mesh with **3–6 summed Gerstner waves** (vertex shader or CPU pre-displace), each `(dir, amplitude, wavelength, steepness)`. Wind direction biases all `dir`s; **wind speed scales amplitude + steepness (choppiness)**.
- **Foam/whitecaps** where summed steepness (Jacobian proxy) exceeds a threshold — more foam at higher wind. We already have the foam-ring primitive from the Mistfall waterfall.
- **Weather state = {wind_speed, wind_dir, swell}** → the whole ocean responds. A storm = high wind (sharp choppy crests + heavy foam + darker sky/fog); calm = low amplitude, parallel swell.

## 3. Canopy weather (rain / sun / wind)
- **Sunshine:** we already have god-rays/bloom; add the leaf transmission (§1) so shafts that pass *through* the crown light the leaves. Volumetric shafts = the godray pass we have, anchored to the sun disc.
- **Rain:** camera-space streak particles (cheap billboards/lines) + a **wet-darkening** of horizontal surfaces (lower albedo, higher gloss when `weather.rain>0`) + ripple rings on water/ground. Reuse the underwater ripple math.
- **Wind:** a **coherent sway field** — a low-frequency phase-coherent noise `sway = sin(t*f + worldpos·k)` applied stronger to thinner/higher geometry (leaf tips, thin branches) than trunks. One global `wind_dir, wind_strength` drives every theme's foliage, fabric, and water consistently.

## Shared "weather" abstraction (proposed)
A single `Weather { wind_dir, wind_strength, rain, cloud, time_of_day }` struct, set per-frame, fed to:
- water (Gerstner amplitude/choppiness + foam),
- foliage/fabric/branches (sway field strength),
- sky/fog (cloud + ToD already partly wired via `sun_clock`),
- rain overlay + wet-surface flag.
Themes subscribe to the parts that apply (Atlantis: wind→ocean; Canopy: wind→sway + rain + sun-shafts).

## Mesh-distortion (your seaweed / fabric / thin-branch / leaf bending)
All variants of the same **per-vertex sway**, differing in anchoring + frequency:
- **Seaweed / kelp:** anchored at the base, amplitude grows toward the tip (`amp ∝ height_fraction`), slow lateral sine + a current bias. (WATER-adjacent.)
- **Curtains / fabric:** anchored at the *top edge*, a travelling wave across the width (`sin(x*k - t*f)`), amplitude grows downward; add a slight billow normal to the cloth.
- **Thin branches:** anchored at the joint, stiffness ∝ radius (thin = more sway), inherits parent sway (hierarchical) so the tree moves as a whole.
- **Leaves:** the existing curl + a tiny flutter (`sway` + per-leaf phase) so individual blades shiver in wind.
A single `sway(worldpos, anchor, stiffness, t, wind)` helper covers all four; each caller picks anchor + stiffness.

### Sources
- Habel, Kusternig, Wimmer 2007 — *Physically Based Real-Time Translucency for Leaves* (forward-scatter, real-time leaf translucency).
- Tessendorf 2001 — *Simulating Ocean Water* (FFT + spectra + foam via Jacobian); Horvath 2015 (directional spectrum + swell).
- Gerstner/Rankine wave model (wind-driven, real-time-friendly summation).
