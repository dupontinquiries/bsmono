# FAO future — what to keep, what to shelf

After several sessions of v10 cleanup and an honest look at where FAO 
sits vs AV1, the project gets reshaped, not abandoned.

## Honest position

FAO does not beat AV1 on pixel compression. A 6567-frame 2560x1440 
encode produced 119MB; AV1 at libaom default settings would land 
~20MB. AV1 has had a decade of paid engineering optimizing exactly 
this problem; FAO can't catch that by competing on the same axis.

FAO's actual strengths sit on different axes:
- 3D worldview / spatial indexing of video content
- Cross-video library reuse
- Custom residual + strategy per frame
- Trailers carrying scene structure decoupled from frame bytes

## What gets shelved

The encode race specifically. v2/v8/v9/v10 fighting for fewest bytes 
per frame was the part trying to compete with AV1 on AV1's turf. 
Stop competing there.

## What survives

Library infrastructure (the strongest piece):
- `library.rs` and `library_db.rs` — Bitcask-style store
- WebSignature, eig0 index, canonical dedup, blob cache
- 5-slot backup + rollback
- Generalize to frame-level hashing for cross-video dedup over AV1

Worldview from webs:
- Mesh + pose + per-frame webs as a sidecar 3D index
- Built once at bundle time from AV1 reconstructions
- Real-time queryable at playback
- Step-into-the-video as the unique value proposition

Residual + DCT machinery:
- `transform.rs`, `residual.rs` 
- Useful as the learned-codec residual layer in Version C
- Frequency-aware quantization is reusable beyond codec context

## Versions discussed

**Version A — FAO wraps AV1.** Asset manager + worldview + cross-video 
dedup. AV1 is the pixel layer; FAO is everything else. Most realistic 
near-term path if returning to FAO.

**Version B — FAO inserts inside AV1 to do something AV1 doesn't.** 
Mostly fighting reality. AV1's intra-frame compression is already 
strong. The wins are at the cross-video level.

**Version C — neural codec observing AV1.** Run AV1, read its mode 
decisions / bit allocation / motion vectors from the bitstream, use 
those as priors for a learned codec that does cross-video dedup + 
quant-table representation. FAO library survives as the bundle store.

The shape sketched in conversation:
```
input → tensor decomp → hypernet + minmax quant → latent → output
```
With quant as the discipline that creates the discrete table of 
"slice configurations" = the learned analog of webs/meshes.

## Tier-1/2/3 cross-video dedup (Version A specifics)

**Tier 1 — exact match.** Loading screens, menus, cutscenes, replay 
UI. Content-addressed I-frame store. Plausible 5-15% saving on 
typical game footage libraries.

**Tier 2 — near match with delta.** Same loading screen with different 
progress bar. Same HUD different health bar. Hash matches static 
region; store delta for dynamic part. Uses existing FAO residual 
machinery. Additional 10-25%.

**Tier 3 — perceptual-hash buckets with neural guidance.** Small 
embedding network on frame features. Frames embedding close together 
but not exact stored as (anchor + residual). Anchor frames shared. 
Additional unknown amount; this is where "99% visuals with huge 
squeeze" becomes plausible for narrow content classes.

Realistic estimate for 500GB of game footage: Tier 1 alone 5-15%, 
Tier 1+2 = 15-35%, all three = 30-55%.

## What to revisit when returning

1. The library + library_db code is the gem. Keep it tested. It's 
   what enables every version above.
2. Worldview rendering. The 3D mesh-step-into-video capability is 
   unique among video tools. Worth keeping even when wrapped around 
   AV1 instead of competing with it.
3. The residual + DCT modules. Reusable as the residual layer in 
   any neural-codec-over-AV1 design.

## What to NOT do when returning

1. Re-litigate the encode race. v2/v8/v9/v10 was educational; the 
   architecture lesson is "stop competing on pixels."
2. Add new mesh-based encoding strategies. The mesh-as-primary-
   compression hypothesis is the part that didn't pan out.
3. Try to beat AV1 on standard rate-distortion. Won't happen.

## Current state of code

After v10 cleanup:
- 39 .rs files, ~48K lines, all brace-balanced
- v3/v4/v5/v6/v7 deleted (source + main.rs refs)
- Geometry + phase_correlation extracted as clean modules
- v8 mesh-rasterized + v9 volumetric + v10 orchestrator survive as 
  the encoding side
- v2/v2.1 baseline codecs survive
- Library/library_db/transform/residual are the reusable pieces
- Builds clean, GUI runs

The codebase is in good shape for either: continuing FAO work later, 
or harvesting pieces (library, residual, worldview) into a successor 
project.

## Trigger for return

When the AV1 batch tuner is stable and shipped, and there's interest 
in either:
- Building cross-video dedup on top of AV1 outputs (Version A)
- Researching neural codec direction with AV1 as guidance (Version C)
- The worldview / step-into-video capability standalone

FAO comes off the shelf for one of those, not for "let's improve the 
encode race."
