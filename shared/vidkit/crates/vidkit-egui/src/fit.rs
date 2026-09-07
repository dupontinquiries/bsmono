//! Fitting a frame into a surface.
//!
//! Pure geometry, separated from the widget so it can be tested without laying
//! out a frame. Aspect-ratio bugs are the ones users notice first and the
//! hardest to see in a screenshot diff, because a slightly-wrong rect still
//! looks like video.

use egui::{Rect, Vec2};

/// How a frame is fitted into its surface.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Whole frame visible, bars on the short axis. The default: a player that
    /// crops by default eventually crops something the viewer wanted.
    Contain,
    /// Fill the surface, cropping the long axis.
    Cover,
    /// Ignore the frame's aspect and stretch. Only ever what someone explicitly
    /// asked for.
    Stretch,
}

/// Pixel aspect ratio, as a numerator/denominator.
///
/// Not always 1:1. Anamorphic DVD content and some broadcast streams encode a
/// 16:9 picture in 720x576 storage; treating storage dimensions as display
/// dimensions renders everyone tall and thin. MP4 carries this in `pasp` and
/// Matroska in `DisplayWidth`/`DisplayHeight` — neither of which the demuxers
/// here read yet, so this is currently always 1:1 in practice and exists so
/// that when they do, the call sites already take it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PixelAspect {
    pub num: u32,
    pub den: u32,
}

impl PixelAspect {
    pub const SQUARE: PixelAspect = PixelAspect { num: 1, den: 1 };

    pub fn ratio(&self) -> f32 {
        if self.den == 0 {
            return 1.0;
        }
        self.num as f32 / self.den as f32
    }
}

impl Default for PixelAspect {
    fn default() -> Self {
        Self::SQUARE
    }
}

/// The rect a frame of `frame_size` should occupy inside `surface`.
///
/// Returns a rect centred in the surface. For `Cover` the result is larger than
/// the surface on one axis; the caller clips.
pub fn fit_rect(surface: Rect, frame_size: Vec2, par: PixelAspect, fit: Fit) -> Rect {
    let sw = surface.width().max(0.0);
    let sh = surface.height().max(0.0);
    // A degenerate frame or surface gets an empty rect rather than a NaN one.
    // NaN propagates into egui's layout and the failure surfaces somewhere
    // else entirely.
    if sw <= 0.0 || sh <= 0.0 || frame_size.x <= 0.0 || frame_size.y <= 0.0 {
        return Rect::from_center_size(surface.center(), Vec2::ZERO);
    }

    if fit == Fit::Stretch {
        return surface;
    }

    let display_w = frame_size.x * par.ratio();
    let display_h = frame_size.y;
    let scale_x = sw / display_w;
    let scale_y = sh / display_h;
    let scale = match fit {
        Fit::Contain => scale_x.min(scale_y),
        Fit::Cover => scale_x.max(scale_y),
        Fit::Stretch => unreachable!("handled above"),
    };

    Rect::from_center_size(
        surface.center(),
        Vec2::new(display_w * scale, display_h * scale),
    )
}

/// Where a point in the surface lands in frame coordinates, or `None` if it is
/// in the letterbox bars. Used to map a tap to a picture position.
pub fn surface_to_frame(
    surface: Rect,
    frame_size: Vec2,
    par: PixelAspect,
    fit: Fit,
    point: egui::Pos2,
) -> Option<Vec2> {
    let r = fit_rect(surface, frame_size, par, fit);
    if r.width() <= 0.0 || r.height() <= 0.0 || !r.contains(point) {
        return None;
    }
    let u = (point.x - r.min.x) / r.width();
    let v = (point.y - r.min.y) / r.height();
    Some(Vec2::new(u * frame_size.x, v * frame_size.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn surface(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), Vec2::new(w, h))
    }

    fn approx(a: f32, b: f32, what: &str) {
        assert!((a - b).abs() < 0.01, "{what}: {a} vs {b}");
    }

    #[test]
    fn contain_letterboxes_a_wide_frame_in_a_square_surface() {
        let r = fit_rect(
            surface(400.0, 400.0),
            Vec2::new(1920.0, 1080.0),
            PixelAspect::SQUARE,
            Fit::Contain,
        );
        approx(r.width(), 400.0, "width fills");
        approx(r.height(), 225.0, "height letterboxed");
        approx(r.center().x, surface(400.0, 400.0).center().x, "centred x");
        approx(r.center().y, surface(400.0, 400.0).center().y, "centred y");
    }

    #[test]
    fn contain_pillarboxes_a_tall_frame() {
        let r = fit_rect(
            surface(400.0, 400.0),
            Vec2::new(1080.0, 1920.0),
            PixelAspect::SQUARE,
            Fit::Contain,
        );
        approx(r.height(), 400.0, "height fills");
        approx(r.width(), 225.0, "width pillarboxed");
    }

    /// Cover overflows on purpose; the caller clips. A Cover that fitted inside
    /// the surface would be Contain under another name.
    #[test]
    fn cover_overflows_the_short_axis() {
        let s = surface(400.0, 400.0);
        let r = fit_rect(s, Vec2::new(1920.0, 1080.0), PixelAspect::SQUARE, Fit::Cover);
        approx(r.height(), 400.0, "height fills");
        assert!(r.width() > s.width(), "cover must overflow: {}", r.width());
    }

    #[test]
    fn stretch_takes_the_whole_surface() {
        let s = surface(400.0, 300.0);
        let r = fit_rect(s, Vec2::new(1920.0, 1080.0), PixelAspect::SQUARE, Fit::Stretch);
        assert_eq!(r, s);
    }

    /// Anamorphic content: 720x576 storage carrying a 16:9 picture. Ignoring
    /// the pixel aspect renders it tall and thin, which reads as "the video is
    /// like that" rather than as a bug.
    #[test]
    fn a_non_square_pixel_aspect_widens_the_result() {
        // A WIDE surface, deliberately. On a square one both variants clamp to
        // the surface width and come out identical at 1000, so the comparison
        // below cannot bind and the test proves nothing -- it passed while
        // measuring the surface rather than the pixel aspect. 2000x1000 makes
        // height the binding constraint for both, which is where the PAR shows.
        let s = surface(2000.0, 1000.0);
        let square = fit_rect(s, Vec2::new(720.0, 576.0), PixelAspect::SQUARE, Fit::Contain);
        let anamorphic = fit_rect(
            s,
            Vec2::new(720.0, 576.0),
            PixelAspect { num: 64, den: 45 },
            Fit::Contain,
        );
        assert!(
            anamorphic.width() > square.width(),
            "PAR ignored: {} vs {}",
            anamorphic.width(),
            square.width()
        );
        // 720 * 64/45 = 1024, so the picture is 1024x576 -> 16:9.
        approx(anamorphic.width() / anamorphic.height(), 16.0 / 9.0, "display aspect");
    }

    #[test]
    fn a_zero_denominator_falls_back_to_square_rather_than_dividing() {
        let s = surface(400.0, 400.0);
        let r = fit_rect(s, Vec2::new(1920.0, 1080.0), PixelAspect { num: 4, den: 0 }, Fit::Contain);
        assert!(r.width().is_finite() && r.width() > 0.0);
    }

    /// A NaN rect propagates into egui's layout and surfaces somewhere
    /// unrelated, so degenerate input has to be caught here.
    #[test]
    fn degenerate_input_gives_an_empty_rect_not_a_nan_one() {
        for (sw, sh, fw, fh) in [
            (0.0f32, 400.0f32, 1920.0f32, 1080.0f32),
            (400.0, 0.0, 1920.0, 1080.0),
            (400.0, 400.0, 0.0, 1080.0),
            (400.0, 400.0, 1920.0, 0.0),
        ] {
            let r = fit_rect(
                surface(sw, sh),
                Vec2::new(fw, fh),
                PixelAspect::SQUARE,
                Fit::Contain,
            );
            assert!(
                r.width().is_finite() && r.height().is_finite(),
                "NaN rect for {sw}x{sh} frame {fw}x{fh}"
            );
            assert_eq!(r.width(), 0.0);
        }
    }

    #[test]
    fn a_point_in_the_picture_maps_to_frame_coordinates() {
        let s = surface(400.0, 400.0);
        let size = Vec2::new(1920.0, 1080.0);
        // Contain puts a 400x225 picture centred at y = 20 + 200 = 220.
        let centre = s.center();
        let p = surface_to_frame(s, size, PixelAspect::SQUARE, Fit::Contain, centre).unwrap();
        approx(p.x, 960.0, "centre x");
        approx(p.y, 540.0, "centre y");
    }

    #[test]
    fn a_point_in_the_letterbox_bar_maps_to_nothing() {
        let s = surface(400.0, 400.0);
        let size = Vec2::new(1920.0, 1080.0);
        // Top edge of the surface is well above the 225-tall picture.
        let in_bar = pos2(s.center().x, s.min.y + 2.0);
        assert!(surface_to_frame(s, size, PixelAspect::SQUARE, Fit::Contain, in_bar).is_none());
    }
}
