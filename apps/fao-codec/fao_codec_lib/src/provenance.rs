//! # provenance — tamper evidence that says WHAT changed, not just that
//! something did
//!
//! Motivated directly by the scene-editing work in `v11_plate`: once a
//! narrative edit costs 0.9% of the file and is expressible as a track
//! rewrite, "is this the video that was recorded" stops being academic.
//!
//! ## What this is and is not
//!
//! It is **tamper evidence and provenance**: a digest published at
//! encode time lets anyone later show a file is or is not the one that
//! was committed to. It is **not** proof that a camera saw reality.
//! Nothing computed from pixels can establish that — you need secure
//! capture hardware attesting at the sensor, which is what C2PA /
//! Content Credentials specifies and what this should eventually carry
//! rather than replace. Stating the boundary matters because a
//! "verified" badge that only means "unedited since encode" is
//! actively misleading if a synthetic source was encoded in the first
//! place.
//!
//! ## Why the obvious design (hash the bitstream) is wrong here
//!
//! A cryptographic hash over the encoded file breaks on every
//! transcode, and transcoding is the normal life of a video — a
//! re-encode at a different dead zone, a platform's own recompression,
//! a resolution ladder. A bitstream hash therefore reports "tampered"
//! for the overwhelmingly common innocent case and gets ignored, which
//! is the failure mode of every brittle integrity signal.
//!
//! ## What FAO can do that a pixel codec cannot
//!
//! The representation is already an object graph: one plate, a set of
//! sprites, one track per object. So the digest can be **structural**,
//! and verification can then LOCALISE a difference instead of merely
//! detecting one — "the plate matches, sprites all match, object 7's
//! track diverges from frame 12" is a materially different statement
//! from "hash mismatch", and it is exactly the fingerprint of the edit
//! path built in `v11_plate::retarget_frames`. That is the real
//! argument for doing this inside FAO rather than over its output: the
//! quantities being hashed are the ones the encoder already computed.
//!
//! ## Robust component, and an honest note on keying
//!
//! Alongside the exact digests sits a robust one built from
//! `region_fingerprint` — the same 16-bit perceptual key the dictionary
//! uses — which survives re-encoding because it depends on coarse block
//! structure rather than exact bytes. It is deliberately NOT presented
//! as a security primitive: a perceptual hash is forgeable by anyone
//! willing to optimise against it.
//!
//! One correction worth recording, because it is a natural idea that
//! does not work: making the sampling pattern *content-derived* adds no
//! security. An attacker holding the video can derive the same pattern.
//! Content-derived sampling is worth doing for EFFICIENCY (spend the
//! sample budget on high-information regions) but it must not be sold
//! as making the fingerprint harder to forge. Actual unforgeability
//! comes from one place: signing the digest with a key the attacker
//! does not have, and publishing that signature somewhere with a
//! trustworthy timestamp.

use crate::v11_plate::{region_fingerprint, Mover, Plate, Yuv420};

pub const REGION_STRIDE: usize = 16;

/// A per-GOP provenance record. Small and constant-size in the number
/// of objects, so publishing one per GOP is cheap even for long video.
#[derive(Clone, Debug, PartialEq)]
pub struct GopDigest {
    pub frames: usize,
    pub width: usize,
    pub height: usize,
    /// Exact digest of the coded plate (the shared background model).
    pub plate: [u8; 32],
    /// Exact digest per mover sprite, in scene order.
    pub sprites: Vec<[u8; 32]>,
    /// Exact digest per mover TRACK, kept separate from the sprite so a
    /// re-staged object (same appearance, new path) is distinguishable
    /// from a replaced one.
    pub tracks: Vec<[u8; 32]>,
    /// Transcode-robust DESCRIPTOR, 64 bits per frame, compared by
    /// Hamming distance rather than equality. Not a security primitive
    /// (see module docs) and deliberately not hashed into `root`:
    /// folding it in would make the root brittle to the exact
    /// re-encoding it exists to tolerate.
    pub robust: Vec<u64>,
    /// Root over all of the above — the single value to publish.
    pub root: [u8; 32],
}

fn h(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for p in parts {
        // Length-prefix every part. Without this, concatenation is
        // ambiguous and two different structures can hash identically —
        // the classic way a "hash all the fields" scheme quietly admits
        // collisions.
        hasher.update(&(p.len() as u64).to_le_bytes());
        hasher.update(p);
    }
    *hasher.finalize().as_bytes()
}

fn plate_digest(p: &Plate) -> [u8; 32] {
    h(&[
        &(p.w as u64).to_le_bytes(),
        &(p.h as u64).to_le_bytes(),
        &p.y,
        &p.u,
        &p.v,
    ])
}

fn sprite_digest(m: &Mover) -> [u8; 32] {
    let mask: Vec<u8> = m.mask.iter().map(|&b| b as u8).collect();
    h(&[
        &(m.bw as u64).to_le_bytes(),
        &(m.bh as u64).to_le_bytes(),
        &mask,
        &m.sprite.y,
        &m.sprite.u,
        &m.sprite.v,
    ])
}

fn track_digest(m: &Mover) -> [u8; 32] {
    let mut buf = Vec::with_capacity(m.track.len() * 5);
    for t in &m.track {
        match t {
            None => buf.push(0u8),
            Some((x, y)) => {
                buf.push(1u8);
                buf.extend_from_slice(&x.to_le_bytes());
                buf.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
    h(&[
        &(m.bx as u64).to_le_bytes(),
        &(m.by as u64).to_le_bytes(),
        &buf,
    ])
}

/// Transcode-robust DESCRIPTOR — deliberately not a hash.
///
/// The first version hashed `region_fingerprint` over every block and
/// called the result a robust digest. It is not robust, and the test
/// that was supposed to prove it caught it: a mild requantisation
/// (`v/4*4+1`, the shape of a lossy pass) changed the digest
/// completely. The reason is structural rather than a tuning problem —
/// **hashing a perceptual feature destroys exactly the property the
/// feature had**. A cryptographic hash is designed so one flipped
/// input bit changes every output bit, and a perceptual descriptor's
/// whole value is that a few bits flip under innocent processing. The
/// two requirements are opposites, and "robust hash" is a contradiction
/// unless the comparison is a DISTANCE over the descriptor itself.
///
/// So the descriptor is published as-is and compared by Hamming
/// distance: 64 bits per frame, each bit one cell of an 8x8 grid
/// compared against the frame's own mean. 8 bytes/frame is 384 bytes
/// for a 48-frame GOP — small enough to publish alongside the exact
/// digests, and coarse enough that requantisation cannot move a cell
/// mean across the frame mean unless the picture actually changed.
pub fn robust_descriptor(frames: &[Yuv420]) -> Vec<u64> {
    let mut out = Vec::with_capacity(frames.len());
    for f in frames {
        let (w, hgt) = (f.w, f.h);
        let mut cells = [0u64; 64];
        for cy in 0..8 {
            for cx in 0..8 {
                let (x0, x1) = (cx * w / 8, (cx + 1) * w / 8);
                let (y0, y1) = (cy * hgt / 8, (cy + 1) * hgt / 8);
                let mut sum = 0u64;
                let mut n = 0u64;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += f.y[y * w + x] as u64;
                        n += 1;
                    }
                }
                cells[cy * 8 + cx] = sum / n.max(1);
            }
        }
        let mean: u64 = cells.iter().sum::<u64>() / 64;
        let mut bits = 0u64;
        for (i, c) in cells.iter().enumerate() {
            if *c > mean {
                bits |= 1 << i;
            }
        }
        out.push(bits);
    }
    out
}

/// Mean per-frame Hamming distance between two descriptors, in bits out
/// of 64. Near 0 = same content through a different encoder; large =
/// the picture changed. Returns None on a length mismatch, which is
/// itself a finding (frames added or dropped).
pub fn descriptor_distance(a: &[u64], b: &[u64]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let total: u32 = a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum();
    Some(total as f32 / a.len() as f32)
}

pub fn digest_gop(
    plate: &Plate,
    movers: &[Mover],
    frames: &[Yuv420],
) -> GopDigest {
    let p = plate_digest(plate);
    let sprites: Vec<[u8; 32]> = movers.iter().map(sprite_digest).collect();
    let tracks: Vec<[u8; 32]> = movers.iter().map(track_digest).collect();
    let robust = robust_descriptor(frames);
    let flat_s: Vec<u8> = sprites.iter().flat_map(|d| d.iter().copied()).collect();
    let flat_t: Vec<u8> = tracks.iter().flat_map(|d| d.iter().copied()).collect();
    let root = h(&[
        b"fao-provenance-v1",
        &(frames.len() as u64).to_le_bytes(),
        &(frames[0].w as u64).to_le_bytes(),
        &(frames[0].h as u64).to_le_bytes(),
        &p,
        &flat_s,
        &flat_t,
    ]);
    GopDigest {
        frames: frames.len(),
        width: frames[0].w,
        height: frames[0].h,
        plate: p,
        sprites,
        tracks,
        robust,
        root,
    }
}

/// What a verification found. The point of the whole module is that
/// this is richer than a boolean.
#[derive(Debug, PartialEq)]
pub enum Finding {
    /// Root matches — same objects, same staging, same background.
    Identical,
    /// Object count changed: something was added or removed.
    ObjectCountChanged { was: usize, now: usize },
    /// Background model differs.
    PlateChanged,
    /// Appearance of these objects differs (replaced or re-rendered).
    SpritesChanged(Vec<usize>),
    /// Appearance identical, PATH differs — the signature of a
    /// re-staging edit: the same object moved somewhere else.
    Restaged(Vec<usize>),
    /// Structure matches and the robust descriptor is close — the
    /// expected result of an innocent transcode, and the case a
    /// bitstream hash would wrongly flag as tampering.
    LikelyTranscode { descriptor_bits: f32 },
    /// Structure matches but the picture does not — content was
    /// replaced without going through the object layer at all.
    PixelsChangedOutsideObjects { descriptor_bits: f32 },
    /// Frame count differs.
    LengthChanged { was: usize, now: usize },
}

pub fn verify(reference: &GopDigest, candidate: &GopDigest) -> Vec<Finding> {
    let mut out = Vec::new();
    if reference.root == candidate.root {
        return vec![Finding::Identical];
    }
    if reference.sprites.len() != candidate.sprites.len() {
        out.push(Finding::ObjectCountChanged {
            was: reference.sprites.len(),
            now: candidate.sprites.len(),
        });
        return out;
    }
    if reference.plate != candidate.plate {
        out.push(Finding::PlateChanged);
    }
    let mut sprites_changed = Vec::new();
    let mut restaged = Vec::new();
    for i in 0..reference.sprites.len() {
        let same_sprite = reference.sprites[i] == candidate.sprites[i];
        let same_track = reference.tracks[i] == candidate.tracks[i];
        if !same_sprite {
            sprites_changed.push(i);
        } else if !same_track {
            restaged.push(i);
        }
    }
    if !sprites_changed.is_empty() {
        out.push(Finding::SpritesChanged(sprites_changed));
    }
    if !restaged.is_empty() {
        out.push(Finding::Restaged(restaged));
    }
    if out.is_empty() {
        // Every structural component matched. The root cannot differ in
        // that case (the robust descriptor is not folded in), so this
        // branch is reached only via an explicit descriptor comparison.
        match descriptor_distance(&reference.robust, &candidate.robust) {
            None => out.push(Finding::LengthChanged {
                was: reference.robust.len(),
                now: candidate.robust.len(),
            }),
            // 4 of 64 bits is the working threshold, chosen as a
            // starting point rather than measured — it needs a real
            // transcode corpus before it should be trusted, and that
            // is a gap, not a setting.
            Some(d) if d <= 4.0 => out.push(Finding::LikelyTranscode { descriptor_bits: d }),
            Some(d) => out.push(Finding::PixelsChangedOutsideObjects { descriptor_bits: d }),
        }
    }
    out
}

impl GopDigest {
    /// Hex of the root — the value a publisher would post.
    pub fn root_hex(&self) -> String {
        self.root.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::residual::ResidualOpts;
    use crate::v11_plate::{build_movers, build_plate, PlateMotion};

    fn scene(shift: i32) -> (Vec<Yuv420>, Plate, Vec<Mover>) {
        let (w, hh) = (128, 96);
        let mut frames = Vec::new();
        for t in 0..10 {
            let mut f = Yuv420::new(w, hh);
            for y in 0..hh {
                for x in 0..w {
                    f.y[y * w + x] = (70 + (x * 13 + y * 29 + (x * y) % 7 * 11) % 60) as u8;
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            let sx = (8 + t * 10 + shift as usize).min(w - 33);
            for y in 32..64 {
                for x in sx..(sx + 32).min(w) {
                    f.y[y * w + x] = (150 + ((x - sx) * 37 + (y - 32) * 101) % 97) as u8;
                }
            }
            frames.push(f);
        }
        let m = vec![PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0, lin: crate::v11_plate::LIN_ID }; frames.len()];
        let plate = build_plate(&frames, &m);
        let opts = ResidualOpts { dead_zone: 8.0, ..ResidualOpts::default() };
        let (movers, _) = build_movers(&frames, &m, &plate, &opts, 12);
        (frames, plate, movers)
    }

    /// The same input must digest identically — a provenance scheme that
    /// is not deterministic is worse than none, because it trains people
    /// to ignore mismatches.
    #[test]
    fn digest_is_deterministic() {
        let (f, p, m) = scene(0);
        let a = digest_gop(&p, &m, &f);
        let b = digest_gop(&p, &m, &f);
        assert_eq!(a, b);
        assert_eq!(verify(&a, &b), vec![Finding::Identical]);
        assert_eq!(a.root_hex().len(), 64);
    }

    /// A re-staging edit — identical sprite, rewritten track, exactly
    /// what `retarget_frames` produces — must be reported as Restaged
    /// and name the object, not as an undifferentiated mismatch.
    #[test]
    fn restaging_is_detected_and_localised() {
        let (f, p, mut m) = scene(0);
        assert!(!m.is_empty(), "test scene must produce a mover");
        let reference = digest_gop(&p, &m, &f);
        // Move object 0 down 20px for the whole GOP; appearance untouched.
        for t in m[0].track.iter_mut() {
            if let Some((_, y)) = t {
                *y += 20;
            }
        }
        let edited = digest_gop(&p, &m, &f);
        assert_ne!(reference.root, edited.root, "edit must change the root");
        let findings = verify(&reference, &edited);
        assert_eq!(findings, vec![Finding::Restaged(vec![0])], "got {findings:?}");
    }

    /// Replacing an object's APPEARANCE must read as SpritesChanged, not
    /// Restaged — the distinction is the whole reason sprite and track
    /// are hashed separately.
    #[test]
    fn appearance_change_is_distinguished_from_restaging() {
        let (f, p, mut m) = scene(0);
        let reference = digest_gop(&p, &m, &f);
        for v in m[0].sprite.y.iter_mut() {
            *v = v.wrapping_add(40);
        }
        let findings = verify(&reference, &digest_gop(&p, &m, &f));
        assert_eq!(findings, vec![Finding::SpritesChanged(vec![0])], "got {findings:?}");
    }

    /// The robust digest must survive a mild re-quantisation that a
    /// transcode would produce, while the exact plate digest does not —
    /// this is the property that stops the scheme crying wolf on every
    /// innocent re-encode.
    #[test]
    fn robust_descriptor_survives_requantisation() {
        let (f, _p, _m) = scene(0);
        let before = robust_descriptor(&f);
        let mut requantised = f.clone();
        for fr in requantised.iter_mut() {
            for v in fr.y.iter_mut() {
                // Coarse-ish requantisation, the shape of a lossy pass.
                *v = (*v / 4) * 4 + 1;
            }
        }
        let d = descriptor_distance(&before, &robust_descriptor(&requantised)).unwrap();
        assert!(d <= 1.0, "requantisation moved {d} bits of 64; must be ~0");
        // And it must still MOVE when content actually moves, or it is
        // measuring nothing. (A single small object on a large frame is
        // a deliberately hard case for a 8x8 descriptor.)
        let (moved, _, _) = scene(24);
        let dm = descriptor_distance(&before, &robust_descriptor(&moved)).unwrap();
        assert!(dm > d, "moved content ({dm} bits) must exceed transcode ({d} bits)");
    }
}
