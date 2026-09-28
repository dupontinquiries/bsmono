//! capture — the thing that finally produces a frame.
//!
//! The engine lives in `shared/capture-kit` now: the plan walk, the
//! latest-wins slot, the stop contract, the bounded restarts, the
//! `gst-launch-1.0` subprocess opener and the plain test pattern were
//! written and verified here (see the kit's module doc for the reasoning
//! that came with them) and moved when snapshot-cam needed the same engine.
//! Everything this app used is re-exported under the names it used, so the
//! host's `CaptureRig` and the app's `CaptureStatus` event did not change.
//!
//! What stays here is the one thing snapclone adds: a test pattern with a
//! **code held up to the lens**, so the friends flow — show a code, scan a
//! code — can be walked on a machine with no second phone. It draws the
//! kit's identicon layout onto the kit's pattern, slowly turning, and the
//! scanner has something to find at an angle.

pub use capture_kit::{
    gst_opener, run, shell_opener, FrameStream, KillSwitch, RawFrame, Shared, SourceCandidate,
    Status, MAX_RESTARTS,
};

/// The kit's synthetic camera, optionally with a code in view.
///
/// `SNAPCLONE_TEST_PATTERN=1` runs it; `SNAPCLONE_TEST_PATTERN_CODE=<sc-id>`
/// puts that id's code on a light card in every frame. Not a test fake: the
/// real capture thread, frame pump, texture upload, shutter and encoder all
/// run; only the pixels come from arithmetic.
pub struct TestPattern {
    width: u32,
    height: u32,
    frame: u64,
    code: Option<uikit_egui::qr::Layout>,
}

impl TestPattern {
    pub fn new(width: u32, height: u32) -> TestPattern {
        TestPattern { width, height, frame: 0, code: None }
    }

    pub fn with_code(mut self, id: Option<String>) -> TestPattern {
        self.code = id.map(|i| uikit_egui::qr::layout_for(&i));
        self
    }
}

impl FrameStream for TestPattern {
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if buf.len() < w * h * 4 {
            return Err("test pattern: buffer smaller than a frame".into());
        }
        capture_kit::TestPattern::paint(self.width, self.height, self.frame, buf);
        if let Some(l) = &self.code {
            // a light card, then the code turning a little either way
            let t = self.frame as f32 * 0.05;
            let side = (h.min(w) as f32) * 0.5;
            let cell = (side / 21.0).floor().max(2.0);
            let used = cell * 21.0;
            let o = ((w as f32 - used) / 2.0, (h as f32 - used) / 2.0);
            let pad = cell * 2.0;
            for y in 0..h {
                for x in 0..w {
                    if (x as f32) >= o.0 - pad && (x as f32) < o.0 + used + pad
                        && (y as f32) >= o.1 - pad && (y as f32) < o.1 + used + pad {
                        let i = (y * w + x) * 4;
                        buf[i..i + 4].copy_from_slice(&[236, 232, 224, 255]);
                    }
                }
            }
            let rot = (t * 0.7).sin() * 0.35;
            uikit_egui::qr::rasterize(l, buf, w, h, o, cell, rot, [22, 22, 24]);
        }
        self.frame += 1;
        std::thread::sleep(std::time::Duration::from_millis(66));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pattern_with_a_code_is_decodable_by_the_kits_scanner() {
        // The reason this pattern exists: a code in the frame that the
        // scanner finds, so the friends flow can be walked without a
        // second phone. Held here rather than only in a screenshot.
        let id = "sc-8fd30abd";
        let (w, h) = (640u32, 480u32);
        let mut p = TestPattern::new(w, h).with_code(Some(id.into()));
        let mut buf = vec![0u8; (w * h * 4) as usize];
        // A few frames in, so the card has turned a little.
        let mut found = None;
        for _ in 0..6 {
            p.read_frame(&mut buf).unwrap();
            if let Some(got) = uikit_egui::qr::decode_frame(&buf, w as usize, h as usize) {
                found = Some(got);
                break;
            }
        }
        assert_eq!(found.as_deref(), Some(id), "the scanner did not read the pattern's code");
        // and the plain pattern has no code in it
        let mut plain = TestPattern::new(w, h);
        plain.read_frame(&mut buf).unwrap();
        assert_eq!(uikit_egui::qr::decode_frame(&buf, w as usize, h as usize), None);
    }
}
