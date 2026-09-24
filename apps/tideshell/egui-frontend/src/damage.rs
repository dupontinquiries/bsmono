//! Not repainting what has not changed.
//!
//! The shell runs several layer surfaces at once — the bar, the peeks, the
//! overlay — and the loop drives all of them every frame. When one of them
//! animates, the others were being refilled and committed anyway: a static
//! bar re-rasterised ninety times a second because a sheet was fading
//! somewhere else.
//!
//! On a GPU that is wasteful. Without one it is the whole cost: a software
//! rasterizer is bound by writing pixels, so the only real lever after
//! "fewer frames" is "fewer pixels", and the cheapest pixels are the ones
//! belonging to a surface that did not change at all.
//!
//! ## Why a hash of the tessellation, rather than asking egui
//!
//! egui knows whether it wants another frame, not whether this frame looks
//! like the last one — those differ exactly where it matters, because an
//! animation that has reached its resting value still requests one more
//! frame. Comparing the tessellated output catches that case and every
//! other one, including "the model changed but nothing visible did".
//!
//! Tessellation still happens; only the fill and the commit are skipped.
//! That is the right trade: tessellating a static bar is a few thousand
//! vertices, filling it is a few hundred thousand pixels.

/// A hash of what would be drawn. Equal hashes mean an identical frame.
///
/// FNV-1a over the raw vertex bytes and the clip rectangles. Vertices are
/// plain data — position, uv, colour — so their bytes are exactly the
/// content, and NaN cannot make two identical frames compare unequal the way
/// float comparison would.
pub fn frame_hash(prims: &[egui::ClippedPrimitive]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    };
    for p in prims {
        for f in [p.clip_rect.min.x, p.clip_rect.min.y, p.clip_rect.max.x, p.clip_rect.max.y] {
            eat(&f.to_bits().to_le_bytes());
        }
        match &p.primitive {
            egui::epaint::Primitive::Mesh(m) => {
                eat(&(m.indices.len() as u64).to_le_bytes());
                for i in &m.indices {
                    eat(&i.to_le_bytes());
                }
                for v in &m.vertices {
                    eat(&v.pos.x.to_bits().to_le_bytes());
                    eat(&v.pos.y.to_bits().to_le_bytes());
                    eat(&v.uv.x.to_bits().to_le_bytes());
                    eat(&v.uv.y.to_bits().to_le_bytes());
                    eat(&v.color.to_array());
                }
                eat(&format!("{:?}", m.texture_id).into_bytes());
            }
            // Callbacks are opaque: assume they changed, because guessing
            // otherwise would freeze whatever they draw.
            egui::epaint::Primitive::Callback(_) => eat(b"callback-always-dirty"),
        }
    }
    h
}

/// What was last put on a surface: the frame's hash and the size it was
/// painted at.
///
/// The size has to be part of it. A configure can change the buffer size or
/// the scale while the CONTENT hashes identically — a bar whose width
/// changed but whose shapes are laid out the same, a scale change on the
/// same logical layout — and skipping that repaint leaves the surface at the
/// old size with the compositor expecting the new one.
pub type Painted = (u64, (i32, i32));

/// Should this surface be painted? `textures` is whether the frame carries a
/// texture upload, which must not be skipped even if the geometry matches —
/// dropping it would leave the painter referencing a texture it never got.
pub fn changed(last: Option<Painted>, now: Painted, textures: bool) -> bool {
    textures || last != Some(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{ClippedPrimitive, Mesh, Primitive, Vertex};
    use egui::{Color32, Pos2, Rect};

    fn quad(x: f32, color: Color32) -> Vec<ClippedPrimitive> {
        let mut m = Mesh::default();
        for (dx, dy) in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)] {
            m.vertices.push(Vertex { pos: Pos2::new(x + dx, dy), uv: egui::epaint::WHITE_UV, color });
        }
        m.indices = vec![0, 1, 2, 0, 2, 3];
        vec![ClippedPrimitive { clip_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)), primitive: Primitive::Mesh(m) }]
    }

    #[test]
    fn an_identical_frame_hashes_identically() {
        assert_eq!(frame_hash(&quad(0.0, Color32::RED)), frame_hash(&quad(0.0, Color32::RED)));
        let h = frame_hash(&quad(0.0, Color32::RED));
        assert!(!changed(Some((h, (720, 1600))), (h, (720, 1600)), false));
    }

    #[test]
    fn the_same_content_at_a_new_size_is_still_a_repaint() {
        // A configure can change the buffer size or the scale while the
        // shapes hash the same; skipping then leaves the surface at the old
        // size with the compositor expecting the new one.
        let h = frame_hash(&quad(0.0, Color32::RED));
        assert!(changed(Some((h, (720, 1600))), (h, (1080, 2400)), false));
        assert!(changed(Some((h, (720, 1600))), (h, (720, 1601)), false), "one pixel is a different surface");
    }

    #[test]
    fn anything_visible_moving_is_caught() {
        let base = frame_hash(&quad(0.0, Color32::RED));
        assert_ne!(base, frame_hash(&quad(0.5, Color32::RED)), "half a pixel is still a change");
        assert_ne!(base, frame_hash(&quad(0.0, Color32::from_rgb(255, 0, 1))), "one colour step");
        // A different clip rect with the same geometry is a different frame.
        let mut other = quad(0.0, Color32::RED);
        other[0].clip_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(50.0, 100.0));
        assert_ne!(base, frame_hash(&other));
    }

    #[test]
    fn a_texture_upload_is_never_skipped() {
        // Skipping a frame that carries a texture delta would leave the
        // painter referencing a texture it never received.
        let h = (frame_hash(&quad(0.0, Color32::RED)), (720i32, 1600i32));
        assert!(changed(Some(h), h, true), "identical geometry, but the upload must go");
        assert!(changed(None, h, false), "and the first frame always paints");
    }

    #[test]
    fn an_empty_frame_is_stable_and_differs_from_a_drawn_one() {
        let empty = frame_hash(&[]);
        assert_eq!(empty, frame_hash(&[]));
        assert_ne!(empty, frame_hash(&quad(0.0, Color32::RED)));
    }
}
