//! AV1 film grain parameters from a frame's side data.
//!
//! In process there is no second demux to parse headers from: libdav1d (or
//! the native decoder) is told AV_CODEC_EXPORT_DATA_FILM_GRAIN, skips
//! synthesis, and attaches the parameters to each frame. They are the
//! bitstream's, already resolved (update_grain / reference loading done).
//!
//! libavutil/film_grain_params.h is not in ffmpeg-sys-next's bindings, so
//! the two structs are declared here. Only the prefix is read -- `type`,
//! `seed`, the AOM union member -- which has been layout-stable since
//! FFmpeg 4.4 (later majors append fields after the union). The oracle test
//! (exported + ours == libdav1d's own synthesis, byte for byte) is what
//! would catch a layout mistake.

use vidkit_core::av1_grain::GrainParams;

#[repr(C)]
#[allow(non_snake_case)]
pub struct AVFilmGrainAOMParams {
    pub num_y_points: i32,
    pub y_points: [[u8; 2]; 14],
    pub chroma_scaling_from_luma: i32,
    pub num_uv_points: [i32; 2],
    pub uv_points: [[[u8; 2]; 10]; 2],
    pub scaling_shift: i32,
    pub ar_coeff_lag: i32,
    pub ar_coeffs_y: [i8; 24],
    pub ar_coeffs_uv: [[i8; 25]; 2],
    pub ar_coeff_shift: i32,
    pub grain_scale_shift: i32,
    pub uv_mult: [i32; 2],
    pub uv_mult_luma: [i32; 2],
    pub uv_offset: [i32; 2],
    pub overlap_flag: i32,
    pub limit_output_range: i32,
}

/// The prefix of AVFilmGrainParams.
#[repr(C)]
pub struct AVFilmGrainParamsPrefix {
    /// AV_FILM_GRAIN_PARAMS_AV1 = 1.
    pub type_: i32,
    pub seed: u64,
    pub aom: AVFilmGrainAOMParams,
}

pub const AV_FILM_GRAIN_PARAMS_AV1: i32 = 1;

/// Ours from libavutil's. Pure.
pub fn from_aom(seed: u64, a: &AVFilmGrainAOMParams) -> GrainParams {
    let n = |k: i32, max: usize| (k.max(0) as usize).min(max);
    GrainParams {
        seed: seed as u16,
        y_points: a.y_points[..n(a.num_y_points, 14)].to_vec(),
        cb_points: a.uv_points[0][..n(a.num_uv_points[0], 10)].to_vec(),
        cr_points: a.uv_points[1][..n(a.num_uv_points[1], 10)].to_vec(),
        chroma_scaling_from_luma: a.chroma_scaling_from_luma != 0,
        scaling_shift: a.scaling_shift as u8,
        ar_coeff_lag: a.ar_coeff_lag as u8,
        ar_coeffs_y: a.ar_coeffs_y,
        ar_coeffs_uv: a.ar_coeffs_uv,
        ar_coeff_shift: a.ar_coeff_shift as u8,
        grain_scale_shift: a.grain_scale_shift as u8,
        uv_mult: a.uv_mult,
        uv_luma_mult: a.uv_mult_luma,
        uv_offset: a.uv_offset,
        overlap: a.overlap_flag != 0,
        clip_to_restricted_range: a.limit_output_range != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Offsets as the C compiler lays them out (x86-64 / aarch64 LP64).
    #[test]
    fn layout_matches_the_header() {
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, y_points), 4);
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, chroma_scaling_from_luma), 32);
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, uv_points), 44);
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, scaling_shift), 84);
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, ar_coeffs_y), 92);
        assert_eq!(std::mem::offset_of!(AVFilmGrainAOMParams, ar_coeff_shift), 168);
        assert_eq!(std::mem::offset_of!(AVFilmGrainParamsPrefix, aom), 16);
    }
}
