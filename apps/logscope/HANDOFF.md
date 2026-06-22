# logscope-suite — mind-palace handoff (for a fresh chat)

This is a working handoff. Read it, then continue. The project renders a conversation/agent log
as a navigable 3D "mind palace": each turn is a node on a strand, dispatched into themed scenes.
**The most important thing to preserve is the image/render verification loop in §3 — the user
considers that workflow perfect, so use it on every visual change.**

---

## 1. What & where

- **Working copy:** `/home/claude/logscope-suite` (this is the live tree you edit/build).
- **Original upload:** `/mnt/user-data/uploads/logscope-suite.zip` (re-extract only if the working copy is gone).
- **Crate of interest:** `crates/logscope-egui` (binary `scene-view` is the render harness).
- **The user builds in-app on their own GPU** with `--features wgpu-engine,llamacpp,gui`; `gpu-scene` is on by default. Your `scene-view` uses software GL (llvmpipe).

## 2. Environment, build, test

```bash
source /home/claude/env.sh                       # Rust 1.96 in /home/claude/.local — REQUIRED first
cd /home/claude/logscope-suite
cargo build --features gpu-scene --bin scene-view # 5–100s
cargo test  --features gpu-scene -p logscope-egui --lib   # → 257 pass
```
- Test note: `sandbox::tests::offline_first…` is flaky/ignorable.
- bash_tool is **sh/dash**, NOT bash: no `[[ ]]`. Put any render loop in a `#!/bin/bash` FILE under `/home/claude/` and run it, rather than inlining bash-isms.
- pip needs `--break-system-packages`. Network allow-list is limited (no general internet for bash).

## 3. ⭐ THE IMAGE / RENDER WORKFLOW (preserve this — it's the part the user loves) ⭐

The loop is: **spin up a virtual display → run `scene-view` with env vars that pick the
scene + camera → screenshot with ImageMagick `import` → montage labelled tiles → `view` the PNG
inline → judge → iterate.** Always write it as a bash FILE (sh can't do the loop cleanly).

Template (copy, edit the `for` list, run with `timeout`):

```bash
cat > /home/claude/r.sh <<'EOF'
#!/bin/bash
cd /home/claude/logscope-suite; . /home/claude/env.sh
pkill -9 Xvfb 2>/dev/null; pkill -9 scene-view 2>/dev/null; sleep 1
Xvfb :99 -screen 0 1180x780x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 & sleep 2
i=0
for SPEC in "Atlantis|0.5,0.06,13" "Treehouse|1.5,0.18,15"; do   # "THEME|yaw,pitch,dist"
  i=$((i+1)); TH="${SPEC%%|*}"; ANG="${SPEC##*|}"
  DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
    LOGSCOPE_THEME="$TH" LOGSCOPE_DOF=0 POS_ANGLE_INIT="$ANG" \
    target/debug/scene-view >/home/claude/r_$i.log 2>&1 & P=$!
  sleep 13                                   # let it draw (~12–14s on llvmpipe)
  import -display :99 -window root /home/claude/r_$i.png 2>/dev/null
  convert /home/claude/r_$i.png -gravity South -background '#0a0f12' -splice 0x24 \
    -gravity South -fill '#cfe0e8' -pointsize 18 -annotate +0+2 "$TH" /home/claude/rc_$i.png 2>/dev/null
  kill $P 2>/dev/null; sleep 2; pkill -9 scene-view 2>/dev/null; sleep 1
done
pkill -9 Xvfb 2>/dev/null
montage /home/claude/rc_1.png /home/claude/rc_2.png -tile 1x2 -geometry +5+5 -background '#05090b' /home/claude/grid.png 2>/dev/null
echo done
EOF
chmod +x /home/claude/r.sh
timeout 120 /home/claude/r.sh
```
Then **`view /home/claude/grid.png`** to read it inline, and iterate.

**scene-view env vars:**
- `LOGSCOPE_THEME=<name>` — Atlantis, Treehouse, Canopy, Mistfall, Reading Room, Cloudscape, Forest, Folding@home, Slate, Library, Archive.
- `POS_ANGLE_INIT="yaw,pitch,dist"` — camera orbit (e.g. `"0.7,0.25,16"`; higher pitch looks down).
- `LOGSCOPE_DOF=0` — disable depth-of-field for clean stills (`1`/value to test it).
- `LOGSCOPE_NO_POST=1` — disable the whole post chain (bloom/godray/shoulder/DOF) → see **raw albedo**; use this to tell "is the geometry there?" apart from "is it just bloom?".
- `LOGSCOPE_SEED=<n>` — seeds a synthetic graph → changes `env_params` (node count/segments/branches/first-preview hash → `seed`). Use different seeds to test graph-derived/global procedural factors (e.g. Treehouse canopy height, Atlantis hall size).
- `LOGSCOPE_ROOM=<0-15>` — render one of the 16 survey ROOMS instead of the themed scene.

### ‼️ The single most important caveat
**llvmpipe (software GL) OVER-BRIGHTENS and over-blooms badly vs the user's real GPU.** Flat lit
surfaces (esp. STONE walls, fog) routinely blow out to near-white in `scene-view` even with
`LOGSCOPE_NO_POST=1` — that is a renderer artifact, NOT the real look. So:
- Use renders to verify **geometry, layout, silhouette, animation, color hue, what-exists** — these are reliable.
- Do **NOT** trust llvmpipe for **absolute brightness / whether something "blows out"**. For brightness, **trust the user's in-app GPU reports.** When you change brightness/fog/exposure, say plainly that it needs their on-GPU eyes.

## 4. Git + deliverable

```bash
GIT='git --git-dir=ai-git --work-tree=. -c user.email=ai@local -c user.name=ai'
$GIT add -A && $GIT commit -q -m "…detailed message…"     # commit every change set
```
Package + share (do this at the end of a change):
```bash
cd /home/claude
rm -f logscope-suite-tree.zip /mnt/user-data/outputs/*.zip /mnt/user-data/outputs/*.png
zip -rq logscope-suite-tree.zip logscope-suite \
  -x 'logscope-suite/target/*' -x 'logscope-suite/crates/gpu-gemm/target/*' -x 'logscope-suite/ai-git/*'
cp logscope-suite-tree.zip /mnt/user-data/outputs/
cp /home/claude/grid.png   /mnt/user-data/outputs/<descriptive>.png
```
Then call **`present_files`** with the PNG first, the zip second. Keep the closing message short.

## 5. Key files (crates/logscope-egui/src/)

- **scene3d.rs** (~2290 lines) — scene builders + dispatch. `build(palette_name)`: if `LOGSCOPE_ROOM` set → `room_variant(idx)`; else `clear_colliders()` then `match name → builder`. `enclose()` (generic r40 shell) applied to all EXCEPT `Atlantis, Cloudscape, Forest, Treehouse, Canopy` (self-contained/open). Helpers: `c(r,g,b)`, `v(x,y,z)`, `addv/mulv/normv`, `hash(x,s)->f32`, `ez()`, `scale_rgb`. `env_params()` reads thread-local `EnvParams{nodes,branches,segments,seed,ext_x,ext_z,bear}` (set per-frame in chat.rs ~1293; `seed` computed ~1259 from node count/segs/branches/first-preview hash). Scene helpers: `contact_shadow` (DIRECTIONAL — offsets toward anti-sun), `colonnade_ring`, `waterfall`, `standing_stone`, `reading_desk`, `grass_tuft`, `flower`, `bird`, `bee`, `firefly` (twinkles via shader), `tree_platform`, `rope_bridge` (catenary + planks w/ holes + sway), `shelf_wall` (moss stone bookshelf w/ chunky book-slates), `dragon_head` ← **about to be written, see §7**. Builders: `canopy` (tree + library-maze of shelf walls), `mistfall`, `archive`, `reading_room`, `forest`, `treehouse`, `cloudscape`, `atlantis`, `foldingathome`, `poseidon_statue`.
- **mesh3d.rs** (~1010) — primitives: `node_mesh(NodeStyle Orb/Stone/Leaf/Pearl)`, `label_cells` (text as 5×7 emissive-sphere glyphs; cap **32** chars; centers on origin), `tube,disc,torus,annulus,dome_cap,paper_fold,arch_window,rock,strand,cairn,uv_sphere,leaf_cluster,panel,tint_up,tint_height,scale_xz,with_mat,set_alpha`, `Light{dir,color,sky,ground,ambient}`, `Float{amp,speed,phase,drift}` (animates POSITION only; `Float::STILL`).
- **gpuscene.rs** — GLSL scene + post (compiles #version 300es AND 330). Scene FS: triplanar materials; water block; MATERIAL VARIATIONS (MOSS leaf translucency, PAPER back-glow, GLASS fresnel); **emissive applied ~line 325 with a TWINKLE for bright MOSS motes (v_emissive>1.6)**. Composite FS: scene+bloom+godray + DOF (depth texture) + **SOFT HIGHLIGHT SHOULDER (knee 0.78) before the final clamp**.
- **glow_scene.rs** — GL renderer; scene depth is a sampleable texture; DOF uniforms.
- **tabs/chat.rs** (~3700) — `spatial_ui()` the shared 3D render. Node mesh layer ~1505: per-theme `NodeStyle` + (strand_mat,node_mat); Treehouse nodes = WOOD BALLS on ropes. PER-SCENE LIGHTING via `scene_light(name)` (free fn at END of file → (sky,ground,ambient,exp_mul)). PHYSICAL UI PANEL (~1660, gated `if !self.backdrop`): per-theme panel rgb/mat/emissive/alpha/text; bezel frame (Atlantis = glowing cyan); left accent strip; message text **LEFT-ALIGNED, pixel-stable, scale 0.030**, 20-char tail, blinking caret. Fog per theme ~1757 (Mistfall 0.055, Treehouse 0.030, Atlantis 0.009, Folding 0.014, else 0.017) ×grade. Exposure ~1764.
- **theme.rs** — `PALETTES` array; `variant()`/`grade()`. Themes incl. CLOUDSCAPE, FOREST, TREEHOUSE.
- **Cargo.toml** — `default=["gui","gpu-scene"]`.
- **TODO.md** (repo root) — dated running log + the full feedback backlog (prepend each pass).
- **RESEARCH_weather_light.md**, **DESIGN_generative_scenes.md** (repo root) — research + the (parked) sanitized-RoomSpec generative-architecture plan.

## 6. What's been done (most recent first)

- **Treehouse polish:** shader-twinkling bioluminescent fireflies; denser forest + fog LOD blot-out; thicker bridge planks with procedural holes; procedural moss on trunks/platforms.
- **Canopy redesign (structural):** one tree in an open mossy circle + a fanning MAZE of moss-stone book-slate shelf walls (concentric broken arcs + radial spokes); `shelf_wall()` helper; dimmed cozy light; skips enclose. *Needs on-GPU brightness/contrast check.*
- **In-app feedback pass 1:** soft highlight shoulder (composite); Reading Room/Cloudscape/Atlantis brightness down; removed Atlantis flying books; Mistfall water-over-ground + heavier fog.
- **Treehouse global height factor** (graph-derived, like Mistfall's rune colour) + **UI text fixes** (label_cells cap 16→32; left-aligned pixel-stable; font −12%; wider plaque; caret) + Atlantis panel polish (glassier, cyan bezel).
- Earlier: Treehouse scene + wood-ball nodes; 16-room engine; per-scene lighting; Cloudscape/Forest scenes; Archive→marble cathedral; Mistfall standing stones+waterfalls; depth-of-field; material variations; directional contact shadows; bees.

## 7. ⏭ IMMEDIATE NEXT TASK (mid-build — pick up here)

**Atlantis sea-dragon statues + gargoyles ("dragon forge").** I was about to write a procedural
**`dragon_head(s, pos, face, sc, seed, col, col_lo)`** generator in scene3d.rs (a sea-dragon head:
skull + 3-seg tapering snout + lower jaw + teeth + brow + glowing eyes + 2 swept-back horns + a
frill of crest spines + cheek gill-fins, STONE material). Then:
1. In `poseidon_statue()` REPLACE the human head+beard (`ball(sc, head, 0.26)` + beard ball, ~line 678) with a `dragon_head(...)` call at the head position → sea-dragon-faced statues.
2. Reuse `dragon_head` at small scale for a ring of **GARGOYLES** perched on the Atlantis cornice/balcony rim (in `atlantis()`), facing outward.
Build → render Atlantis close on a statue + on the dome rim → commit → package → present.

## 8. Remaining backlog (queued; each ≈ its own turn)

- **Atlantis:** (sea-dragons/gargoyles above); **fix gaps in the ceiling structures**; **remove "the rings that cut through the middle area"** — ⚠️ ASK the user WHICH rings (candidates: floor-mosaic concentric annulus rings / architrave+cornice torus bands / balcony walk ring). Flying books already removed; whites eased via shoulder (verify on GPU).
- **Mistfall:** cane walls; far-off **anti-gravity waterfalls** that fall into mist then loop back + recondense. (water-over-ground + heavy fog done.)
- **Cloudscape:** wispier fog; occasional **storms with multi-colour lightning arcs tuned globally to content**; **night variation with moonlight**. (brightness lowered.)
- **Forest:** many more tall thin trees; ground leaves + **persistent fallen leaves (30 min then decay + ragdoll pileup → needs a timed/stateful system)**; **rabbits + cats darting between bushes (simple AI)**; extend size + heavy LOD fog blot-out (not walls).
- **Treehouse (remaining):** fuzzy creatures swinging; **wood GRAIN + surface deformation (shader/material)**; varied wind gusts + trickle rain.
- **Folding@home concept:** DNA strand that **spits out proteins as the graph nodes**.
- Also parked: per-ROOM lighting (rooms inherit theme light); a unified `Weather`+`sway()` system (kelp/branches/leaves/fabric); RoomSpec JSON + sanitize → the parked LLM/img2img/point-cloud generators (DESIGN_generative_scenes.md); true pixel-perfect egui-input overlay onto the 3D text; full shadow-MAP pass.

## 9. User preferences (carry these)

Procedural generation, no external art assets yet. BATCH changes. Minimal-but-honest commentary with
explicit limitations/deferrals — when llvmpipe can't judge brightness, SAY so and defer to their GPU.
Likes open scenes, physically-real "pearls", iterative aesthetic refinement. Reads inline render images
and gives rapid per-scene feedback. They are now testing **in-app on a real GPU**, so trust their
brightness reports over llvmpipe. They say "keep working" to mean: proceed through the backlog without
waiting — but still surface the one open question (which Atlantis rings) when you reach it.
