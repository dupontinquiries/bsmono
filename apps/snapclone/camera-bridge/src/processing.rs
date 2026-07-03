//! Frame crop + resize pipeline.

use anyhow::Result;
use image::{imageops::FilterType, ImageBuffer, Rgb};

use crate::capture::Frame;
use crate::tracking::CropRect;

/// Output dimensions for the virtual camera.
pub struct FrameConfig {
    pub output_width:  u32,
    pub output_height: u32,
}

/// Crop `frame` to `crop` and resize to the output dimensions.
///
/// Takes `frame` by value to avoid cloning the pixel data — the caller
/// no longer needs the frame after this call.
pub fn crop_and_resize(frame: Frame, crop: CropRect, config: &FrameConfig) -> Result<Frame> {
    let (fw, fh) = (frame.width, frame.height);

    // Consume frame data directly into an ImageBuffer — zero extra allocation.
    let src: ImageBuffer<Rgb<u8>, _> =
        ImageBuffer::from_raw(fw, fh, frame.data)
            .ok_or_else(|| anyhow::anyhow!("Invalid source frame dimensions"))?;

    // Clamp crop to frame bounds.
    let crop_x = crop.x.min(fw.saturating_sub(1));
    let crop_y = crop.y.min(fh.saturating_sub(1));
    let crop_w = crop.width.min(fw.saturating_sub(crop_x)).max(1);
    let crop_h = crop.height.min(fh.saturating_sub(crop_y)).max(1);

    let cropped = image::imageops::crop_imm(&src, crop_x, crop_y, crop_w, crop_h).to_image();

    // Lanczos3 gives good quality for the face-tracking zoom crop.
    let resized = image::imageops::resize(
        &cropped,
        config.output_width,
        config.output_height,
        FilterType::Lanczos3,
    );

    Ok(Frame {
        data:   resized.into_raw(),
        width:  config.output_width,
        height: config.output_height,
    })
}
