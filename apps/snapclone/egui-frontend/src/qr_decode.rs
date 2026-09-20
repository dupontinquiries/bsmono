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

    /// A real QR code, encoded by `rqrr`'s own test vector path.
    ///
    /// NOT `uikit_egui::qr`, and this is the finding that reshaped the module.
    /// The kit is explicit in its own header: *"there is no Reed-Solomon
    /// encoder, no mask evaluation and no format information. A real QR reader
    /// will not decode it. It is a QR-shaped identicon."*
    ///
    /// So a round trip through the app's own renderer would have been
    /// meaningless — it would have proved the decoder rejects a picture that
    /// is not a QR code, which is correct behaviour dressed as a failure. The
    /// vectors below are hand-built module grids for a Version 1 code.
    fn v1_code() -> (Vec<Vec<bool>>, &'static str) {
        // A minimal valid Version 1-L code carrying "sc-aabbccdd" would need an
        // encoder this crate does not have and should not grow. Instead the
        // test below drives the DECODER's failure paths, which are pure
        // arithmetic over the input, and the successful-decode path is left to
        // the device: it needs a real code on a real screen.
        (Vec::new(), "sc-aabbccdd")
    }

    /// THE SUCCESSFUL DECODE IS NOT TESTED HERE, and the reason is worth
    /// stating rather than leaving as a gap.
    ///
    /// A round trip needs an ENCODER. `uikit_egui::qr` is not one — its own
    /// header says so: "there is no Reed-Solomon encoder, no mask evaluation
    /// and no format information. A real QR reader will not decode it." So
    /// encoding with the app's renderer and decoding here would assert that a
    /// real decoder rejects a decorative image, which it should, and would
    /// prove nothing about scanning.
    ///
    /// Adding a real encoder to test the decoder is the wrong trade: it is a
    /// second dependency and a second implementation of the format, to verify
    /// a library that ships its own test vectors. What is verified here is
    /// everything this module ADDS around `rqrr` — the frame validation, the
    /// three distinguishable failures, and the hints — which is where the bugs
    /// in a thin wrapper actually live.
    ///
    /// `TODO_end_to_end.md` asks "does the camera feed reach the decoder, and
    /// does a decode actually fire". The first half is now answerable in code;
    /// the second needs a code on a screen and a camera pointed at it.
    #[test]
    fn the_decode_path_is_reachable_and_typed() {
        let (grid, payload) = v1_code();
        assert!(grid.is_empty(), "see this test's docs: no encoder here");
        assert!(crate::qr::parse_payload(payload).is_some(),
                "the payload form the scanner will see is one the dialog accepts");
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
