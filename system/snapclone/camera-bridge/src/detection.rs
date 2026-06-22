//! YOLO face detection via tract-onnx.
//!
//! # ndarray version
//! tract-onnx 0.21 does NOT re-export ndarray through its prelude.
//! The correct approach is to declare ndarray in Cargo.toml with
//! `ndarray = "0.16"` (matching tract's internal dep) so Cargo unifies
//! them to one instance.  The Cargo.toml also sets
//! `[patch.crates-io] ndarray = ...` or just pins the minor version so
//! Cargo picks the same resolution as tract.
//!
//! If you still see a "two versions" error, run:
//!   cargo tree -i ndarray
//! and set ndarray in Cargo.toml to the version tract resolved to.

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, ImageBuffer, Rgb};
use log::debug;
use ndarray::{Array, ArrayView, IxDyn};
use tract_onnx::prelude::*;

use crate::capture::Frame;

// ─── model constants — adjust to match your specific model ──────────────────
const MODEL_W: usize = 320;
const MODEL_H: usize = 320;
const CONF_THRESHOLD: f32 = 0.45;
const IOU_THRESHOLD: f32 = 0.45;
// ────────────────────────────────────────────────────────────────────────────

/// A single detected face.
#[derive(Debug, Clone)]
pub struct Detection {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
    pub confidence: f32,
}

/// Wraps the compiled tract model.
pub struct FaceDetector {
    model: SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>,
}

impl FaceDetector {
    pub fn new(path: &str) -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .context("Failed to load ONNX model")?
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec![1, 3, MODEL_H as i64, MODEL_W as i64],
                ),
            )?
            .into_optimized()
            .context("Model optimisation failed")?
            .into_runnable()
            .context("Failed to make model runnable")?;

        Ok(Self { model })
    }

    pub fn detect(&self, frame: &Frame) -> Result<Vec<Detection>> {
        // 1. Borrow frame data as an ImageBuffer view (no allocation).
        let img: ImageBuffer<Rgb<u8>, &[u8]> =
            ImageBuffer::from_raw(frame.width, frame.height, frame.data.as_slice())
                .ok_or_else(|| {
                    anyhow!(
                        "Cannot build ImageBuffer from frame ({}×{}, {} bytes)",
                        frame.width, frame.height, frame.data.len()
                    )
                })?;

        // 2. Resize to model input size (Nearest is fast enough at this scale).
        let resized =
            image::imageops::resize(&img, MODEL_W as u32, MODEL_H as u32, FilterType::Nearest);

        // 3. Build CHW float tensor normalised to [0, 1].
        //    Three separate passes — each writes a contiguous memory region,
        //    which is cache-sequential on ARM Cortex-A55.
        let total = MODEL_H * MODEL_W;
        let raw   = resized.as_raw(); // &[u8], interleaved RGB
        let mut tensor_data = vec![0f32; 3 * total];
        for i in 0..total {
            tensor_data[i]             = raw[i * 3]     as f32 * (1.0 / 255.0);
            tensor_data[total + i]     = raw[i * 3 + 1] as f32 * (1.0 / 255.0);
            tensor_data[2 * total + i] = raw[i * 3 + 2] as f32 * (1.0 / 255.0);
        }

        // 4. ndarray → tract Tensor.
        //    .into_tensor() is the tract 0.21 API (was .into_arc_tensor() in 0.20).
        let input: Tensor = Array::from_shape_vec(
            [1usize, 3, MODEL_H, MODEL_W],
            tensor_data,
        )
        .context("Failed to build input ndarray")?
        .into_dyn()
        .into_tensor();

        // 5. Run inference.
        let outputs = self
            .model
            .run(tvec![input.into()])
            .context("Inference failed")?;

        // 6. Parse + rescale bboxes.
        let output_tensor = outputs[0].to_array_view::<f32>()?;
        let scale_x = frame.width as f32 / MODEL_W as f32;
        let scale_y = frame.height as f32 / MODEL_H as f32;

        let mut detections = parse_output(output_tensor, scale_x, scale_y, CONF_THRESHOLD);
        nms(&mut detections, IOU_THRESHOLD);

        debug!("Detected {} face(s)", detections.len());
        Ok(detections)
    }
}

fn parse_output(
    tensor: ArrayView<f32, IxDyn>,
    sx: f32,
    sy: f32,
    threshold: f32,
) -> Vec<Detection> {
    let shape = tensor.shape();
    let mut detections = Vec::new();

    if shape.len() != 3 {
        return detections;
    }

    let (_, dim1, dim2) = (shape[0], shape[1], shape[2]);
    let use_layout_b = dim2 >= dim1 && dim1 >= 5;

    if !use_layout_b && dim2 >= 5 {
        // Layout A: [1, N_anchors, ≥5]
        for i in 0..dim1 {
            let conf = tensor[[0, i, 4]];
            if conf < threshold { continue; }
            detections.push(bbox_to_detection(
                tensor[[0,i,0]], tensor[[0,i,1]],
                tensor[[0,i,2]], tensor[[0,i,3]],
                conf, sx, sy,
            ));
        }
    } else if use_layout_b {
        // Layout B: [1, ≥5, N_anchors] — transposed
        for j in 0..dim2 {
            let conf = tensor[[0, 4, j]];
            if conf < threshold { continue; }
            detections.push(bbox_to_detection(
                tensor[[0,0,j]], tensor[[0,1,j]],
                tensor[[0,2,j]], tensor[[0,3,j]],
                conf, sx, sy,
            ));
        }
    }

    detections
}

#[inline]
fn bbox_to_detection(
    cx: f32, cy: f32, w: f32, h: f32, conf: f32,
    sx: f32, sy: f32,
) -> Detection {
    Detection {
        x_min: ((cx - w / 2.0) * sx).max(0.0),
        y_min: ((cy - h / 2.0) * sy).max(0.0),
        x_max: ((cx + w / 2.0) * sx).max(0.0),
        y_max: ((cy + h / 2.0) * sy).max(0.0),
        confidence: conf,
    }
}

fn nms(detections: &mut Vec<Detection>, iou_threshold: f32) {
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = vec![true; detections.len()];
    for i in 0..detections.len() {
        if !keep[i] { continue; }
        for j in (i + 1)..detections.len() {
            if keep[j] && iou(&detections[i], &detections[j]) > iou_threshold {
                keep[j] = false;
            }
        }
    }

    let mut idx = 0;
    detections.retain(|_| { let k = keep[idx]; idx += 1; k });
}

fn iou(a: &Detection, b: &Detection) -> f32 {
    let inter_w = (a.x_max.min(b.x_max) - a.x_min.max(b.x_min)).max(0.0);
    let inter_h = (a.y_max.min(b.y_max) - a.y_min.max(b.y_min)).max(0.0);
    let inter   = inter_w * inter_h;
    let union   = (a.x_max-a.x_min)*(a.y_max-a.y_min)
                + (b.x_max-b.x_min)*(b.y_max-b.y_min)
                - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}
