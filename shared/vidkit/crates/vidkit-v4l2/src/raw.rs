//! Raw V4L2 uAPI: structs, ioctl numbers, constants.
//!
//! Transcribed from `linux/videodev2.h`. Every size, alignment and field offset
//! in here is asserted against numbers extracted from the real header by a C
//! program — see the tests at the bottom. Those assertions are the only thing
//! standing between a transcription typo and a decoder that silently reads the
//! wrong field, so do not delete them when they get in the way.
//!
//! # Two traps in this file
//!
//! **`v4l2_pix_format_mplane` and `v4l2_plane_pix_format` are
//! `__attribute__((packed))`.** They must be `#[repr(C, packed)]`, not plain
//! `repr(C)`. Getting this wrong shifts `num_planes` by three bytes and the
//! failure looks like a driver bug rather than a layout bug.
//!
//! **`v4l2_buffer` embeds `struct timeval`, so its layout is
//! userspace-ABI-dependent, not just architecture-dependent.** On 64-bit it is
//! 88 bytes (verified here). On 32-bit ARM the answer depends on whether the
//! libc uses 32- or 64-bit `time_t` — musl switched, glibc has both — and the
//! kernel's compat path keys off the *ioctl number*, which encodes the struct
//! size. If you ever target armv7 postmarketOS rather than aarch64, re-run the
//! extraction for that target and expect these constants to differ. The
//! aarch64 Fairphone case is what is verified.

#![allow(non_camel_case_types)]

// ---------------------------------------------------------------------------
// ioctl encoding
// ---------------------------------------------------------------------------

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

/// Generic `_IOC`. Correct for x86, arm, arm64, riscv. **Not** correct for
/// mips, powerpc, sparc or alpha, which use a different direction encoding —
/// if this ever needs to run there, the constants above change.
const fn ioc(dir: u32, ty: u32, nr: u32, size: u32) -> u32 {
    (dir << IOC_DIRSHIFT) | (ty << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

const fn iow(nr: u32, size: u32) -> u32 {
    ioc(IOC_WRITE, b'V' as u32, nr, size)
}
const fn ior(nr: u32, size: u32) -> u32 {
    ioc(IOC_READ, b'V' as u32, nr, size)
}
const fn iowr(nr: u32, size: u32) -> u32 {
    ioc(IOC_READ | IOC_WRITE, b'V' as u32, nr, size)
}

macro_rules! sz {
    ($t:ty) => {
        core::mem::size_of::<$t>() as u32
    };
}

pub const VIDIOC_QUERYCAP: u32 = ior(0, sz!(v4l2_capability));
pub const VIDIOC_ENUM_FMT: u32 = iowr(2, sz!(v4l2_fmtdesc));
pub const VIDIOC_G_FMT: u32 = iowr(4, sz!(v4l2_format));
pub const VIDIOC_S_FMT: u32 = iowr(5, sz!(v4l2_format));
pub const VIDIOC_REQBUFS: u32 = iowr(8, sz!(v4l2_requestbuffers));
pub const VIDIOC_QUERYBUF: u32 = iowr(9, sz!(v4l2_buffer));
pub const VIDIOC_QBUF: u32 = iowr(15, sz!(v4l2_buffer));
pub const VIDIOC_EXPBUF: u32 = iowr(16, sz!(v4l2_exportbuffer));
pub const VIDIOC_DQBUF: u32 = iowr(17, sz!(v4l2_buffer));
pub const VIDIOC_STREAMON: u32 = iow(18, sz!(i32));
pub const VIDIOC_STREAMOFF: u32 = iow(19, sz!(i32));
pub const VIDIOC_G_CTRL: u32 = iowr(27, sz!(v4l2_control));
pub const VIDIOC_ENUM_FRAMESIZES: u32 = iowr(74, sz!(v4l2_frmsizeenum));
pub const VIDIOC_G_SELECTION: u32 = iowr(94, sz!(v4l2_selection));
/// The visible picture inside the (padded) decoded buffer.
pub const V4L2_SEL_TGT_COMPOSE: u32 = 0x0100;
/// Selection takes the single-planar buffer type even for mplane queues.
pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const VIDIOC_DQEVENT: u32 = ior(89, sz!(v4l2_event));
pub const VIDIOC_SUBSCRIBE_EVENT: u32 = iow(90, sz!(v4l2_event_subscription));
pub const VIDIOC_DECODER_CMD: u32 = iowr(96, sz!(v4l2_decoder_cmd));

// ---------------------------------------------------------------------------
// constants
// ---------------------------------------------------------------------------

pub const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
pub const V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE: u32 = 10;

pub const V4L2_MEMORY_MMAP: u32 = 1;
pub const V4L2_MEMORY_DMABUF: u32 = 4;

pub const V4L2_BUF_FLAG_ERROR: u32 = 0x0000_0040;
pub const V4L2_BUF_FLAG_LAST: u32 = 0x0010_0000;

pub const V4L2_EVENT_EOS: u32 = 2;
pub const V4L2_EVENT_SOURCE_CHANGE: u32 = 5;
pub const V4L2_EVENT_SRC_CH_RESOLUTION: u32 = 1;

pub const V4L2_DEC_CMD_START: u32 = 0;
pub const V4L2_DEC_CMD_STOP: u32 = 1;

pub const V4L2_CID_MIN_BUFFERS_FOR_CAPTURE: u32 = 0x0098_0927;

pub const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;
pub const V4L2_FRMSIZE_TYPE_CONTINUOUS: u32 = 2;
pub const V4L2_FRMSIZE_TYPE_STEPWISE: u32 = 3;

pub const VIDEO_MAX_PLANES: usize = 8;

// ---------------------------------------------------------------------------
// structs
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_fmtdesc {
    pub index: u32,
    pub type_: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmsize_discrete {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmsize_stepwise {
    pub min_width: u32,
    pub max_width: u32,
    pub step_width: u32,
    pub min_height: u32,
    pub max_height: u32,
    pub step_height: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_frmsize_union {
    pub discrete: v4l2_frmsize_discrete,
    pub stepwise: v4l2_frmsize_stepwise,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmsizeenum {
    pub index: u32,
    pub pixel_format: u32,
    pub type_: u32,
    pub u: v4l2_frmsize_union,
    pub reserved: [u32; 2],
}

/// `__attribute__((packed))` in the header. See module docs.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct v4l2_plane_pix_format {
    pub sizeimage: u32,
    pub bytesperline: u32,
    pub reserved: [u16; 6],
}

/// `__attribute__((packed))` in the header. See module docs.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct v4l2_pix_format_mplane {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub colorspace: u32,
    pub plane_fmt: [v4l2_plane_pix_format; VIDEO_MAX_PLANES],
    pub num_planes: u8,
    pub flags: u8,
    pub ycbcr_enc: u8,
    pub quantization: u8,
    pub xfer_func: u8,
    pub reserved: [u8; 7],
}

/// The `fmt` union. Aligned to 8 because other members of the real union
/// (`v4l2_sdr_format`, `v4l2_meta_format`) require it; the two we use do not,
/// and omitting the alignment silently shrinks `v4l2_format` below what the
/// ioctl number encodes, which makes every `S_FMT` fail with `ENOTTY`.
#[repr(C, align(8))]
#[derive(Copy, Clone)]
pub union v4l2_format_union {
    pub pix_mp: v4l2_pix_format_mplane,
    pub raw_data: [u8; 200],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_format {
    pub type_: u32,
    _pad: u32,
    pub fmt: v4l2_format_union,
}

impl v4l2_format {
    pub fn new_mplane(buf_type: u32) -> Self {
        Self {
            type_: buf_type,
            _pad: 0,
            fmt: v4l2_format_union {
                raw_data: [0u8; 200],
            },
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_requestbuffers {
    pub count: u32,
    pub type_: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_plane_m {
    pub mem_offset: u32,
    /// `unsigned long` in the header, so pointer-width.
    pub userptr: usize,
    pub fd: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_plane {
    pub bytesused: u32,
    pub length: u32,
    pub m: v4l2_plane_m,
    pub data_offset: u32,
    pub reserved: [u32; 11],
}

impl Default for v4l2_plane {
    fn default() -> Self {
        // SAFETY: all-zero is a valid bit pattern for every member, including
        // the union.
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_timecode {
    pub type_: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_buffer_m {
    pub offset: u32,
    pub userptr: usize,
    pub planes: *mut v4l2_plane,
    pub fd: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_buffer {
    pub index: u32,
    pub type_: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: timeval,
    pub timecode: v4l2_timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: v4l2_buffer_m,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

impl Default for v4l2_buffer {
    fn default() -> Self {
        // SAFETY: zeroed is the documented way to prepare this for QBUF/DQBUF.
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_exportbuffer {
    pub type_: u32,
    pub index: u32,
    pub plane: u32,
    pub flags: u32,
    pub fd: i32,
    pub reserved: [u32; 11],
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_event_src_change {
    pub changes: u32,
}

#[repr(C, align(8))]
#[derive(Copy, Clone)]
pub union v4l2_event_union {
    pub src_change: v4l2_event_src_change,
    pub data: [u8; 64],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_event {
    pub type_: u32,
    _pad: u32,
    pub u: v4l2_event_union,
    pub pending: u32,
    pub sequence: u32,
    pub timestamp: timespec,
    pub id: u32,
    pub reserved: [u32; 8],
}

impl Default for v4l2_event {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_event_subscription {
    pub type_: u32,
    pub id: u32,
    pub flags: u32,
    pub reserved: [u32; 5],
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_decoder_cmd_stop {
    pub pts: u64,
}

#[repr(C, align(8))]
#[derive(Copy, Clone)]
pub union v4l2_decoder_cmd_union {
    pub stop: v4l2_decoder_cmd_stop,
    pub raw: [u32; 16],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_decoder_cmd {
    pub cmd: u32,
    pub flags: u32,
    pub u: v4l2_decoder_cmd_union,
}

impl v4l2_decoder_cmd {
    pub fn new(cmd: u32) -> Self {
        Self {
            cmd,
            flags: 0,
            u: v4l2_decoder_cmd_union { raw: [0u32; 16] },
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_rect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_selection {
    pub type_: u32,
    pub target: u32,
    pub flags: u32,
    pub r: v4l2_rect,
    pub reserved: [u32; 9],
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct v4l2_control {
    pub id: u32,
    pub value: i32,
}

#[cfg(test)]
mod layout {
    use super::*;
    use core::mem::{align_of, size_of};

    /// Every number below was printed by a C program compiled against
    /// `/usr/include/linux/videodev2.h` on x86_64. Re-extract for any target
    /// whose ABI differs (see the module docs on `struct timeval`).
    #[test]
    fn struct_sizes_match_the_kernel_header() {
        assert_eq!(size_of::<v4l2_requestbuffers>(), 20);
        assert_eq!(size_of::<v4l2_plane>(), 64);
        // gcc against linux/videodev2.h: 64, VIDIOC_G_SELECTION 0xc040565e.
        assert_eq!(size_of::<v4l2_selection>(), 64);
        assert_eq!(VIDIOC_G_SELECTION, 0xc040_565e);
        assert_eq!(size_of::<v4l2_buffer>(), 88);
        assert_eq!(size_of::<v4l2_format>(), 208);
        assert_eq!(size_of::<v4l2_pix_format_mplane>(), 192);
        assert_eq!(size_of::<v4l2_plane_pix_format>(), 20);
        assert_eq!(size_of::<v4l2_fmtdesc>(), 64);
        assert_eq!(size_of::<v4l2_frmsizeenum>(), 44);
        assert_eq!(size_of::<v4l2_event>(), 136);
        assert_eq!(size_of::<v4l2_event_subscription>(), 32);
        assert_eq!(size_of::<v4l2_exportbuffer>(), 64);
        assert_eq!(size_of::<v4l2_decoder_cmd>(), 72);
        assert_eq!(size_of::<v4l2_control>(), 8);
        assert_eq!(size_of::<v4l2_timecode>(), 16);
    }

    /// The packed attribute is load-bearing; assert it directly rather than
    /// inferring it from the size, which would also pass if the compiler
    /// happened to pick the same total.
    #[test]
    fn mplane_formats_are_packed() {
        assert_eq!(align_of::<v4l2_pix_format_mplane>(), 1);
        assert_eq!(align_of::<v4l2_plane_pix_format>(), 1);
    }

    #[test]
    fn struct_alignments_match() {
        assert_eq!(align_of::<v4l2_plane>(), 8);
        assert_eq!(align_of::<v4l2_buffer>(), 8);
        assert_eq!(align_of::<v4l2_format>(), 8);
        assert_eq!(align_of::<v4l2_event>(), 8);
        assert_eq!(align_of::<v4l2_decoder_cmd>(), 8);
        assert_eq!(align_of::<v4l2_requestbuffers>(), 4);
    }

    macro_rules! off {
        ($t:ty, $f:ident) => {{
            let u = core::mem::MaybeUninit::<$t>::uninit();
            let base = u.as_ptr() as usize;
            // SAFETY: computing an offset only; the field is never read.
            let f = unsafe { core::ptr::addr_of!((*u.as_ptr()).$f) } as usize;
            f - base
        }};
    }

    #[test]
    fn field_offsets_match_the_kernel_header() {
        assert_eq!(off!(v4l2_buffer, timestamp), 24);
        assert_eq!(off!(v4l2_buffer, timecode), 40);
        assert_eq!(off!(v4l2_buffer, sequence), 56);
        assert_eq!(off!(v4l2_buffer, memory), 60);
        assert_eq!(off!(v4l2_buffer, m), 64);
        assert_eq!(off!(v4l2_buffer, length), 72);
        assert_eq!(off!(v4l2_buffer, reserved2), 76);

        assert_eq!(off!(v4l2_plane, m), 8);
        assert_eq!(off!(v4l2_plane, data_offset), 16);

        assert_eq!(off!(v4l2_pix_format_mplane, plane_fmt), 20);
        assert_eq!(off!(v4l2_pix_format_mplane, num_planes), 180);

        assert_eq!(off!(v4l2_format, fmt), 8);

        assert_eq!(off!(v4l2_frmsizeenum, type_), 8);
        assert_eq!(off!(v4l2_frmsizeenum, u), 12);

        assert_eq!(off!(v4l2_event, u), 8);
        assert_eq!(off!(v4l2_event, pending), 72);
        assert_eq!(off!(v4l2_event, sequence), 76);
        assert_eq!(off!(v4l2_event, timestamp), 80);
        assert_eq!(off!(v4l2_event, id), 96);

        assert_eq!(off!(v4l2_fmtdesc, pixelformat), 44);
        assert_eq!(off!(v4l2_fmtdesc, mbus_code), 48);

        assert_eq!(off!(v4l2_decoder_cmd, u), 8);
        assert_eq!(off!(v4l2_exportbuffer, fd), 16);
    }

    /// Fourccs, checked against the values the kernel header expands to.
    #[test]
    fn pixel_format_fourccs_match() {
        use crate::api::pixfmt;
        assert_eq!(pixfmt::VP9, 0x3039_5056, "V4L2_PIX_FMT_VP9");
        assert_eq!(pixfmt::NV12, 0x3231_564e, "V4L2_PIX_FMT_NV12");
        assert_eq!(pixfmt::AV1_FRAME, 0x4631_5641, "V4L2_PIX_FMT_AV1_FRAME");
    }

    #[test]
    fn control_and_flag_constants_match() {
        assert_eq!(V4L2_CID_MIN_BUFFERS_FOR_CAPTURE, 0x0098_0927);
        assert_eq!(V4L2_BUF_FLAG_LAST, 0x0010_0000);
        assert_eq!(V4L2_BUF_FLAG_ERROR, 0x40);
        assert_eq!(V4L2_EVENT_SOURCE_CHANGE, 5);
        assert_eq!(V4L2_EVENT_SRC_CH_RESOLUTION, 1);
        assert_eq!(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, 9);
        assert_eq!(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 10);
    }

    /// The ioctl request numbers encode the struct size, so these double as a
    /// second, independent check on every size above: get a struct wrong and
    /// the corresponding constant stops matching the kernel's.
    #[test]
    fn ioctl_numbers_match_the_kernel_header() {
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_G_FMT, 0xc0d0_5604);
        assert_eq!(VIDIOC_S_FMT, 0xc0d0_5605);
        assert_eq!(VIDIOC_REQBUFS, 0xc014_5608);
        assert_eq!(VIDIOC_QUERYBUF, 0xc058_5609);
        assert_eq!(VIDIOC_QBUF, 0xc058_560f);
        assert_eq!(VIDIOC_EXPBUF, 0xc040_5610);
        assert_eq!(VIDIOC_DQBUF, 0xc058_5611);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(VIDIOC_STREAMOFF, 0x4004_5613);
        assert_eq!(VIDIOC_G_CTRL, 0xc008_561b);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
        assert_eq!(VIDIOC_DQEVENT, 0x8088_5659);
        assert_eq!(VIDIOC_SUBSCRIBE_EVENT, 0x4020_565a);
        assert_eq!(VIDIOC_DECODER_CMD, 0xc048_5660);
    }
}
