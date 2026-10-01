//! orient — turning a sensor's frames upright.
//!
//! # The bug this is for
//!
//! A phone's camera sensor is mounted in its own orientation and the pipeline
//! hands over its pixels as they come. On a Fairphone 5 the kernel says so:
//! `camera_sensor_rotation = 270`, `camera_orientation = Back`
//! (`v4l2-ctl -d /dev/v4l-subdevN -l`), and `libcamerasrc` applies none of it.
//! A viewfinder that draws the frame as received is therefore rotated, in every
//! app that does — snapshot-cam, snapclone-egui and tideshell's lock screen all
//! drew the frame as-is.
//!
//! This is the one place that knows how to turn it right, so the three apps
//! share a fix instead of each growing a rotate button.
//!
//! # What is NOT settled
//!
//! The SIGN. The V4L2 control gives a number of degrees; whether the image must
//! be turned that many degrees clockwise or counter-clockwise to be upright is a
//! convention the kernel docs and libcamera's `Rotation` property state
//! differently, and no frame with a known "up" has been captured on this phone
//! yet. [`Rotation::upright_for_sensor`] encodes one reading and is a single
//! function to flip; `SNAPSHOT_CAM_ROTATE` / the apps' override (`quarter turns
//! clockwise`) exist so the answer can be set from what is seen on screen.

use crate::RawFrame;
use std::sync::Arc;

/// A clockwise turn to apply to a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation { #[default] None, Cw90, Half, Ccw90 }

impl Rotation {
    pub fn degrees(self) -> u32 { match self { Rotation::None => 0, Rotation::Cw90 => 90, Rotation::Half => 180, Rotation::Ccw90 => 270 } }
    /// Any number of degrees, wrapped; anything that is not a multiple of 90 is no turn.
    pub fn from_degrees(d: i64) -> Rotation {
        match d.rem_euclid(360) { 90 => Rotation::Cw90, 180 => Rotation::Half, 270 => Rotation::Ccw90, _ => Rotation::None }
    }
    /// Quarter turns clockwise, wrapped (the override a person types).
    pub fn from_quarter_turns(n: i64) -> Rotation { Rotation::from_degrees(n.rem_euclid(4) * 90) }
    /// Does this turn swap width and height?
    pub fn swaps_axes(self) -> bool { matches!(self, Rotation::Cw90 | Rotation::Ccw90) }
    /// This turn followed by `other`.
    pub fn then(self, other: Rotation) -> Rotation { Rotation::from_degrees((self.degrees() + other.degrees()) as i64) }
    pub fn inverse(self) -> Rotation { Rotation::from_degrees(-(self.degrees() as i64)) }

    /// The turn that makes a sensor's output upright, given the mounting
    /// rotation the driver reports. ONE READING of an ambiguous convention —
    /// see the module doc. Flip here, nowhere else.
    pub fn upright_for_sensor(sensor_rotation_degrees: i64) -> Rotation {
        Rotation::from_degrees(sensor_rotation_degrees)
    }
}

impl RawFrame {
    /// The frame turned clockwise by `r`. Pixels are moved, not resampled. A
    /// torn frame comes back as it is (the upload refuses it), `captured_ms`
    /// and `seq` are kept: it is the same moment.
    pub fn rotated(&self, r: Rotation) -> RawFrame {
        if r == Rotation::None || !self.pixels_plausible() { return self.clone(); }
        let (w, h) = (self.width as usize, self.height as usize);
        let (nw, nh) = if r.swaps_axes() { (h, w) } else { (w, h) };
        let mut out = vec![0u8; nw * nh * 4];
        for y in 0..h {
            for x in 0..w {
                // Where does the source pixel (x, y) land?
                let (dx, dy) = match r {
                    Rotation::Cw90 => (h - 1 - y, x),
                    Rotation::Half => (w - 1 - x, h - 1 - y),
                    Rotation::Ccw90 => (y, w - 1 - x),
                    Rotation::None => (x, y),
                };
                let (s, d) = ((y * w + x) * 4, (dy * nw + dx) * 4);
                out[d..d + 4].copy_from_slice(&self.rgba[s..s + 4]);
            }
        }
        RawFrame { width: nw as u32, height: nh as u32, captured_ms: self.captured_ms, seq: self.seq, rgba: Arc::from(out) }
    }
}

/// Which way a camera faces, as the driver reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing { Front, Back, External }

/// What a sensor says about its mounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mounting { pub rotation_degrees: i64, pub facing: Option<Facing> }

/// Parse `v4l2-ctl -l` for `camera_sensor_rotation` and `camera_orientation`.
/// `None` when the driver reports neither.
pub fn parse_mounting(list_ctrls: &str) -> Option<Mounting> {
    let find = |name: &str| list_ctrls.lines().find(|l| l.trim_start().starts_with(name));
    let value = |l: &str| l.split_whitespace().find_map(|t| t.strip_prefix("value=")?.parse::<i64>().ok());
    let rotation = find("camera_sensor_rotation").and_then(value);
    let facing = find("camera_orientation").and_then(|l| {
        // "value=1 (Back)": the label is the stable part, the number is the menu index.
        let open = l.rfind("value=")?;
        let tail = &l[open..];
        let label = tail.split('(').nth(1)?.split(')').next()?.trim().to_ascii_lowercase();
        match label.as_str() { "front" => Some(Facing::Front), "back" => Some(Facing::Back), "external" => Some(Facing::External), _ => None }
    });
    if rotation.is_none() && facing.is_none() { return None; }
    Some(Mounting { rotation_degrees: rotation.unwrap_or(0), facing })
}

/// The mounting of the sensor behind `device`, via `v4l2-ctl`.
pub fn read_mounting(prog: &str, device: &str) -> Result<Mounting, String> {
    let out = std::process::Command::new(prog).args(["-d", device, "-l"]).output().map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() { return Err(format!("{prog} -d {device} -l: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    parse_mounting(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| format!("{device}: no rotation or orientation control"))
}

/// The turn to apply for a camera: the person's override if there is one,
/// else what the sensor's mounting implies, else none. A front camera's
/// MIRROR is the viewfinder's business and is not touched here.
pub fn turn_for(override_quarter_turns: Option<i64>, mounting: Option<Mounting>) -> Rotation {
    match (override_quarter_turns, mounting) {
        (Some(n), _) => Rotation::from_quarter_turns(n),
        (None, Some(m)) => Rotation::upright_for_sensor(m.rotation_degrees),
        (None, None) => Rotation::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 wide x 2 tall, every pixel distinct: [[A B C],[D E F]].
    fn abc() -> RawFrame {
        let px: Vec<u8> = (0u8..6).flat_map(|i| [i * 10, i * 10 + 1, i * 10 + 2, 255]).collect();
        RawFrame { width: 3, height: 2, captured_ms: 7, seq: 9, rgba: Arc::from(px) }
    }
    fn ids(f: &RawFrame) -> Vec<u8> { f.rgba.chunks(4).map(|p| p[0] / 10).collect() }

    #[test]
    fn a_clockwise_quarter_turn_puts_the_left_column_on_top() {
        // [[0 1 2],[3 4 5]] turned clockwise is [[3 0],[4 1],[5 2]].
        let r = abc().rotated(Rotation::Cw90);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(ids(&r), vec![3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn counter_clockwise_and_half_turn_are_what_they_say() {
        // CCW of [[0 1 2],[3 4 5]] is [[2 5],[1 4],[0 3]].
        assert_eq!(ids(&abc().rotated(Rotation::Ccw90)), vec![2, 5, 1, 4, 0, 3]);
        let h = abc().rotated(Rotation::Half);
        assert_eq!((h.width, h.height), (3, 2));
        assert_eq!(ids(&h), vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn four_quarter_turns_and_inverse_pairs_return_the_frame() {
        let f = abc();
        let mut r = f.clone();
        for _ in 0..4 { r = r.rotated(Rotation::Cw90); }
        assert_eq!(r, f);
        assert_eq!(f.rotated(Rotation::Cw90).rotated(Rotation::Ccw90), f);
        assert_eq!(f.rotated(Rotation::Half).rotated(Rotation::Half), f);
        for r in [Rotation::None, Rotation::Cw90, Rotation::Half, Rotation::Ccw90] {
            assert_eq!(f.rotated(r).rotated(r.inverse()), f, "{r:?}");
        }
    }

    #[test]
    fn the_moment_survives_and_a_torn_frame_is_left_alone() {
        let r = abc().rotated(Rotation::Cw90);
        assert_eq!((r.captured_ms, r.seq), (7, 9));
        let torn = RawFrame { rgba: Arc::from(vec![1u8; 5]), ..abc() };
        assert_eq!(torn.rotated(Rotation::Cw90), torn);
    }

    #[test]
    fn degrees_wrap_and_nonsense_is_no_turn() {
        assert_eq!(Rotation::from_degrees(450), Rotation::Cw90);
        assert_eq!(Rotation::from_degrees(-90), Rotation::Ccw90);
        assert_eq!(Rotation::from_degrees(37), Rotation::None);
        assert_eq!(Rotation::from_quarter_turns(-1), Rotation::Ccw90);
        assert_eq!(Rotation::Cw90.then(Rotation::Ccw90), Rotation::None);
        assert!(Rotation::Cw90.swaps_axes() && !Rotation::Half.swaps_axes());
    }

    #[test]
    fn the_fp5_rear_sensor_mounting_parses() {
        // Read off the phone: v4l2-ctl -d /dev/v4l-subdev29 -l
        let t = "Camera Controls\n\n             camera_orientation 0x009a0922 (menu)   : min=0 max=2 default=1 value=1 (Back) flags=read-only, has-min-max\n         camera_sensor_rotation 0x009a0923 (int)    : min=270 max=270 step=1 default=270 value=270 flags=read-only, has-min-max\n";
        assert_eq!(parse_mounting(t), Some(Mounting { rotation_degrees: 270, facing: Some(Facing::Back) }));
        assert_eq!(parse_mounting("exposure 0x1 (int) : min=1 max=2 value=1"), None);
        let front = "camera_orientation 0x1 (menu) : min=0 max=2 default=0 value=0 (Front) flags=read-only";
        assert_eq!(parse_mounting(front).unwrap().facing, Some(Facing::Front));
    }

    #[test]
    fn an_override_beats_the_sensor_and_none_means_upright_already() {
        let m = Some(Mounting { rotation_degrees: 270, facing: Some(Facing::Back) });
        assert_eq!(turn_for(Some(1), m), Rotation::Cw90, "the override wins");
        assert_eq!(turn_for(None, m), Rotation::upright_for_sensor(270));
        assert_eq!(turn_for(None, None), Rotation::None);
        assert_eq!(turn_for(Some(0), m), Rotation::None, "an explicit 0 is a choice, not absence");
    }
}
