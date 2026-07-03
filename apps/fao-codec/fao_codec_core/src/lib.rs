//! `fao_codec_core` — codec-agnostic primitives shared across all FAO codecs.
//!
//! This crate holds the fundamental building blocks that don't depend on any 
//! particular codec version. Used by `fao_codec_lib` (which contains v2..v8) 
//! and any future codecs.
//!
//! # Modules
//!
//! - [`dct`]: 8x8 forward/inverse DCT
//! - [`quant`]: quantization tables, dead-zone quantizer, RLE bit estimator  
//! - [`color`]: BT.709 RGB ↔ YCbCr conversion
//! - [`frame_tag`]: per-frame metadata flowing through the encode pipeline
//! - [`bitio`]: MSB-first bit reader/writer
//! - [`chroma`]: 4:2:0 chroma resampling
//! - [`f16`]: IEEE 754 half-precision float conversion
//! - [`pixel_filters`]: per-block pre-filters (denoise, quantize)
//! - [`bitpack`]: variable-length packing for block IDs and motion vectors
//! - [`region_activity`]: change-detection between consecutive frames
//!
//! # Design
//!
//! All items here are pure CPU-side primitives. No GPU code, no codec 
//! state, no platform-specific dependencies. This makes them safe to 
//! reuse anywhere — from CPU encoders to GPU shaders to standalone tools.
//!
//! Consumers can use the crate-level re-exports for common operations:
//!
//! ```ignore
//! use fao_core::{dct_fwd, dct_inv, FrameTag, BitWriter};
//! ```
//!
//! Or reach into specific modules for less-common items:
//!
//! ```ignore
//! use fao_core::quant::{est_bits_dpcm, build_qtable_inv};
//! ```

#![allow(unused_variables, unused_mut, dead_code)]

pub mod dct;
pub mod quant;
pub mod color;
pub mod frame_tag;
pub mod bitio;
pub mod chroma;

/// IEEE 754 half-precision float conversion. Useful for compact storage 
/// of values where 3-4 decimal digits suffice (depth, confidence, etc).
pub mod f16;

/// Pixel pre-filters (denoise, quantize, edge-preserve) applied per-block 
/// during encode to reduce residuals on smooth/UI content.
pub mod pixel_filters;

/// Variable-length packing helpers for block IDs, motion vectors, etc.
/// Generic over the data shape — used by v2 and v3 codecs.
pub mod bitpack;

/// Region activity tracking — detects which screen regions have changed 
/// between consecutive frames. Used to gate encode work to active regions.
pub mod region_activity;

// Convenient crate-root re-exports
pub use dct::{init as init_dct, dct_fwd, dct_inv};
pub use quant::{LUMA_Q, CHROMA_Q, ZIGZAG, build_qtable, build_qtable_inv, dzq, dzq_fast, est_bits_dpcm};
pub use color::{rgb_to_planes, planes_to_rgb};
pub use frame_tag::FrameTag;
pub use bitio::{BitWriter, BitReader};
pub use chroma::{chroma_downsample_420, chroma_upsample_420};
pub use f16::{f32_to_f16_bits, f16_bits_to_f32};
