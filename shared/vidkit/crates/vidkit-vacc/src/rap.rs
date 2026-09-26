//! Random-access points: where a batch of frames can start and decode on
//! its own. Pure bitstream parsing, per codec.
//!
//! The batch rule the whole-bitstream decoders need (vacc's Vulkan path has
//! only `decode_all`): cut only where every reference resets and the frame
//! is SHOWN. Anything else joins the current batch, so no batch ever needs
//! frames from the one before it.

/// A small MSB-first bit reader.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    fn bit(&mut self) -> Option<u32> {
        let b = *self.d.get(self.pos / 8)?;
        let v = (b >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(v as u32)
    }
    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |a, _| Some((a << 1) | self.bit()?))
    }
}

/// VP9: a packet (one frame, or a superframe whose LAST frame is the shown
/// one) that is a keyframe. Uncompressed header: frame_marker(2)=2,
/// profile(2 bits, low first), [reserved(1) if profile 3],
/// show_existing_frame(1), frame_type(1): 0 = KEY_FRAME, show_frame(1).
/// A keyframe resets every reference slot.
pub fn vp9_is_rap(packet: &[u8]) -> bool {
    // Superframe index at the end: marker byte 0b110xxxxx at both ends of
    // the index. The frames inside are listed by size; the keyframe, if
    // any, is the first frame.
    let first = vp9_superframe_first(packet).unwrap_or(packet);
    let mut b = Bits::new(first);
    let Some(2) = b.bits(2) else { return false };
    let (lo, hi) = (b.bit().unwrap_or(0), b.bit().unwrap_or(0));
    if (hi << 1 | lo) == 3 && b.bit() != Some(0) {
        return false;
    }
    if b.bit() != Some(0) {
        return false; // show_existing_frame
    }
    b.bit() == Some(0) && b.bit() == Some(1)
}

fn vp9_superframe_first(p: &[u8]) -> Option<&[u8]> {
    let last = *p.last()?;
    if last & 0xe0 != 0xc0 {
        return None;
    }
    let frames = (last & 0x7) as usize + 1;
    let mag = ((last >> 3) & 0x3) as usize + 1;
    let idx_len = 2 + mag * frames;
    if p.len() < idx_len || p[p.len() - idx_len] != last {
        return None;
    }
    let idx = &p[p.len() - idx_len + 1..];
    let size = (0..mag).fold(0usize, |a, i| a | (idx[i] as usize) << (8 * i));
    p.get(..size)
}

/// AV1: a temporal unit that is a random-access point -- a sequence header
/// and a KEY_FRAME that is shown. A forward keyframe (KEY_FRAME with
/// show_frame = 0, shown later) is open-GOP and is NOT one.
pub fn av1_is_rap(tu: &[u8]) -> bool {
    let mut i = 0;
    let mut reduced_still = None;
    let mut key_shown = false;
    while i < tu.len() {
        let h = tu[i];
        let obu_type = (h >> 3) & 0xf;
        let ext = (h >> 2) & 1 == 1;
        let has_size = (h >> 1) & 1 == 1;
        i += 1 + ext as usize;
        let (size, n) = if has_size { match leb128(&tu[i.min(tu.len())..]) { Some(v) => v, None => return false } } else { (tu.len() - i, 0) };
        i += n;
        let Some(payload) = tu.get(i..i + size) else { return false };
        match obu_type {
            // OBU_SEQUENCE_HEADER: seq_profile(3) still_picture(1)
            // reduced_still_picture_header(1).
            1 => {
                let mut b = Bits::new(payload);
                let _ = b.bits(4);
                reduced_still = b.bit().map(|v| v == 1);
            }
            // OBU_FRAME_HEADER / OBU_FRAME: with a full header,
            // show_existing_frame(1), frame_type(2): 0 = KEY_FRAME,
            // show_frame(1).
            3 | 6 => {
                match reduced_still {
                    Some(true) => key_shown = true,
                    Some(false) => {
                        let mut b = Bits::new(payload);
                        if b.bit() == Some(0) && b.bits(2) == Some(0) && b.bit() == Some(1) {
                            key_shown = true;
                        }
                    }
                    None => return false, // no sequence header in this TU
                }
                break;
            }
            _ => {}
        }
        i += size;
    }
    key_shown
}

fn leb128(d: &[u8]) -> Option<(usize, usize)> {
    let mut v = 0usize;
    for (i, b) in d.iter().take(8).enumerate() {
        v |= ((b & 0x7f) as usize) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

/// H.264: an access unit containing an IDR slice (NAL type 5).
pub fn h264_is_rap(au: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < au.len() {
        let sc = if au[i..].starts_with(&[0, 0, 1]) { 3 } else if au[i..].starts_with(&[0, 0, 0, 1]) { 4 } else { 0 };
        if sc > 0 {
            if let Some(h) = au.get(i + sc) {
                if h & 0x1f == 5 {
                    return true;
                }
            }
            i += sc;
        } else {
            i += 1;
        }
    }
    false
}

/// Frames of an IVF file: (header fourcc, frames). Pure.
pub fn ivf_frames(data: &[u8]) -> Option<([u8; 4], Vec<&[u8]>)> {
    if data.get(..4)? != b"DKIF" {
        return None;
    }
    let hdr = u16::from_le_bytes([data[6], data[7]]) as usize;
    let fourcc = [data[8], data[9], data[10], data[11]];
    let mut out = Vec::new();
    let mut i = hdr;
    while i + 12 <= data.len() {
        let n = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        out.push(data.get(i + 12..i + 12 + n)?);
        i += 12 + n;
    }
    Some((fourcc, out))
}

/// Wrap frames as an IVF file -- the container vacc's VP9/AV1 paths read --
/// for one batch. Pure.
pub fn ivf_wrap(fourcc: [u8; 4], width: u16, height: u16, frames: &[&[u8]]) -> Vec<u8> {
    let mut o = Vec::with_capacity(32 + frames.iter().map(|f| f.len() + 12).sum::<usize>());
    o.extend_from_slice(b"DKIF");
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&32u16.to_le_bytes());
    o.extend_from_slice(&fourcc);
    o.extend_from_slice(&width.to_le_bytes());
    o.extend_from_slice(&height.to_le_bytes());
    o.extend_from_slice(&30u32.to_le_bytes());
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    for (i, f) in frames.iter().enumerate() {
        o.extend_from_slice(&(f.len() as u32).to_le_bytes());
        o.extend_from_slice(&(i as u64).to_le_bytes());
        o.extend_from_slice(f);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    /// Encode with `enc` into IVF; return (the IVF bytes, ffprobe's
    /// key_frame flag per packet).
    fn encoded(tag: &str, enc: &[&str]) -> Option<(Vec<u8>, Vec<bool>)> {
        let p = std::env::temp_dir().join(format!("vk-rap-{tag}-{}.ivf", std::process::id()));
        let mut args = vec!["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=6"];
        args.extend_from_slice(enc);
        let ok = Command::new("ffmpeg").args(&args).arg(&p).status().ok()?.success();
        if !ok {
            return None;
        }
        let probe = Command::new("ffprobe").args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=flags", "-of", "csv=p=0"]).arg(&p).output().ok()?;
        let keys = String::from_utf8_lossy(&probe.stdout).lines().map(|l| l.starts_with('K')).collect();
        let data = std::fs::read(&p).ok()?;
        let _ = std::fs::remove_file(&p);
        Some((data, keys))
    }

    /// Real libvpx-vp9 output (with alt-ref superframes): our keyframe
    /// reading agrees with ffprobe's on every packet.
    #[test]
    fn vp9_keyframes_match_ffprobe() {
        if !have_ffmpeg() {
            return;
        }
        let Some((ivf, keys)) = encoded("vp9", &["-c:v", "libvpx-vp9", "-g", "60", "-auto-alt-ref", "1", "-lag-in-frames", "16", "-deadline", "realtime", "-cpu-used", "8", "-f", "ivf"]) else { return };
        let (fourcc, frames) = ivf_frames(&ivf).unwrap();
        assert_eq!(&fourcc, b"VP90");
        assert_eq!(frames.len(), keys.len());
        let ours: Vec<bool> = frames.iter().map(|f| vp9_is_rap(f)).collect();
        assert_eq!(ours, keys, "vp9 keyframe mismatch");
        assert!(ours.iter().filter(|k| **k).count() >= 3, "a keyframe every 60 frames");
    }

    /// Real libsvtav1 output: every packet ffprobe calls a keyframe is one
    /// we would cut at, and the answer to whether libsvtav1's default GOPs
    /// are closed (shown keyframes) or open (forward keyframes).
    #[test]
    fn svtav1_random_access_points() {
        if !have_ffmpeg() {
            return;
        }
        let Some((ivf, keys)) = encoded("av1", &["-c:v", "libsvtav1", "-preset", "12", "-g", "60", "-f", "ivf"]) else { return };
        let (fourcc, frames) = ivf_frames(&ivf).unwrap();
        assert_eq!(&fourcc, b"AV01");
        let ours: Vec<bool> = frames.iter().map(|f| av1_is_rap(f)).collect();
        let (n_ours, n_probe) = (ours.iter().filter(|k| **k).count(), keys.iter().filter(|k| **k).count());
        eprintln!("DIAG libsvtav1 -g 60: {} TUs, ffprobe keyframes {n_probe}, our random-access points {n_ours}", frames.len());
        assert!(ours[0], "the stream starts at a random-access point");
        for (i, (o, k)) in ours.iter().zip(&keys).enumerate() {
            if *o {
                assert!(*k, "TU {i}: we cut where ffprobe sees no keyframe");
            }
        }
        assert!(n_ours >= 3, "cuts every ~60 frames: {n_ours}");
    }

    #[test]
    fn ivf_round_trips() {
        let a: &[u8] = &[1, 2, 3];
        let b: &[u8] = &[4];
        let w = ivf_wrap(*b"VP90", 320, 180, &[a, b]);
        let (cc, f) = ivf_frames(&w).unwrap();
        assert_eq!((&cc, f), (b"VP90", vec![a, b]));
    }
}
