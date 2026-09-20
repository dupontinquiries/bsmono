//! qr_decode — camera pixels to a payload string.
//!
//! The missing half of the scanner. `qr.rs` has the dialog, the four accepted
//! payload forms, the latch and the overflow rule; `lib.rs::qr_frame` takes a
//! *"decoded frame from the camera"*; `camera.rs` throttles to ~8fps
//! specifically so a scanner can keep up. Nothing turned pixels into that
//! string, which is exactly what `TODO_end_to_end.md` asks:
//!
//! > QR SCANNER specifically: does the camera feed reach the decoder, and does
//! > a decode actually fire?
//!
//! It could not, because there was no decoder.
//!
//! ## Luminance in, not RGBA
//!
//! A QR code is black and white. Taking an 8-bit luma plane rather than RGBA
//! means the caller hands over the plane the camera already produces — every
//! sensor format here is YUV of some kind — with no conversion, and a quarter
//! of the bytes. A caller with RGBA converts once at the boundary rather than
//! this module pretending colour matters.
//!
//! ## What it deliberately does not do
//!
//! No image crate, no `std::fs`, no camera. `decode` is a pure function over a
//! byte slice, which is what lets the scanner be tested without a camera —
//! the boundary the TODO says nothing had crossed.

/// A decoded code and where it sat, in luma-plane pixels.
///
/// The bounds are carried because a viewfinder wants to draw the box it
/// latched onto. Returning only the string would make the UI guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub payload: String,
    /// Corners in plane coordinates, clockwise from the top-left finder.
    pub corners: [(i32, i32); 4],
}

/// Why a frame produced nothing.
///
/// Distinguished because they mean different things to a user: an empty frame
/// is "keep pointing", a too-small one is "move closer", and a damaged code is
/// "hold still". A single `None` makes all three look like a dead camera —
/// which the TODO names as the failure mode it could not tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCode {
    /// No finder patterns at all. The ordinary case, thirty times a second.
    NothingSeen,
    /// Found a candidate but could not read it: blur, glare, a partial code at
    /// the frame edge.
    Unreadable,
    /// The frame itself is unusable — zero-sized, or the slice does not match
    /// the stated dimensions.
    BadFrame,
}

/// Smallest frame worth attempting.
///
/// Below this the finder patterns are a few pixels across and every attempt
/// fails, so the work is wasted on every frame rather than occasionally.
pub const MIN_SIDE: usize = 64;

/// Decode the first QR code in an 8-bit luminance plane.
///
/// `luma` is `w * h` bytes, row-major, 0 = black. Returns the first code
/// found: a scanner pointed at two codes should latch one and not flicker
/// between them, and `qr.rs`'s latch then holds it.
pub fn decode(luma: &[u8], w: usize, h: usize) -> Result<Decoded, NoCode> {
    // Checked rather than trusted: a caller who passes a stride-padded plane
    // as if it were packed would otherwise read past the end or, worse, decode
    // a sheared image and produce a plausible wrong id.
    if w < MIN_SIDE || h < MIN_SIDE || luma.len() < w * h {
        return Err(NoCode::BadFrame);
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma[y * w + x]);
    let grids = img.detect_grids();
    let Some(grid) = grids.into_iter().next() else {
        return Err(NoCode::NothingSeen);
    };
    let bounds = grid.bounds;
    match grid.decode() {
        Ok((_meta, payload)) => Ok(Decoded {
            payload,
            corners: [
                (bounds[0].x, bounds[0].y),
                (bounds[1].x, bounds[1].y),
                (bounds[2].x, bounds[2].y),
                (bounds[3].x, bounds[3].y),
            ],
        }),
        // A grid was found and would not read. That is a real distinction from
        // NothingSeen: the user is pointing at something.
        Err(_) => Err(NoCode::Unreadable),
    }
}

impl NoCode {
    /// What to tell the user, or `None` for the ordinary empty frame.
    ///
    /// `NothingSeen` says nothing on purpose: a scanner that reports "no code
    /// found" eight times a second is noise, and the viewfinder already
    /// communicates that state by not having latched.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            NoCode::NothingSeen => None,
            NoCode::Unreadable => Some("Hold still — the code is there but blurred"),
            NoCode::BadFrame => Some("Camera frame is too small to scan"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// Render a real QR code to a luma plane via the kit's `qrbig`, which is
    /// Nayuki's `qrcodegen` — a genuine encoder with Reed-Solomon, masking and
    /// format info.
    ///
    /// NOT `uikit_egui::qr`, which its own header calls a "QR-shaped
    /// identicon": "there is no Reed-Solomon encoder, no mask evaluation and
    /// no format information. A real QR reader will not decode it." A round
    /// trip through THAT would assert that a real decoder rejects a decorative
    /// image — correct behaviour dressed as a passing test.
    fn render(payload: &str, scale: usize, quiet: usize) -> (Vec<u8>, usize, usize) {
        let m = uikit_egui::qrbig::encode(payload.as_bytes()).expect("qrcodegen encodes this");
        let side = (m.size + quiet * 2) * scale;
        let mut luma = vec![255u8; side * side];
        for y in 0..m.size {
            for x in 0..m.size {
                if !m.get(x, y) {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x + quiet) * scale + dx;
                        let py = (y + quiet) * scale + dy;
                        luma[py * side + px] = 0;
                    }
                }
            }
        }
        (luma, side, side)
    }

    /// THE ROUND TRIP, now that snapclone encodes a real code. This is the
    /// question `TODO_end_to_end.md` asks — "does a decode actually fire" —
    /// and it could not be answered while the app's own code was undecodable.
    #[test]
    fn a_real_encoded_code_decodes_back_to_its_payload() {
        let (luma, w, h) = render("sc-aabbccdd", 6, 4);
        let got = decode(&luma, w, h).expect("a clean render of a real code must decode");
        assert_eq!(got.payload, "sc-aabbccdd");
    }

    /// Every form the dialog accepts must survive the camera, or the scanner
    /// works for some codes and not others.
    #[test]
    fn every_accepted_payload_form_survives_a_round_trip() {
        for p in ["sc-aabbccdd", "sc-aabbccdd:9876",
                  "snapclone://sc-aabbccdd", "snapclone://sc-aabbccdd:9876"] {
            let (luma, w, h) = render(p, 6, 4);
            let got = decode(&luma, w, h).unwrap_or_else(|e| panic!("{p} failed: {e:?}"));
            assert_eq!(got.payload, p);
            assert!(crate::qr::parse_payload(&got.payload).is_some(),
                    "{p} decoded but the dialog rejects it");
        }
    }

    /// TWO SNAPCLONES CAN NOW SCAN EACH OTHER, which is what "add a friend by
    /// QR" needs and what the identicon made impossible. The id the app shows
    /// is the id another instance reads.
    #[test]
    fn the_code_this_app_shows_is_one_this_app_can_read() {
        let id = "sc-0123abcd";
        let (luma, w, h) = render(id, 5, 4);
        let got = decode(&luma, w, h).expect("our own code must be scannable");
        let parsed = crate::qr::parse_payload(&got.payload).expect("and parseable");
        assert_eq!(got.payload, id);
        let _ = parsed;
    }

    /// The bounds are carried so a viewfinder can draw what it latched, and
    /// must land inside the frame rather than being a placeholder.
    #[test]
    fn the_decoded_corners_are_inside_the_frame() {
        let (luma, w, h) = render("sc-aabbccdd", 6, 4);
        let got = decode(&luma, w, h).unwrap();
        for (x, y) in got.corners {
            assert!(x >= 0 && (x as usize) < w, "x {x} outside 0..{w}");
            assert!(y >= 0 && (y as usize) < h, "y {y} outside 0..{h}");
        }
    }

    /// AN EMPTY FRAME IS NOT AN ERROR, and says nothing to the user — a
    /// scanner reporting "no code" eight times a second is noise.
    #[test]
    fn a_blank_frame_reports_nothing_seen_and_stays_quiet() {
        let blank = vec![255u8; 128 * 128];
        assert_eq!(decode(&blank, 128, 128), Err(NoCode::NothingSeen));
        assert_eq!(NoCode::NothingSeen.hint(), None);
    }

    /// A BAD FRAME IS ITS OWN CASE. A caller passing a stride-padded plane as
    /// packed would otherwise read past the end, or decode a sheared image and
    /// return a plausible WRONG id — the worst outcome available here.
    #[test]
    fn a_frame_that_does_not_match_its_dimensions_is_refused() {
        let short = vec![255u8; 100];
        assert_eq!(decode(&short, 128, 128), Err(NoCode::BadFrame));
        assert_eq!(decode(&[], 0, 0), Err(NoCode::BadFrame));
        // Too small to hold finder patterns at all.
        let tiny = vec![255u8; 32 * 32];
        assert_eq!(decode(&tiny, 32, 32), Err(NoCode::BadFrame));
        assert!(NoCode::BadFrame.hint().is_some(), "this one is worth saying");
    }

    /// Each failure tells the user something different, which is the reason
    /// for the enum rather than an Option.
    #[test]
    fn the_three_failures_do_not_read_alike() {
        let hints = [NoCode::NothingSeen.hint(), NoCode::Unreadable.hint(),
                     NoCode::BadFrame.hint()];
        assert_eq!(hints[0], None, "the ordinary case is silent");
        assert_ne!(hints[1], hints[2], "blur and a bad frame are different advice");
    }

}
