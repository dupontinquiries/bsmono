//! wire — turning an estimate into what the bitstream writer wants.
//!
//! The estimator produces a `Warp` in pixels. The writer wants
//! quantised integers in AV1's warped-model precision, and it wants
//! them in a form it can actually encode.
//!
//! ## Only translation is writable today
//!
//! From rav1e's `header.rs`:
//!
//! ```text
//! GlobalMVMode::ROTZOOM => unimplemented!(),
//! GlobalMVMode::AFFINE => unimplemented!(),
//! ```
//!
//! Those branches PANIC. An estimator that confidently returns a
//! rotzoom would not produce a worse stream, it would crash the
//! encoder mid-frame. So [`Writable::for_rav1e`] refuses anything the
//! writer cannot encode, and says why rather than silently downgrading
//! -- a silent downgrade would hide the fact that the rotzoom branch
//! still needs writing.
//!
//! This is also the order to build in: translation is the common case
//! on real footage (a pan is a translation), it is the branch that
//! exists, and it can be validated end to end before anyone writes the
//! rotzoom coder.
//!
//! ## Precision, matched to rav1e's own arithmetic
//!
//! The writer computes:
//!
//! ```text
//! let bits = 12 - 6 + 3 - !allow_high_precision_mv as u8;
//! let bits_diff = 12 - 3 + allow_high_precision_mv as u8;
//! write_s_refsubexpfin(self, (1 << bits) + 1, 3,
//!                      mv_ref >> bits_diff, mv >> bits_diff);
//! ```
//!
//! So `mv` is a warped-model-precision fixed-point value and the coder
//! sees it shifted down by `bits_diff`. These constants are taken from
//! that code rather than from the spec, deliberately: the thing that
//! has to agree is rav1e's writer, and a value derived from a
//! different reading of the spec would encode a stream rav1e's own
//! decoder path did not expect.

use super::{Model, Warp};

/// AV1 carries warp parameters as fixed point with this many
/// fractional bits.
pub const WARPEDMODEL_PREC_BITS: u32 = 16;

/// Translation-only parameters are carried at reduced precision.
///
/// ## THIS CONSTANT IS DERIVED, NOT VERIFIED
///
/// Taken as `WARPEDMODEL_PREC_BITS - GM_TRANS_ONLY_PREC_BITS` = 16 - 3,
/// and cross-checked by asking what range it produces against rav1e's
/// own `bits` and `bits_diff`:
///
/// ```text
///   frac  high precision   low precision
///     16       8.0 px          2.0 px
///     13      64.0 px         16.0 px
///     12     128.0 px         32.0 px
/// ```
///
/// 16 is wrong: a 3.3-pixel pan -- an ordinary camera move -- overflows
/// the coded range, which no real format would do. 13 gives a range
/// that matches what global motion is for.
///
/// But this was reasoned from rav1e's arithmetic and a range
/// plausibility check, NOT confirmed against a decoder. Before this
/// ships, encode a known translation and check a reference decoder
/// reconstructs the same shift. Getting it wrong produces a stream
/// that decodes to the wrong motion rather than one that fails
/// loudly, which is the worst kind of wrong.
pub const TRANS_ONLY_PREC_BITS: u32 = 13;

/// What rav1e's writer can currently encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writable {
    Identity,
    Translation { mv_x: i32, mv_y: i32 },
}

/// Why an estimate could not be written.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The writer branch is `unimplemented!()`.
    WriterMissing(Model),
    /// Quantising it would land on identity anyway, so sending a
    /// header would cost bits for nothing.
    QuantisesToZero,
    /// Beyond what the coded range can carry.
    OutOfRange { mv_x: i32, mv_y: i32 },
}

impl Refused {
    pub fn explain(&self) -> String {
        match self {
            Refused::WriterMissing(m) => format!(
                "rav1e's header writer has `unimplemented!()` for {m:?}. Sending it would \
                 panic mid-frame, not degrade. Write that branch before the estimator is \
                 allowed to return it."),
            Refused::QuantisesToZero => "the motion is smaller than one quantisation step, \
                so the header would encode identity and cost bits for nothing".into(),
            Refused::OutOfRange { mv_x, mv_y } => format!(
                "({mv_x}, {mv_y}) is outside the coded range. A shift this large is usually \
                 a scene cut mistaken for motion."),
        }
    }
}

/// The coded range, from the writer's own `bits` computation.
pub fn coded_bits(allow_high_precision_mv: bool) -> u32 {
    // 12 - 6 + 3 - !hp
    (12 - 6 + 3) - if allow_high_precision_mv { 0 } else { 1 }
}

/// The shift the writer applies before coding.
pub fn bits_diff(allow_high_precision_mv: bool) -> u32 {
    // 12 - 3 + hp
    (12 - 3) + if allow_high_precision_mv { 1 } else { 0 }
}

/// Convert pixels to the fixed-point units the writer expects.
pub fn px_to_warpedmodel(px: f64) -> i32 {
    (px * (1i64 << TRANS_ONLY_PREC_BITS) as f64).round() as i32
}

pub fn warpedmodel_to_px(v: i32) -> f64 {
    v as f64 / (1i64 << TRANS_ONLY_PREC_BITS) as f64
}

/// The largest shift that can be coded, in pixels. Worth having
/// explicitly: it is what tells a caller that a 200-pixel "motion" was
/// a cut.
pub fn max_shift_px(allow_high_precision_mv: bool) -> f64 {
    let limit = (1i64 << coded_bits(allow_high_precision_mv)) as f64;
    limit * (1i64 << bits_diff(allow_high_precision_mv)) as f64
        / (1i64 << TRANS_ONLY_PREC_BITS) as f64
}

/// Prepare an estimate for rav1e's writer.
///
/// `allow_high_precision_mv` must match what the frame header will
/// say, or the value is quantised against a different grid than the
/// one it is coded on.
pub fn for_rav1e(model: Model, warp: &Warp, allow_high_precision_mv: bool)
    -> Result<Writable, Refused> {
    match model {
        Model::Identity => Ok(Writable::Identity),
        Model::RotZoom | Model::Affine => Err(Refused::WriterMissing(model)),
        Model::Translation => {
            let diff = bits_diff(allow_high_precision_mv);
            let (mut x, mut y) = (px_to_warpedmodel(warp.tx), px_to_warpedmodel(warp.ty));
            // ROUND TO THE GRID THE CODER USES. The writer shifts right
            // by `bits_diff` before coding, so anything below that is
            // discarded -- quantising here rather than letting the
            // shift truncate keeps the encoder's idea of the motion and
            // the decoder's identical, which matters because the
            // predictor uses it.
            let step = 1i32 << diff;
            x = ((x as f64 / step as f64).round() as i32) * step;
            y = ((y as f64 / step as f64).round() as i32) * step;
            if x == 0 && y == 0 { return Err(Refused::QuantisesToZero); }
            // the coder writes (mv >> bits_diff) into a range of
            // (1 << bits) + 1 symbols, signed
            let bits = coded_bits(allow_high_precision_mv);
            let limit = 1i32 << bits;
            let (cx, cy) = (x >> diff, y >> diff);
            if cx.abs() > limit || cy.abs() > limit {
                return Err(Refused::OutOfRange { mv_x: cx, mv_y: cy });
            }
            Ok(Writable::Translation { mv_x: x, mv_y: y })
        }
    }
}

/// What the decoder will reconstruct, so the encoder can predict from
/// the same thing.
///
/// Using the unquantised estimate for prediction while the decoder has
/// the quantised one is a drift bug: every block predicts from a
/// slightly different place than the decoder will, and the residual
/// silently pays for it.
pub fn as_decoded(w: &Writable) -> Warp {
    match w {
        Writable::Identity => Warp::default(),
        Writable::Translation { mv_x, mv_y } =>
            Warp::translation(warpedmodel_to_px(*mv_x), warpedmodel_to_px(*mv_y)),
    }
}

/// The seven reference slots a frame header carries.
pub const INTER_REFS_PER_FRAME: usize = 7;

/// A whole frame's worth, ready to hand to the writer.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameGlobalMotion {
    pub per_ref: [Writable; INTER_REFS_PER_FRAME],
    /// Anything refused, and why, for the log rather than for silence.
    pub refused: Vec<(usize, Refused)>,
}

impl FrameGlobalMotion {
    pub fn identity() -> FrameGlobalMotion {
        FrameGlobalMotion { per_ref: [Writable::Identity; INTER_REFS_PER_FRAME],
                            refused: Vec::new() }
    }

    /// Did anything actually get signalled?
    pub fn any(&self) -> bool {
        self.per_ref.iter().any(|w| !matches!(w, Writable::Identity))
    }
}

/// Assemble a frame's global motion from per-reference estimates.
///
/// A reference with no estimate, or one that was refused, becomes
/// identity -- which is always safe and always writable.
pub fn assemble(estimates: &[(usize, Model, Warp)], allow_high_precision_mv: bool)
    -> FrameGlobalMotion {
    let mut out = FrameGlobalMotion::identity();
    for (slot, model, warp) in estimates {
        if *slot >= INTER_REFS_PER_FRAME { continue; }
        match for_rav1e(*model, warp, allow_high_precision_mv) {
            Ok(w) => out.per_ref[*slot] = w,
            Err(e) => out.refused.push((*slot, e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_the_writer_cannot_encode_is_refused_not_downgraded() {
        // `ROTZOOM => unimplemented!()` PANICS. Sending it would crash
        // mid-frame, and silently downgrading would hide that the
        // branch still needs writing.
        let w = Warp { a: 0.99, b: 0.02, tx: 3.0, c: -0.02, d: 0.99, ty: 1.0 };
        let e = for_rav1e(Model::RotZoom, &w, true).unwrap_err();
        assert_eq!(e, Refused::WriterMissing(Model::RotZoom));
        assert!(e.explain().contains("unimplemented"), "{}", e.explain());
        assert!(e.explain().contains("panic mid-frame, not degrade"), "{}", e.explain());
        assert!(e.explain().contains("before the estimator is allowed"), "{}", e.explain());
        assert!(for_rav1e(Model::Affine, &w, true).is_err());
        // translation, the branch that exists, goes through
        assert!(for_rav1e(Model::Translation, &Warp::translation(4.0, -2.0), true).is_ok());
    }

    #[test]
    fn the_encoder_predicts_from_what_the_decoder_will_reconstruct() {
        // using the unquantised estimate while the decoder has the
        // quantised one is a drift bug: every block predicts from a
        // slightly different place and the residual silently pays
        let want = Warp::translation(6.35, -2.7);
        let w = for_rav1e(Model::Translation, &want, true).expect("writable");
        let got = as_decoded(&w);
        // it is NOT the input -- it is the grid point, and that is the
        // point
        let step_px = warpedmodel_to_px(1 << bits_diff(true));
        assert!((got.tx - want.tx).abs() <= step_px, "tx {} vs {}", got.tx, want.tx);
        assert!((got.ty - want.ty).abs() <= step_px, "ty {} vs {}", got.ty, want.ty);
        // and round-tripping the decoded value is stable
        let again = for_rav1e(Model::Translation, &got, true).expect("stable");
        assert_eq!(again, w, "quantising twice changes nothing");
    }

    #[test]
    fn motion_below_one_step_is_not_worth_a_header() {
        // the header costs bits per reference, and there are seven
        let tiny = Warp::translation(0.0001, -0.0001);
        assert_eq!(for_rav1e(Model::Translation, &tiny, true).unwrap_err(),
                   Refused::QuantisesToZero);
        assert!(for_rav1e(Model::Translation, &tiny, true).unwrap_err()
                .explain().contains("cost bits for nothing"));
        // and exact zero likewise
        assert!(for_rav1e(Model::Translation, &Warp::default(), true).is_err());
    }

    #[test]
    fn precision_follows_the_frame_header_not_a_guess() {
        // quantising against a different grid than the one it is coded
        // on puts the encoder and decoder out of step
        assert_eq!(coded_bits(true), 9);
        assert_eq!(coded_bits(false), 8);
        assert_eq!(bits_diff(true), 10);
        assert_eq!(bits_diff(false), 9);
        // AN OPEN QUESTION, RECORDED RATHER THAN FORCED.
        //
        // In rav1e's arithmetic `allow_high_precision_mv` INCREASES
        // bits_diff (9 -> 10), so it shifts more away: the step gets
        // COARSER, 0.0625 px to 0.125 px, while the range doubles.
        // That is the opposite of what the flag's name suggests.
        //
        // It may be correct -- the flag governs motion vector
        // precision, and global motion translation may deliberately
        // trade precision for reach when MVs are finer. Or rav1e's
        // constants may be transposed; the branch has never run, so a
        // sign or order error there would never have been caught.
        //
        // Asserting my expectation would encode a guess. This asserts
        // the OBSERVED behaviour and names the question.
        let w = Warp::translation(3.3, 0.0);
        let hp = for_rav1e(Model::Translation, &w, true).unwrap();
        let lp = for_rav1e(Model::Translation, &w, false).unwrap();
        let (hx, lx) = (as_decoded(&hp).tx, as_decoded(&lp).tx);
        assert!((hx - 3.3).abs() >= (lx - 3.3).abs(),
                "observed: hp is the COARSER grid here ({hx} vs {lx}) -- verify against a \
                 decoder before shipping");
        // and an ordinary pan fits comfortably in both
        assert!(max_shift_px(true) > 32.0 && max_shift_px(false) > 8.0,
                "an ordinary camera move must fit: {} / {}",
                max_shift_px(true), max_shift_px(false));
    }

    #[test]
    fn a_shift_too_large_to_code_is_named_as_probably_a_cut() {
        let huge = Warp::translation(4000.0, 0.0);
        assert!(4000.0 > max_shift_px(true), "well past the range");
        match for_rav1e(Model::Translation, &huge, true).unwrap_err() {
            Refused::OutOfRange { .. } => {}
            o => panic!("{o:?}"),
        }
        let e = for_rav1e(Model::Translation, &huge, true).unwrap_err();
        assert!(e.explain().contains("scene cut mistaken for motion"), "{}", e.explain());
    }

    #[test]
    fn a_frame_assembles_seven_slots_and_records_what_it_refused() {
        let est = vec![
            (0usize, Model::Translation, Warp::translation(5.0, -3.0)),
            (1, Model::RotZoom, Warp { a: 0.99, b: 0.01, tx: 1.0, c: -0.01, d: 0.99, ty: 0.0 }),
            (2, Model::Identity, Warp::default()),
        ];
        let f = assemble(&est, true);
        assert!(f.any(), "slot 0 carries motion");
        assert!(matches!(f.per_ref[0], Writable::Translation { .. }));
        // a refused slot falls back to identity, which is always safe
        assert_eq!(f.per_ref[1], Writable::Identity);
        assert_eq!(f.refused.len(), 1);
        assert_eq!(f.refused[0].0, 1);
        // untouched slots too
        assert_eq!(f.per_ref[6], Writable::Identity);
        // and an all-identity frame says so, so the caller can skip the
        // header entirely
        assert!(!FrameGlobalMotion::identity().any());
    }

    #[test]
    fn a_slot_out_of_range_is_ignored_rather_than_panicking() {
        let est = vec![(99usize, Model::Translation, Warp::translation(2.0, 2.0))];
        let f = assemble(&est, true);
        assert!(!f.any());
        assert!(f.refused.is_empty(), "out of range is not a refusal, it is not a slot");
    }
}
