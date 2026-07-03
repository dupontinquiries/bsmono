# Session brief — deferrals, tools, and harnesses not yet in the zip

> Filed verbatim from the session that produced the 0623j base. This captures state that
> the root `HANDOFF.md` does NOT (HANDOFF's §7 still reads as the old dragon-forge task;
> the real scene work has moved far past it — Bifrost/Titan/Oz battlefields, Mansion
> Garden, the Alley scene, camera FSM, power modes, LOD). Treat this brief + `TODO.md` +
> `FIELD_NOTES.md` as the live state; HANDOFF.md as stale on §6–§8.

Quick capture at end of session. Covers (1) what I did **not** get to, (2) the **tools/knobs** in play, (3) **harnesses** that exist only as ad-hoc bash and aren't packaged.

---

## 1. Not done this conversation (deferred / backlog)

**Rendering fidelity (blocked by software GL — needs a real GPU to validate):**
- True **PBR materials + real reflections** — needs new materials in the shared `texgen` array; can't be validated under llvmpipe.
- **Bump/normal, power modes, dynamic LOD, marble/sandstone/Oz brightness** — all built but only user-verifiable on a real GPU (llvmpipe over-brightens light palettes toward white).
- **Storytelling non-uniform lighting** (essay): flickering-fluorescent vs golden-hour shaft — needs shader accent lights, not just emissive geometry. Aurora in Bifrost is the only piece done.

**Content not built:**
- **3 of 5 battlefield briefs** remain (same POI set-piece pattern as Bifrost/Titan): Sandstone Execution Gauntlet (funnel walls + snapped shrapnel-embedded obelisk), Shattered Dragon Corpse Canyon (beast-carcass bridge + pagoda fort), Blood-Soaked Colosseum Trenches (ring wall + cratered no-man's-land). The two desert/blood ones will wash under llvmpipe.
- **More recursive maps** (trivial from existing generators): recursive reef/coral, recursive root/mycelium network, Menger/Sierpinski megastructure.
- **Glyph textures** for sign / café / panel **text** (readable names) — needs per-mesh texturing.
- **Micro-narrative decay props** with radiating damage (soot radius, peeled metal, shattered-glass scatter) — essay's "square-foot" decay, only partially done via `ground_wear` + `organic_boundary`.

**Camera / interaction:**
- Camera FSM: **deeper occlusion-raycast POI targeting + true cinematic passthroughs** — the Push-in shot approximates; there's no real visibility solver yet. Shots read context via graph AABB + strand spline + collider evasion only.
- **Canopy hang**: explicit **rope-to-branch meshes** not drawn (anchors just sit at branch height so they read as hung).
- **Physical-UI interactive node-selection** wiring; **freecam**; **voice / speech-to-text (Whisper)**.
- Essay items that don't map onto a non-combat mind-palace: verticality/sightline **flow** (→ atmosphere here), **asymmetrical-symmetry** balance (N/A).

**Polish noted but skipped:**
- **Oz** storybook contrast pushed harder (deeper grass, more orange-gold brick) so it reads under llvmpipe.
- **Forest** distant trees read "bare" when LOD culls small leaf clusters (trunks survive).
- **Mistfall** volumetric water; further passes on Cloudscape / Archive / Reading Room / Atlantis.
- Organic boundary on the supersized worlds sits at the far-clip/fog horizon — reads as skyline, not a close wall.

---

## 2. Tools / knobs in play

**New env knobs added this session** (documented in `RUN_ON_DEVICE.md`):
- `LOGSCOPE_POWER=full|half|quarter` — battery saver: render scale 1/0.5/0.25, texture-tier cap, scatter thinning, tighter cull.
- `LOGSCOPE_NO_CULL=1` — disable frustum culling.
- `LOGSCOPE_LOD=<f32>` / `LOGSCOPE_NO_LOD=1` — dynamic per-mesh LOD threshold / off.
- `LOGSCOPE_RENDER_SCALE=<f32>` — explicit 3D render-resolution scale.

**Pre-existing knobs used:** `LOGSCOPE_THEME`, `LOGSCOPE_CONVO`, `POS_ANGLE_INIT="yaw,pitch,dist"`, `LOGSCOPE_NO_POST=1` (read geometry, no bloom), `LOGSCOPE_GEOM_DUMP=1`, `LOGSCOPE_WORLD_ONLY=1`, `LOGSCOPE_SEASON`, `LOGSCOPE_HOUR=0-24`, `LOGSCOPE_QUALITY`, `LOGSCOPE_FLY` / `LOGSCOPE_FLY_AUTO` / `LOGSCOPE_FLY_DIST` (the camera FSM rides these), `LOGSCOPE_PEARL_POS`.

**`rustcheck`** (`/home/claude/toolkit/rustcheck --fast <file.rs>`) — instant brace-drop static catch; run after every edit before `cargo build`. **Already in the zip** under `reference-docs/rustcheck-tool`.

---

## 3. Harnesses that exist only as ad-hoc bash (NOT yet packaged)

These I ran by hand every turn; none are committed as scripts, so the next session re-types them. Worth capturing as `tools/` scripts:

1. **Single-scene render harness** — the `Xvfb :99 … llvmpipe` + backgrounded `scene-view` + `import -window root` + kill pattern (one scene per fresh Xvfb, ~18–22 s sleep for dense/recursive maps). Pure ad-hoc bash.
2. **Brightness probe** — `python3 -c "from PIL import Image,np; print(np.array(Image.open(P).convert('L')).mean())"` to detect black frames (≈0 = not loaded / camera inside geometry) and llvmpipe wash (>150 = blown). Ad-hoc one-liner.
3. **Detached release-build + split-poll** — `setsid … cargo build --release … ; echo BUILD_EXIT=$?` then short poll loops (a single long poll hits the exec-time limit). Pattern only lives in the handoff prose.
4. **Package recipe** — the strip → copy both binaries → refresh bundle/PNGs/reference-docs → versioned `zip` from `/home/claude`. Documented in the handoff but not scripted.

**Already in the zip:** `tools/geom_check.py` (obj_clip strand-vs-scenery metric), `HARNESS-SCORECARD.md`, `RUN_ON_DEVICE.md`, the git bundle, `reference-docs/` (HANDOFF*.md, the two forge HTMLs, turn9 patch, garden concept png, rustcheck-tool).

**Suggested next packaging step:** fold items 1–4 into `tools/render.sh`, `tools/brightness.py`, `tools/relbuild.sh`, `tools/package.sh` so the workflow is reproducible instead of re-derived from prose each session.
