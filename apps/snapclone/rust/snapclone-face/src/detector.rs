//! Face detection via YOLO-class ONNX model.
//!
//! Currently a public-API mirror of `camera-bridge/src/detection.rs`.
//! When the bridge is refactored to consume this crate (separate turn),
//! the real implementation moves here and the bridge becomes a thin
//! wrapper. For now the implementation IS in the bridge — this file
//! provides the type names so consumers can write code today against
//! the eventual home.

use crate::FaceError;

/// One detected face with its bounding box in pixel coordinates.
///
/// Coordinates are floats because the source image may be resized
/// before detection and we want sub-pixel accuracy when mapping the
/// bbox back to the original frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    /// Left edge, in source-image pixels.
    pub x_min: f32,
    /// Top edge, in source-image pixels.
    pub y_min: f32,
    /// Right edge, in source-image pixels.
    pub x_max: f32,
    /// Bottom edge, in source-image pixels.
    pub y_max: f32,
    /// Model confidence in [0.0, 1.0].
    pub confidence: f32,
}

impl Detection {
    /// Width of the bounding box.
    pub fn width(&self) -> f32 { (self.x_max - self.x_min).max(0.0) }
    /// Height of the bounding box.
    pub fn height(&self) -> f32 { (self.y_max - self.y_min).max(0.0) }
    /// Area in source-image pixels.
    pub fn area(&self) -> f32 { self.width() * self.height() }
    /// Centre point of the bounding box.
    pub fn centre(&self) -> (f32, f32) {
        ((self.x_min + self.x_max) * 0.5, (self.y_min + self.y_max) * 0.5)
    }
}

/// Tunable thresholds for detection.
///
/// Defaults match the camera-bridge YOLO config so the two detection
/// paths produce consistent results on the same input.
#[derive(Debug, Clone, Copy)]
pub struct DetectorConfig {
    /// Minimum confidence to emit a detection.
    pub conf_threshold: f32,
    /// IoU (intersection-over-union) threshold for non-max suppression.
    /// Lower = stricter de-duplication of overlapping boxes.
    pub iou_threshold: f32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self { conf_threshold: 0.45, iou_threshold: 0.45 }
    }
}

/// Face detector backed by an ONNX model.
///
/// Construction loads the model from disk — expensive (50-200ms),
/// do it once per process. Inference (`detect()`) is per-frame —
/// budget for ~30-60ms on FP5-class ARM.
///
/// Thread-safety: the inner tract `SimplePlan` is `Send + Sync` once
/// built. Wrapping a `Detector` in `Arc` and calling `detect()` from
/// multiple threads is safe. The `detect()` signature takes `&self`
/// rather than `&mut self` for this reason.
///
/// Implementation note (V32.32): this Detector mirrors the YOLO
/// detection logic in `camera-bridge/src/detection.rs`. The bridge
/// runs detection in a separate process for tracking-mode cameras;
/// this crate exposes the same capability for in-app code paths
/// (photo capture, inbox decoding). The two implementations are
/// independent but share model file format and constants, so they
/// produce equivalent results on equivalent inputs.
pub struct Detector {
    config: DetectorConfig,
    #[cfg(feature = "onnx")]
    model: tract_onnx::prelude::SimplePlan<
        tract_onnx::prelude::TypedFact,
        Box<dyn tract_onnx::prelude::TypedOp>,
        tract_onnx::prelude::Graph<
            tract_onnx::prelude::TypedFact,
            Box<dyn tract_onnx::prelude::TypedOp>,
        >,
    >,
}

// Model input dimensions. Match the YOLO export the bridge uses.
const MODEL_W: usize = 320;
const MODEL_H: usize = 320;

impl Detector {
    /// Load a YOLO ONNX model from disk.
    ///
    /// Returns `ModelLoad` if the path doesn't exist, the file isn't
    /// valid ONNX, the model fails optimization, or the input shape
    /// can't be fixed to 1×3×320×320.
    #[cfg(feature = "onnx")]
    pub fn from_onnx(path: &str, config: DetectorConfig) -> Result<Self, FaceError> {
        use tract_onnx::prelude::*;

        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| FaceError::ModelLoad(format!("read {}: {}", path, e)))?
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec![1, 3, MODEL_H as i64, MODEL_W as i64],
                ),
            )
            .map_err(|e| FaceError::ModelLoad(format!("input fact: {}", e)))?
            .into_optimized()
            .map_err(|e| FaceError::ModelLoad(format!("optimize: {}", e)))?
            .into_runnable()
            .map_err(|e| FaceError::ModelLoad(format!("runnable: {}", e)))?;

        Ok(Self { config, model })
    }

    /// Stub version when the `onnx` feature is disabled. Useful in test
    /// builds and in CI runs where we don't want to compile the tract
    /// dep tree. Returns a Detector whose `detect()` returns empty.
    #[cfg(not(feature = "onnx"))]
    pub fn from_onnx(_path: &str, config: DetectorConfig) -> Result<Self, FaceError> {
        Ok(Self { config })
    }

    /// Detect faces in an RGB image.
    ///
    /// `rgb` must be tightly-packed RGB888 bytes with stride = width*3.
    /// Returns one Detection per face with confidence ≥ threshold,
    /// after non-max suppression. Empty vec when no faces are found.
    ///
    /// Coordinates returned are in source-image pixel space — already
    /// scaled back from the 320×320 model input.
    #[cfg(feature = "onnx")]
    pub fn detect(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<Detection>, FaceError> {
        use image::{imageops::FilterType, ImageBuffer, Rgb};
        use ndarray::Array;
        use tract_onnx::prelude::*;

        // Validate input
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(3))
            .ok_or(FaceError::InvalidInput("dimensions overflow"))?;
        if rgb.len() < expected {
            return Err(FaceError::InvalidInput(
                "rgb buffer smaller than width*height*3",
            ));
        }
        if width < 32 || height < 32 {
            return Err(FaceError::InvalidInput("image smaller than 32x32"));
        }

        // 1. Wrap source bytes in an ImageBuffer view (no allocation).
        //    Have to copy because resize() needs owned storage; the
        //    upstream &[u8] could become invalid mid-resize otherwise.
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_raw(width, height, rgb.to_vec())
                .ok_or(FaceError::InvalidInput("ImageBuffer build failed"))?;

        // 2. Resize to model input. Nearest is fast enough at this scale
        //    and matches the bridge's choice — keeps behavior consistent.
        let resized = image::imageops::resize(
            &img, MODEL_W as u32, MODEL_H as u32, FilterType::Nearest,
        );

        // 3. Build CHW float tensor normalized to [0, 1]. Three
        //    contiguous passes — cache-sequential on Cortex-A.
        let total = MODEL_H * MODEL_W;
        let raw = resized.as_raw();
        let mut tensor_data = vec![0f32; 3 * total];
        for i in 0..total {
            tensor_data[i]             = raw[i * 3]     as f32 * (1.0 / 255.0);
            tensor_data[total + i]     = raw[i * 3 + 1] as f32 * (1.0 / 255.0);
            tensor_data[2 * total + i] = raw[i * 3 + 2] as f32 * (1.0 / 255.0);
        }

        // 4. ndarray → tract Tensor.
        let input: Tensor = Array::from_shape_vec(
            [1usize, 3, MODEL_H, MODEL_W],
            tensor_data,
        )
        .map_err(|e| FaceError::Inference(format!("shape: {}", e)))?
        .into_dyn()
        .into_tensor();

        // 5. Run inference.
        let outputs = self.model
            .run(tvec![input.into()])
            .map_err(|e| FaceError::Inference(format!("run: {}", e)))?;

        // 6. Parse + rescale bboxes.
        let output_view = outputs[0]
            .to_array_view::<f32>()
            .map_err(|e| FaceError::Inference(format!("output view: {}", e)))?;
        let scale_x = width as f32 / MODEL_W as f32;
        let scale_y = height as f32 / MODEL_H as f32;

        let raw_detections = parse_output(
            output_view, scale_x, scale_y, self.config.conf_threshold,
        );
        Ok(nms(raw_detections, self.config.iou_threshold))
    }

    /// Stub version when onnx feature is off — returns empty.
    #[cfg(not(feature = "onnx"))]
    pub fn detect(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<Detection>, FaceError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(3))
            .ok_or(FaceError::InvalidInput("dimensions overflow"))?;
        if rgb.len() < expected {
            return Err(FaceError::InvalidInput(
                "rgb buffer smaller than width*height*3",
            ));
        }
        if width < 32 || height < 32 {
            return Err(FaceError::InvalidInput("image smaller than 32x32"));
        }
        Ok(Vec::new())
    }

    /// Current config.
    pub fn config(&self) -> DetectorConfig { self.config }
}

/// Parse a YOLO output tensor into a list of (unfiltered) Detections.
///
/// Handles both common YOLO output layouts:
///   - Layout A: shape [1, N_anchors, ≥5] (anchor-major)
///   - Layout B: shape [1, ≥5, N_anchors] (feature-major; e.g. yolov8)
///
/// Auto-detects which is in use by comparing dim1 vs dim2.
#[cfg(feature = "onnx")]
fn parse_output(
    tensor: ndarray::ArrayView<f32, ndarray::IxDyn>,
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
                tensor[[0, i, 0]], tensor[[0, i, 1]],
                tensor[[0, i, 2]], tensor[[0, i, 3]],
                conf, sx, sy,
            ));
        }
    } else if use_layout_b {
        // Layout B: [1, ≥5, N_anchors]
        for j in 0..dim2 {
            let conf = tensor[[0, 4, j]];
            if conf < threshold { continue; }
            detections.push(bbox_to_detection(
                tensor[[0, 0, j]], tensor[[0, 1, j]],
                tensor[[0, 2, j]], tensor[[0, 3, j]],
                conf, sx, sy,
            ));
        }
    }

    detections
}

/// Convert (center_x, center_y, width, height) → Detection bbox.
/// Coordinates scaled from model space to source-image space.
#[cfg(feature = "onnx")]
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

/// Non-maximum suppression: filter overlapping detections.
///
/// Pure function exposed so callers can run NMS on detections from
/// multiple sources (e.g. merging detection-from-bridge with
/// detection-from-still-capture).
pub fn nms(mut dets: Vec<Detection>, iou_threshold: f32) -> Vec<Detection> {
    // Sort by confidence descending.
    dets.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence)
                       .unwrap_or(std::cmp::Ordering::Equal));
    let mut keep: Vec<Detection> = Vec::with_capacity(dets.len());
    for d in dets {
        let collides = keep.iter().any(|k| iou(&d, k) > iou_threshold);
        if !collides {
            keep.push(d);
        }
    }
    keep
}

fn iou(a: &Detection, b: &Detection) -> f32 {
    let ix0 = a.x_min.max(b.x_min);
    let iy0 = a.y_min.max(b.y_min);
    let ix1 = a.x_max.min(b.x_max);
    let iy1 = a.y_max.min(b.y_max);
    let iw = (ix1 - ix0).max(0.0);
    let ih = (iy1 - iy0).max(0.0);
    let inter = iw * ih;
    let ua = a.area() + b.area() - inter;
    if ua <= 0.0 { 0.0 } else { inter / ua }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(x: f32, y: f32, w: f32, h: f32, c: f32) -> Detection {
        Detection {
            x_min: x, y_min: y,
            x_max: x + w, y_max: y + h,
            confidence: c,
        }
    }

    #[test]
    fn nms_keeps_highest_confidence() {
        let dets = vec![
            det(10.0, 10.0, 100.0, 100.0, 0.9),
            det(20.0, 20.0, 100.0, 100.0, 0.8),  // overlaps with first
            det(200.0, 200.0, 50.0, 50.0, 0.7),  // far away
        ];
        let kept = nms(dets, 0.5);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].confidence, 0.9);
        assert_eq!(kept[1].confidence, 0.7);
    }

    #[test]
    fn nms_passes_through_no_overlap() {
        let dets = vec![
            det(0.0, 0.0, 10.0, 10.0, 0.9),
            det(100.0, 100.0, 10.0, 10.0, 0.8),
            det(200.0, 200.0, 10.0, 10.0, 0.7),
        ];
        assert_eq!(nms(dets, 0.5).len(), 3);
    }

    #[test]
    fn detection_geometry_helpers() {
        let d = det(10.0, 20.0, 30.0, 40.0, 0.8);
        assert_eq!(d.width(), 30.0);
        assert_eq!(d.height(), 40.0);
        assert_eq!(d.area(), 1200.0);
        assert_eq!(d.centre(), (25.0, 40.0));
    }

    #[test]
    fn detect_rejects_too_small_input() {
        let det = Detector::from_onnx("nonexistent", DetectorConfig::default()).unwrap();
        let buf = vec![0u8; 32 * 32 * 3];
        assert!(matches!(
            det.detect(&buf, 16, 16),
            Err(FaceError::InvalidInput(_))
        ));
    }
}
