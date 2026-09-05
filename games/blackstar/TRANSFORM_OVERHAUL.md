# Why the in-game transform looks wrong

Read from `game.c` around the fold blend and `robot_parts.c`. The engine is not
missing a transform system — it has hinges, a parent chain, and per-part fold
magnitudes. Four specific things make it look bad, and each has a named fix.

## 1. It lerps SCALE, so parts morph instead of moving

```c
base_poses[i].sx = (a.sx + (b.sx-a.sx)*e2) * fade;
base_poses[i].sy = (a.sy + (b.sy-a.sy)*e2) * fade * crimp;
```

Robot pose `a` and vehicle pose `b` are blended by interpolating each part's
**scale**. A part that is a long shin in robot mode and a short exhaust in
vehicle mode does not travel — it *stretches* between the two. That is the
"slides against each other unrealistically" reading: nothing is moving rigidly,
it is all being squashed in place.

A real transformation is **rigid**: parts rotate and translate, never scale. That
is also why it can be verified — `model_verify::verify_pair` proves a rigid
motion is lossless, and no scale-lerp can pass it.

There is even an extra squash on top:

```c
float crimp = 1.0f - fold*0.12f;   // slight mechanical compression mid-fold
```

An arbitrary 12% compression applied to every part mid-fold. It is there to make
the morph read as mechanical, which is a symptom of the morph being wrong rather
than a fix for it.

## 2. The fold is 2D

```c
float dy = pv*(1.0f - cosf(th)), dz = -pv*sinf(th);
```

Every hinge rotates in the Y–Z plane only. Nothing folds sideways, nothing
twists. Real transformations rotate about arbitrary axes — a door panel swings
outward *and* rotates as it goes. `motion::axis_angle` already does this
(Rodrigues, any axis); the engine needs the axis per joint rather than an
implied one.

## 3. Everything moves on one clock

```c
float th = fold * fmag * fdir;
```

`fold` is a single global 0→1. Parts differ only in *how far* they rotate, never
in *when*. So all 20-odd parts start and stop together — which is exactly the
"parts just fly around" impression, because a real transformation is a
**sequence**: big panels commit first, detail follows into the space they
opened, and things *stop* while others move.

`motion::schedule` is this: a per-joint window, parent never starting after its
child, widest swings first. The bend-and-stop is the pauses.

## 4. Basic shapes and stray greebles

`gen_props.py` compiles procedural part templates, and `greeble_boost` in
`chassis.json` scales surface-detail density — density, not placement. Detail
scattered by density rather than attached to structure is where "random
greebles" and z-fighting artifacts come from: two procedural plates at the same
depth flicker.

The fix is the imported geometry. `anatomy::body_plan` finds real parts at
74–85% coverage, and `unwrap` can give them UVs. A greeble that is part of an
authored mesh is in the right place by construction.

## The overhaul, in order

1. **Stop lerping scale.** Blend rigid transforms — rotation and translation
   only. This alone removes the morph, the crimp hack, and most of the
   interpenetration.
2. **Give each joint an axis.** 3D hinges via `motion::axis_angle`.
3. **Schedule the joints.** `motion::schedule` replaces the global `fold`, so
   parts move in sequence and hold still between moves. `fold_spin` and
   `xform_dur` stay as the per-chassis character.
4. **Swap procedural parts for imported geometry**, slot by slot, using the
   anatomy pass. Detail then comes from authored meshes, not density.
5. **Verify.** Every stage of the animation is a rigid transform of the source
   mesh, so `model-verify` can prove no vertex was lost or stretched — and a
   mirrored part that would render inside-out is caught by the determinant
   check.

The work already exists for steps 2, 3 and 5; it was built for a derivation that
turned out to be unnecessary, and it is the right medicine for this instead.

## UPDATE (c15unified session): confirmed still exactly accurate, unchanged

Got the C engine building for the first time in this bundle this session
(60 files missing from the Makefile's object list — separate commit). With
a real build in hand, checked directly rather than assumed: is the specific
bug this document quotes — `base_poses[i].sx = (a.sx + (b.sx-a.sx)*e2) *
fade;`, the `crimp` compression hack — still live in `game.c`, or has it
since been addressed?

It's still there, unchanged, same lines, same mechanism (`game.c` ~6931-6938).
So this document's core technical finding is still exactly correct today,
not stale — worth stating plainly since this session found several other
"still open" claims elsewhere in this repo that had quietly stopped being
true. This one hasn't.

What HAS changed since this doc was written: `rust/foldview` (see
`TRANSFORM_SYSTEMS.md`'s own update) proved the single-mesh anatomy-based
approach this overhaul's steps 2-5 depend on actually works, on real
imported vehicle geometry, not just the procedural bot parts step 1-4 were
originally scoped around. That derisks steps 2-5; it doesn't touch step 1
(stop lerping scale in `game.c`), which is still exactly the same
unattempted rewrite it was when this doc was written — and still not
something to do blind in this sandbox, since there's no way to see the
result (Xvfb's built-in GLX can't satisfy this SDL2 app's
`SDL_GL_ACCELERATED_VISUAL` requirement — confirmed by reading
`exengine/render/window.c`'s own `SDL_GL_SetAttribute` calls, not just by
the earlier failed attempts).
