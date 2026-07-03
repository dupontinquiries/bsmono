# Face-Aware Bandwidth Optimization

This document describes SnapClone's approach to using face detection
and (eventually) recognition to reduce the bandwidth cost of sending
photos and videos between contacts. It captures the design rationale,
the phase plan, the privacy posture, and what's deliberately out of
scope.

## TL;DR

Opportunistic, never-worse-than-baseline compression. When the
sender's device can confidently express a photo as "small delta
against past snap X that the recipient already has," send the delta.
When it can't, send the full image. The user never sees a degraded
experience; sometimes they consume less data.

The system trains on the user's own communication history. No
generic pretrained face-identification model. No stored biometric
features of contacts. The smallest possible biometric footprint
that delivers the bandwidth saving.

## Why bandwidth, not identity

Earlier scaffold framing was "face recognition for identity
matching" — recognize Alice across photos, tag snaps, suggest
recipients. That framing is wrong for SnapClone:

  - Identity matching needs a generic-trained model on stranger
    faces. That implies controversial training datasets, regulatory
    weight, and a feature that mostly serves users who want
    organizational tooling rather than communication.
  - Per-contact biometric embedding storage is the worst privacy
    posture: the user consents on behalf of contacts who never
    agreed to have their face geometry stored.

The bandwidth framing inverts both:

  - No generic model needed. The system trains (eventually) on
    photos the user has already received with explicit
    sender-attribution metadata.
  - The biometric data stored is the user's own face (clear
    consent) plus crops of contacts who voluntarily transmitted
    those photos to the user (implicit consent via the act of
    sending). Never crops of strangers, never of someone who
    didn't initiate the data flow.

This is a smaller, cleaner system that produces a real benefit
(less mobile data) for a feature users care about.

## Cold start is a feature, not a bug

The system is **opportunistic**. Day-one users see no behavior
change — every snap sends as a full image. Over weeks and months
of use, the local face library accumulates, and occasionally a
snap qualifies for compression. The user never knows the difference
except that their data bill is slightly lower.

This shape is critical because it means:

  - We can ship Phase 1 (data collection) before any compression
    works at all.
  - Failure modes (model uncertain, reference missing on recipient,
    decoder errors) all degrade to "send full image." Users never
    see broken faces in their snaps.
  - The protocol extension is purely additive — old clients
    receiving from new clients see normal snaps. Wire-format-tier
    fits naturally with the existing tiered-delivery design.

## Phase plan

### Phase 1: Local face library (this turn)

Pure data collection. No compression yet, no model yet.

Inputs:
  - When the user captures a selfie (front camera, photo or video
    frame), the detector identifies the face, the crop is stored
    under label `self`.
  - When a snap arrives from a known sender, the detector runs on
    the decoded image. If a clear face is found, the crop is stored
    under label `contact:{contact_id}`.

Storage:
  - Per-label cap (default 50 crops per person). LRU eviction when
    full.
  - Total disk budget (default 50 MB, tunable). LRU across the
    whole library when over budget.
  - On-disk encryption using existing per-device storage keys
    (defense against opportunistic device-loss data leak).
  - Wiped completely when consent is withdrawn.

What this enables:
  - We have a local labeled dataset growing over time. Even before
    Phase 2 lands, the library is useful for opportunistic photo
    library organization features.
  - Phase 2 has data to work with from day one of its deployment.

### Phase 2: Reference-based delta compression (no ML)

When sending a photo to Bob, the sender:

  1. Runs face detection. If no clear face, send full image as
     today.
  2. If face found, looks up the local library for past snaps
     sent to or received from Bob containing the same person's
     face (initially: only `self` snaps trigger this — sender's
     own face going to a recipient who has past selfies of the
     sender on their device).
  3. Picks the most similar past snap (via perceptual hash of the
     face crop) as a reference candidate.
  4. Negotiates with recipient: "I have reference X (hash h). Do
     you have it?" If yes, encode the new snap as a content-aware
     delta against X (background mostly identical; only the face
     and pose differ). If no, fall back to full image.

Compression is via **standard image codec applied to the delta**,
not neural. WebP / AVIF with a reference frame. AV1 can encode
single-frame deltas against a reference. No reconstruction model
needed — the decoder is just "decode the codec, add to reference
pixels."

Expected savings: 2-5x for selfies where the same person occupies
~30% of the frame, the background is similar, and the codec can
find efficient deltas. Less for action shots, more for "talking
head" style content.

Negotiation protocol (sketch):

  - Sender includes in snap header: `reference_hash` (32 bytes,
    SHA-256 of canonical-form reference crop)
  - Recipient checks local library for matching hash
  - If found, decodes delta against reference; if not, requests
    full image
  - Round-trip cost is one extra RTT for the negotiation;
    avoidable when sender already knows from past delivery
    receipts that the recipient has reference X

### Phase 3: Neural personal model (deferred, research-track)

Only if Phase 2 measurably helps in real use. Trains a small
on-device model (head on top of MobileNetV2 or similar ImageNet-
trained backbone) to produce compact face representations.
Uses `burn` or `candle` for on-device training.

This is where the original "neural face codec" idea lives, but
**only after** we have data on whether Phase 2's classical-codec
approach is enough. The simplest thing that works is the right
thing to ship first.

Open questions for Phase 3 (track in TODO):
  - Which training framework
  - Backbone choice (MobileNetV2 vs MobileFaceNet vs custom)
  - Update cadence (every N samples? overnight while charging?)
  - Cross-device determinism strategy

## Privacy posture

Three commitments:

  1. **Stored data is user-originated.** The library only contains
     crops from photos the user took (selfies) or photos the user
     received from someone who chose to send them. Never photos
     scraped, never photos of strangers, never images outside the
     SnapClone communication graph.

  2. **No biometric data leaves the device.** Phase 1 stores
     image crops. Phase 2 transmits opaque image-codec bytes
     (delta payload) plus a reference hash. Phase 3 (if ever)
     uses model weights that live only on the device that trained
     them. Even the personal model never goes off-device.

  3. **Wipe is always one click.** Consent withdrawal:
     a. Sets the persistent `face_features_enabled` flag to false
     b. Wipes the entire local face library
     c. Wipes any trained model state (Phase 3)
     d. Future incoming snaps continue to work; they just won't
        contribute to a rebuilt library until consent is restored

## Wire format

Phase 1: no wire format changes. Pure local feature.

Phase 2: extends snap header with optional fields, all backwards-
compatible (old clients ignore the fields, get full image):

```
SnapHeader {
  // existing fields...
  reference_hash: Option<[u8; 32]>,  // SHA-256 of reference crop
  delta_codec: Option<DeltaCodec>,    // WebP-Ref / AVIF-Ref / etc.
}
```

Recipient logic:
  - If `reference_hash` is present AND recipient has matching crop
    AND recipient supports `delta_codec` → decode as delta
  - Otherwise → request retransmission as full image, OR sender
    proactively sends both (depends on transport)

Phase 3 (if): adds `model_version` and `embedding_hash` for
neural-reconstructed snaps.

## Consent boundaries

The user is asked to opt in once, with clear language:

> SnapClone can use the photos you send and receive to reduce
> data usage on future snaps. This works by remembering recurring
> faces (yours and your contacts' as you message them) so similar
> photos can be sent more compactly. All data stays on this
> device. You can turn this off at any time, which will erase
> everything that was remembered. [Enable] [Not now]

The opt-in is for the whole face-features track, not per-Phase.
If users say yes, they get Phase 1 data collection immediately;
Phase 2 compression starts working as soon as there's enough
data; Phase 3 (if ever shipped) is enabled the same way.

If consent is denied or revoked:
  - No detection runs on captured or received images
  - No crops are stored
  - Existing snap delivery works unchanged
  - The user can re-enable later, library starts empty

## What's deliberately NOT in scope

Even at full ambition, this system does NOT:

  - Tell the user "this photo is of Alex" (no UI for identity
    matching, even though Phase 3's model could in principle
    do it — the feature scope is bandwidth, not organization)
  - Distinguish strangers from contacts (no general face
    recognition)
  - Generate or synthesize faces from prompts (no generative
    component)
  - Run any inference when consent is off
  - Send anything face-related off-device except the opaque
    compressed snap bytes

## Crate alignment

The existing `snapclone-face` crate was scaffolded around the
generic-pretrained-ArcFace direction. We keep the crate name and
the `Detector` (face detection is still useful) and rebuild the
storage/matching layer around the new purpose:

  - `detector` — unchanged, still wraps a YOLO-class face detector
  - `library` — new module, replaces `identity`. Stores actual
    image crops per label rather than 512-dim embeddings.
  - `consent` — unchanged
  - `reference` — new module for Phase 2. Reference selection,
    hash computation, negotiation helpers.
  - `embedding` — DEFERRED. The scaffold remains but is no longer
    on the critical path. If Phase 3 ever ships, embedding logic
    lives here. For Phase 1+2 it's unused.

Documentation in lib.rs is updated to reflect the new direction so
the next contributor doesn't follow the ArcFace breadcrumb.

## Implementation order

1. Design doc (this file)
2. Rebuild `snapclone-face::library` for Phase 1 storage
3. Update `snapclone-face::lib.rs` documentation
4. Add `snapclone-face::reference` skeleton (real impl in a
   later turn)
5. Move `identity` to "deferred" status
6. Wire detection output → library storage in C++ photo paths
   (separate turn — once Rust API is settled)
7. Phase 2 negotiation protocol (separate turn after deployment
   of Phase 1)
8. Phase 3 model decisions (later)

## Video ROI bit-allocation (V32.34+, implemented)

The photo delta-compression above (Phase 2) is distinct from face-aware
**video** encoding, which is now implemented end-to-end.

The original video path applied a single ffmpeg `addroi=...:-1` filter:
it only BOOSTED quality inside the detected face box (lower QP) and did
nothing to the background. Under bitrate rate-control that barely shrinks
the file — and `qoffset=-1` (the maximum) over-spends on the face. The
"compresses the background harder" the design promised was never done.

The improved path makes the bit reallocation explicit:

  - `sc_roi_plan` (Rust, `snapclone-saliency::roi_plan`) turns the
    saliency box into an ORDERED plan: (1) a background QP PENALTY over
    the whole frame (compress harder where the viewer isn't looking),
    then (2) a PADDED face QP BOOST that protects the subject from that
    global degradation. ffmpeg addroi-chain semantics mean the later
    (face) directive overrides the earlier (background) in the overlap.
  - Net effect: bits move FROM background TO face → smaller file at equal
    perceived quality, the actual point of face-aware compression.
  - The face box is PADDED outward (default 30%) so the protected region
    contains the whole head and the QP discontinuity lands in low-detail
    background rather than slicing through the chin/ears.
  - When the face already dominates the frame (>60% area), the background
    penalty is SKIPPED — penalizing a tiny "background" would just wreck
    most of the visible image.
  - All geometry is frame-clamped (defends the rotation-mismatch overshoot
    the static-box path hit) and unit-tested in Rust; the C++ encoder just
    consumes the plan via `sc_roi_plan`.

Tunables (`RoiTuning`, defaults net-save on talking-head/selfie video):
face_qp_centi=-30, background_qp_centi=+25, face_pad_centi=30,
max_face_area_for_bg_penalty_centi=60.

Still a STATIC box from one representative frame — a moving subject would
need per-frame ROI (the in-process libav* rewrite, filed in TODO). The
plan improves the gain for the common stationary-subject snap without that
larger rewrite.
