//! Whether a frame can go to the compositor as-is (a subsurface holding the
//! decoder's dma-buf), skipping our conversion pass -- and, where the
//! compositor puts that subsurface on a hardware overlay plane, skipping
//! GPU composition of the video entirely. Pure.
//!
//! What the compositor does with it is its business: an overlay plane when
//! it can (the feedback's zero-copy flag says so), or its own GL
//! composition otherwise -- still no conversion pass in the player.

/// Everything the decision needs, gathered by the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    /// The user's setting: `Off`, `Auto` (only when exact), `Force`.
    pub mode: Mode,
    /// NV12, 8-bit (P010/HDR needs the colour-management protocol too).
    pub nv12: bool,
    /// The compositor advertised this (format, modifier) on linux-dmabuf.
    pub format_supported: bool,
    /// Something in our pipeline changes pixels: sharpening, upscaling,
    /// temporal denoise, film grain, a test pattern overlay.
    pub we_change_pixels: bool,
    /// The compositor can be TOLD the matrix and range (color-
    /// representation); without it, it guesses -- usually BT.601 limited.
    pub can_say_matrix: bool,
    /// The content is BT.601 limited: a guess of BT.601 limited is right.
    pub content_is_bt601_limited: bool,
    /// Subsurfaces, dmabuf and viewporter are all there.
    pub protocols: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Auto,
    Force,
}

impl Mode {
    /// `VIDKIT_SCANOUT`: 0/off, 1/auto, force.
    pub fn parse(v: Option<&str>) -> Self {
        match v {
            Some("1") | Some("auto") => Mode::Auto,
            Some("force") => Mode::Force,
            _ => Mode::Off,
        }
    }
}

/// `Ok` to hand frames to the compositor, or why not (for the status line
/// and profile: a scanout that silently does not happen is the failure
/// this exists to prevent).
pub fn decide(i: &Inputs) -> Result<(), &'static str> {
    if i.mode == Mode::Off {
        return Err("off");
    }
    if !i.protocols {
        return Err("compositor lacks subsurface/dmabuf/viewporter");
    }
    if !i.nv12 {
        return Err("not an 8-bit NV12 dma-buf");
    }
    if !i.format_supported {
        return Err("compositor does not take this format/modifier");
    }
    if i.we_change_pixels {
        return Err("sharpening/upscaling/denoise/grain needs our pass");
    }
    if i.mode == Mode::Auto && !i.can_say_matrix && !i.content_is_bt601_limited {
        return Err("compositor cannot be told the colour matrix (VIDKIT_SCANOUT=force to accept its guess)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Inputs {
        Inputs { mode: Mode::Auto, nv12: true, format_supported: true, we_change_pixels: false, can_say_matrix: true, content_is_bt601_limited: false, protocols: true }
    }

    #[test]
    fn each_blocker_says_why() {
        assert_eq!(decide(&ok()), Ok(()));
        assert!(decide(&Inputs { mode: Mode::Off, ..ok() }).is_err());
        assert!(decide(&Inputs { protocols: false, ..ok() }).unwrap_err().contains("subsurface"));
        assert!(decide(&Inputs { nv12: false, ..ok() }).is_err());
        assert!(decide(&Inputs { format_supported: false, ..ok() }).is_err());
        assert!(decide(&Inputs { we_change_pixels: true, ..ok() }).unwrap_err().contains("sharpening"));
        let guess = Inputs { can_say_matrix: false, ..ok() };
        assert!(decide(&guess).unwrap_err().contains("matrix"));
        assert_eq!(decide(&Inputs { content_is_bt601_limited: true, ..guess.clone() }), Ok(()), "the guess is right for BT.601");
        assert_eq!(decide(&Inputs { mode: Mode::Force, ..guess }), Ok(()));
    }

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse(None), Mode::Off);
        assert_eq!(Mode::parse(Some("1")), Mode::Auto);
        assert_eq!(Mode::parse(Some("force")), Mode::Force);
        assert_eq!(Mode::parse(Some("0")), Mode::Off);
    }
}
