//! Finding the face, and with it both eyes, with YuNet (OpenCV Zoo, MIT): a 230 KB detector whose output includes the
//! centre of each eye. It is the anchor the rest of the chain hangs from: crop an eye around its landmark and hand it to
//! orlosky-kit's pupil detector.
//!
//! Why this and not image processing: a dark-blob eye finder was tried on 57 selfie frames and found the collar and the
//! chin (docs/agents/FP5_PHONE_LEARNINGS.md). YuNet found the face in all 57 and put its eye landmarks on the eyes across
//! head poses, blur and dim light (checked by eye on a sample, and against OpenCV's own YuNet).
//!
//! The decode follows OpenCV's `FaceDetectorYN`: three strides (8, 16, 32), a cell's score is sqrt(cls * obj), the box is
//! centred at (col + dx) * stride with exp(dw) * stride sides, each of the 5 landmarks is (col + kx) * stride, and
//! overlapping boxes go through NMS. The input is BGR 0-255 padded with zeros to a multiple of 32.
//!
//! The face must be SMALL in the input: at 720x1280 with a face half the frame wide it finds 0 of 57 faces; at 0.35 scale
//! it finds 57. `detect_scaled` does that scaling.

use orlosky_kit::Gray;
use tract_onnx::prelude::*;

/// A detected face. Landmarks are in the pixels of the image passed in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// The face's right eye (the image's left when it faces the camera), left eye, nose tip, right and left mouth corners.
    pub eye_a: (f32, f32),
    pub eye_b: (f32, f32),
    pub nose: (f32, f32),
    pub mouth_a: (f32, f32),
    pub mouth_b: (f32, f32),
}

impl Face {
    /// The two eyes ordered by x: (left in the image, right in the image).
    pub fn eyes(&self) -> ((f32, f32), (f32, f32)) {
        if self.eye_a.0 <= self.eye_b.0 { (self.eye_a, self.eye_b) } else { (self.eye_b, self.eye_a) }
    }

    /// Distance between the eyes in pixels, the face's own ruler.
    pub fn eye_distance(&self) -> f32 {
        let (a, b) = self.eyes();
        (b.0 - a.0).hypot(b.1 - a.1)
    }

    fn scaled(&self, k: f32) -> Face {
        let s = |p: (f32, f32)| (p.0 * k, p.1 * k);
        Face { x: self.x * k, y: self.y * k, w: self.w * k, h: self.h * k, score: self.score, eye_a: s(self.eye_a), eye_b: s(self.eye_b), nose: s(self.nose), mouth_a: s(self.mouth_a), mouth_b: s(self.mouth_b) }
    }
}

pub struct YuNet {
    model: TypedRunnableModel<TypedModel>,
    pub w: usize,
    pub h: usize,
    pub score_threshold: f32,
    pub nms_threshold: f32,
}

type TypedRunnableModel<M> = RunnableModel<TypedFact, Box<dyn TypedOp>, M>;

impl YuNet {
    /// Load the model for inputs of `w` x `h` pixels (each rounded UP to a multiple of 32; smaller inputs are padded).
    pub fn load(path: &std::path::Path, w: usize, h: usize) -> TractResult<YuNet> {
        let (pw, ph) = (w.div_ceil(32) * 32, h.div_ceil(32) * 32);
        // the zoo's file bakes 640x640 into every intermediate shape; drop those annotations so another size can be inferred
        let raw = std::fs::read(path)?;
        let bytes = strip_value_info(&raw).ok_or_else(|| std::io::Error::other("not an ONNX model this loader understands"))?;
        let model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))?
            .with_input_fact(0, f32::fact([1, 3, ph, pw]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(YuNet { model, w: pw, h: ph, score_threshold: 0.6, nms_threshold: 0.3 })
    }

    /// Faces in a grey image of exactly the size this was loaded for (up to the padding), best first.
    pub fn detect(&self, g: &Gray) -> TractResult<Vec<Face>> {
        let (pw, ph) = (self.w, self.h);
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, ph, pw), |(_, _, y, x)| if x < g.w && y < g.h { g.at(x, y) as f32 } else { 0.0 });
        let out = self.model.run(tvec!(input.into_tensor().into()))?;
        let get = |i: usize| -> TractResult<Vec<f32>> { Ok(out[i].to_array_view::<f32>()?.iter().cloned().collect()) };
        let (cls, obj, bbox, kps) = ([get(0)?, get(1)?, get(2)?], [get(3)?, get(4)?, get(5)?], [get(6)?, get(7)?, get(8)?], [get(9)?, get(10)?, get(11)?]);
        let mut faces = Vec::new();
        for (k, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let (cols, rows) = (pw / stride, ph / stride);
            if cls[k].len() != cols * rows { return Err(std::io::Error::other(format!("unexpected output size {} for stride {stride}", cls[k].len())).into()); }
            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    let score = (cls[k][i].clamp(0.0, 1.0) * obj[k][i].clamp(0.0, 1.0)).sqrt();
                    if score < self.score_threshold { continue; }
                    let s = stride as f32;
                    let b = &bbox[k][i * 4..i * 4 + 4];
                    let (cx, cy, w, h) = ((c as f32 + b[0]) * s, (r as f32 + b[1]) * s, b[2].exp() * s, b[3].exp() * s);
                    let kp = |n: usize| ((kps[k][i * 10 + 2 * n] + c as f32) * s, (kps[k][i * 10 + 2 * n + 1] + r as f32) * s);
                    faces.push(Face { x: cx - w / 2.0, y: cy - h / 2.0, w, h, score, eye_a: kp(0), eye_b: kp(1), nose: kp(2), mouth_a: kp(3), mouth_b: kp(4) });
                }
            }
        }
        Ok(nms(faces, self.nms_threshold))
    }
}

fn read_varint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 { return Some(v); }
        shift += 7;
        if shift > 63 { return None; }
    }
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 { out.push(byte); return; }
        out.push(byte | 0x80);
    }
}

/// Copy a protobuf message, dropping every length-delimited field numbered `drop` (None on a malformed or group-using
/// message). `recurse` names one field whose payload is a message to filter the same way, with its own `drop`.
fn filter_message(b: &[u8], drop: Option<u64>, recurse: Option<(u64, u64)>) -> Option<Vec<u8>> {
    let (mut pos, mut out) = (0usize, Vec::with_capacity(b.len()));
    while pos < b.len() {
        let start = pos;
        let tag = read_varint(b, &mut pos)?;
        let (field, wire) = (tag >> 3, tag & 7);
        let end = match wire {
            0 => { read_varint(b, &mut pos)?; pos }
            1 => pos.checked_add(8)?,
            5 => pos.checked_add(4)?,
            2 => {
                let len = read_varint(b, &mut pos)? as usize;
                let end = pos.checked_add(len)?;
                if end > b.len() { return None; }
                if drop == Some(field) { pos = end; continue; }
                if let Some((f, inner_drop)) = recurse {
                    if f == field {
                        let inner = filter_message(&b[pos..end], Some(inner_drop), None)?;
                        write_varint(&mut out, tag);
                        write_varint(&mut out, inner.len() as u64);
                        out.extend_from_slice(&inner);
                        pos = end;
                        continue;
                    }
                }
                end
            }
            _ => return None,
        };
        if end > b.len() { return None; }
        out.extend_from_slice(&b[start..end]);
        pos = end;
    }
    Some(out)
}

/// An ONNX ModelProto without its graph's `value_info` (GraphProto field 13: the shapes of intermediate tensors).
pub fn strip_value_info(model: &[u8]) -> Option<Vec<u8>> { filter_message(model, None, Some((7, 13))) }

/// Greedy non-maximum suppression, best score first.
pub fn nms(mut faces: Vec<Face>, iou_max: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for f in faces {
        if kept.iter().all(|k| iou(k, &f) <= iou_max) { kept.push(f); }
    }
    kept
}

fn iou(a: &Face, b: &Face) -> f32 {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Detect in a frame of any size by scaling it to `target_w` wide first (`0.35` of 720 is 252), and report the faces
/// in the ORIGINAL frame's pixels. `net` must have been loaded for the scaled size.
pub fn detect_scaled(net: &YuNet, g: &Gray, target_w: usize) -> TractResult<Vec<Face>> {
    let k = target_w as f64 / g.w as f64;
    let th = (g.h as f64 * k).round() as usize;
    // averaging, not bilinear sampling: a 0.35x shrink that skips most pixels aliases, and the detector notices
    let small = g.resize_area(target_w, th);
    Ok(net.detect(&small)?.into_iter().map(|f| f.scaled((1.0 / k) as f32)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, w: f32, h: f32, score: f32) -> Face {
        let p = (x, y);
        Face { x, y, w, h, score, eye_a: (x + w * 0.3, y + h * 0.35), eye_b: (x + w * 0.7, y + h * 0.35), nose: p, mouth_a: p, mouth_b: p }
    }

    fn field(out: &mut Vec<u8>, number: u64, payload: &[u8]) {
        write_varint(out, number << 3 | 2);
        write_varint(out, payload.len() as u64);
        out.extend_from_slice(payload);
    }

    #[test]
    fn value_info_is_dropped_from_the_graph_and_everything_else_survives_byte_for_byte() {
        let mut graph = Vec::new();
        field(&mut graph, 1, b"a node");
        field(&mut graph, 13, &[7u8; 200]); // longer than 127: a two-byte length
        field(&mut graph, 11, b"an input");
        field(&mut graph, 13, b"more shapes");
        field(&mut graph, 12, b"an output");
        let mut model = vec![0x08, 0x08]; // ir_version = 8, a varint field
        field(&mut model, 7, &graph);
        field(&mut model, 8, b"opset");
        let out = strip_value_info(&model).unwrap();
        let mut want_graph = Vec::new();
        field(&mut want_graph, 1, b"a node");
        field(&mut want_graph, 11, b"an input");
        field(&mut want_graph, 12, b"an output");
        let mut want = vec![0x08, 0x08];
        field(&mut want, 7, &want_graph);
        field(&mut want, 8, b"opset");
        assert_eq!(out, want);
        // and a field 13 OUTSIDE the graph is not value_info
        let mut other = Vec::new();
        field(&mut other, 13, b"not in the graph");
        assert_eq!(strip_value_info(&other).unwrap(), other);
    }

    #[test]
    fn a_truncated_or_group_using_message_is_none_not_garbage() {
        let mut m = Vec::new();
        field(&mut m, 7, b"0123456789");
        m.truncate(m.len() - 3);
        assert!(strip_value_info(&m).is_none(), "the length runs past the end");
        assert!(strip_value_info(&[0x0b]).is_none(), "a start-group tag");
        assert!(strip_value_info(&[0x80]).is_none(), "a varint that never ends");
        assert_eq!(strip_value_info(&[]), Some(vec![]));
    }

    #[test]
    #[ignore = "needs the YuNet model: run tools/fetch_yunet.sh, then cargo test --features yunet -- --ignored"]
    fn the_real_model_loads_for_a_small_input_and_finds_nothing_in_a_blank_frame() {
        let path = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/eye-kit/models/yunet.onnx");
        let net = YuNet::load(&path, 252, 448).expect("the model loads at 252x448 (padded to 256x448)");
        assert_eq!((net.w, net.h), (256, 448));
        assert!(net.detect(&Gray::new(252, 448, 128)).unwrap().is_empty(), "no face in a flat grey frame");
    }

    #[test]
    fn overlapping_detections_collapse_to_the_best_and_separate_faces_stay() {
        let kept = nms(vec![face(10.0, 10.0, 100.0, 120.0, 0.7), face(14.0, 12.0, 100.0, 120.0, 0.9), face(300.0, 10.0, 100.0, 120.0, 0.65)], 0.3);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9, "the best of the pair survives, and comes first");
        assert_eq!(kept[1].score, 0.65);
    }

    #[test]
    fn eyes_are_ordered_by_x_and_scaling_moves_every_landmark() {
        let mut f = face(0.0, 0.0, 100.0, 100.0, 0.9);
        std::mem::swap(&mut f.eye_a, &mut f.eye_b);
        let (l, r) = f.eyes();
        assert!(l.0 < r.0, "left then right whichever order the model gave them");
        assert!((f.eye_distance() - 40.0).abs() < 1e-4);
        let big = f.scaled(2.0);
        assert!((big.eye_distance() - 80.0).abs() < 1e-3 && big.w == 200.0);
    }
}
