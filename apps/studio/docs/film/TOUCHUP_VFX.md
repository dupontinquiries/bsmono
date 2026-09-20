# TOUCHUP_VFX — the touch-up phase, looks, and cleanup

**Status key:** EXISTS = in Studio today. PLANNED = designed here, not built.
Never tell a person a PLANNED feature can do something yet.

## 0. The delivery sets the rules

The film was shot for a **4K release, with wide frames, in an older film
style**. That means:

- **No reframing to hide problems.** Cropping into a wide frame is a creative
  change to the composition and costs resolution the release needs. Treat
  anachronisms in the frame (modern cars, storefronts, LED lamps, phones) as
  cleanup, not as a reason to punch in.
- **Every fix has to hold up at 4K.** A fix that looks fine on the 960×540
  proxy can fail at full resolution. Check at 1:1 before calling it done.

## 1. How a touch-up pass goes in Studio

Editing and touch-up overlap. Don't wait for the whole film to lock.

1. **Lock per scene, not per film.** When a scene's cut stops changing
   (director signs off on S1, say), that scene enters touch-up while others
   are still being cut.
2. **Pick the take and the range.** Touch-up works on the clip's source range
   plus handles (at least 12 frames each side, 24 is better), so a small
   re-trim later doesn't expose unfixed frames.
3. **The fixed shot is a new VERSION of the same clip, not a new clip.** v001
   is the camera file; v002 is the cleanup render of the same range; v003 has
   the grade applied. The timeline clip keeps its id, position and trim and
   points at whichever version is chosen. This is how collaborators stay
   aligned: Studio addresses clips by stable id.
   *PLANNED: per-clip version stack. EXISTS: stable clip ids, `SetEffects`,
   and content-hashed sources that relink by hash.*
4. **Write a ticket for every shot that needs work,** in the plan's `vfx`
   list: shot, frame range in source time, the problem, the proposed
   technique, and a status (open / in progress / review / done).
5. **Re-check a ticket when its clip changes length.** If a re-trim moves a
   clip's range past its handles, the ticket reopens by itself (see §5).

## 2. Cleanup techniques, cheapest first

Use the cheapest technique that holds up at 4K.

| technique | when | status |
|---|---|---|
| Practical pickup | Prop in hand, needs to read as period (Max's phone in N8) | shoot it |
| Cut around | Problem is on screen < 1 s, or in a shot you can drop | EXISTS |
| Planar card | Flat things: poster, sign, label, screen (casting poster C0070–74 is locked-off: trivial) | PLANNED (corner-pin effect) |
| **Temporal clean plate** | Something passes through a fairly static background (car behind an actor, a pedestrian in a wide) | PLANNED, see §3 |
| Clean-plate patch | Static object in a locked-off shot; paint from a frame where it's absent, or a separately shot clean plate | PLANNED |
| Mask + roto | Object stays and moves: anchors and shapes, tracked | partly EXISTS (you have tried this) |
| Video inpainting (ProPainter-style) | Moving camera, object present the whole shot | external tool; bring the result back as a version |
| Keying | Planned green/blue, or a sky/window with clean contrast | PLANNED |
| Depth-aware haze and focus | Adding atmosphere or pulling focus in post; a monocular depth estimate feeds a mask | external model; PLANNED hook |
| Grain management | Every comp: degrain before the fix, regrain after, so the patch doesn't look smoother than the plate | PLANNED; regrain via the look chain |
| Deflicker | LED practicals flicker at 24 fps; rooftop lantern | ffmpeg `deflicker` as a v0 |

Heavier-VFX needs this film doesn't have but a bigger one would: 3D camera
solve (for set extension and 3D inserts), lens distortion match (undistort,
comp, redistort), relighting from normals, digital make-up and beauty work,
crowd duplication, rig and wire removal, and stereo/HDR mastering.

## 3. The interruption filter (temporal clean plate) — PLANNED

**Problem:** an actor stands against a soft bokeh street and a car crosses
fast behind them. The background is almost static; the car covers any given
pixel for only a few frames.

**Idea:** for each pixel, look along time at the neighbouring frames and take
the value the background has most of the time. A fast occluder is a
minority in the window, so the background wins.

**Pipeline:**
1. **Align** the neighbours to the current frame. Locked-off shots need
   nothing. A gently moving camera gets a global-motion warp per frame. The
   repo already has a global-motion estimator in
   `apps/av1tuner/av1tuner_core/src/gm/` (not yet integrated anywhere, per `docs/PICK-UP-HERE.md`), which is worth reusing rather than writing another.
2. **Detect the interruption.** Per pixel, the difference from the temporal
   median of the aligned window (radius k frames). Large difference = the
   pixel is being covered.
3. **Protect the actor.** A person-drawn protect mask, or a roto from the
   existing anchor/shape tools. The filter never touches protected pixels,
   because an actor who holds still would otherwise be "cleaned" into the
   background.
4. **Fill** covered, unprotected pixels from the aligned neighbour frames
   (the per-pixel median, or better, the neighbour whose local patch best
   agrees with the median). Feather the mask edge.
5. **Report confidence** per pixel: the share of the window that agreed with
   the chosen value. Where the occluder lasted longer than half the window
   (a slow car, a parked bus), confidence drops and the region is flagged,
   not silently smeared.

**Parameters a person sets:** window radius (default 6 frames at 24 fps);
difference threshold; protect mask; alignment on/off.

**v0 today, without writing code:** ffmpeg `tmedian=radius=6` on the shot,
merged over the plate through a mask (`maskedmerge`). This works for
locked-off shots only.

**Tests to write when it's built** (all synthetic, see TESTING.md):
- a textured static background plus a fast square crossing it: the output
  matches the background within tolerance where the square was;
- a static "actor" region inside the protect mask comes out bit-identical;
- a slow square (covering pixels for more than half the window) produces
  low confidence there, not a smear;
- performance: a 4K frame with radius 6 stays within a stated time and
  memory budget, measured and recorded, not guessed.

## 4. Where looks live

Keep three layers, the video equivalent of clip effects, busses and the
master chain in audio:

1. **Shot corrections** on the clip: exposure and white-balance fixes, so a
   shot matches its neighbours. Travels with the clip.
2. **Scene look** on an adjustment layer spanning a scene: the 80s childhood
   look for S1–S4, the 90s look for the rest, the stage look for P1/P2. It is
   anchored to the scene's first and last clips, not to timeline seconds, so
   when S2 gets shorter the 80s look ends where S2 ends.
3. **Master** at the output: the film-wide finishing (grain, halation, final
   curve) and the delivery transform. Applied once, last, like a mastering
   chain.

A "colour mastering clip group at the end of production" is the right
instinct for layer 3. Layers 1 and 2 must exist earlier, because the edit is
judged with the look on.

Status:
- **EXISTS:** per-clip `transform` and `color` effects. The preview shows
  `transform`.
- **Known gap:** the export (`studio_video::edit::render_args`) ignores all
  clip effects, so what the preview shows is not what the file gets. Fixing
  that comes before any look work.
- **PLANNED:** adjustment layers, the master chain, `grain` and `look` effect
  kinds.

Starting presets (tested on stills only; tune by eye on the real grade):
- **80s childhood:** warm, soft highlight glow (halation), lifted blacks,
  fine grain, light vignette.
- **90s:** slightly cooler shadows, a touch more contrast, less grain.
- **Stage:** warm spot, deep blacks, gentle glow.
- **Base conversion:** if the footage turns out to be shot in a log profile,
  the camera's conversion LUT goes first, below every look.

## 5. Tests for flows that must survive re-edits — PLANNED

Every one of these is a fold of ops over a small project, so they are cheap:

- trim a clip shorter: its scene look still ends at the scene's last clip;
  its VFX ticket reopens if the new range leaves the rendered handles;
- ripple-delete a clip in the middle of a scene: the adjustment layer
  shrinks and the next scene's look does not start early;
- move a clip to another scene: it takes the new scene's look, keeps its
  own shot correction;
- undo each of the above: bit-identical project to before;
- two collaborators trim the same scene concurrently: both converge, and the
  look span converges with them.
