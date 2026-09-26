//! `V4l2Api` over a real `/dev/videoN` fd.
//!
//! **Compiled, never executed.** There is no V4L2 device in the environment
//! this was written in, so nothing here has been run against a driver — not
//! even `vicodec`. The struct layouts and ioctl numbers it depends on *are*
//! verified (see `raw.rs`), which removes the largest class of silent failure,
//! but the sequencing of `mmap`/`EXPBUF`/`QBUF` against a live driver is
//! untested. Treat first-run failures as expected.
//!
//! # Portability note that will bite on postmarketOS
//!
//! `libc::ioctl`'s request parameter is `c_ulong` on glibc and `c_int` on musl.
//! postmarketOS is Alpine-based, so the Fairphone target is musl. Every call
//! site here casts with `as _` and lets inference pick, which compiles on both.
//! Writing `as libc::c_ulong` explicitly is the version of this that builds on
//! your laptop and fails on the phone.

use crate::api::{ApiError, ApiResult, CaptureFormat, DequeuedCapture, Event, Queue, V4l2Api};
use crate::raw::*;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;
use vidkit_core::frame::{DmaBufImage, DmaBufPlane, FrameData};

const OUTPUT_BUFFER_COUNT: u32 = 8;

/// A memory-mapped OUTPUT buffer we copy bitstream into.
struct MappedBuffer {
    ptr: *mut u8,
    len: usize,
    /// False while the driver holds it.
    free: bool,
}

impl Drop for MappedBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: ptr/len came from a successful mmap and are unmapped once.
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.len);
            }
        }
    }
}

/// Per-capture-buffer exported dma-buf fds, one per plane.
///
/// Exported **once** at allocation time, not per frame. `EXPBUF` on every
/// dequeue would work but leaks fds fast at 60 fps and shows up as `EMFILE`
/// twenty minutes into playback.
struct ExportedBuffer {
    fds: Vec<OwnedFd>,
    strides: Vec<u32>,
    offsets: Vec<u32>,
}

pub struct Device {
    fd: OwnedFd,
    output_buffers: Vec<MappedBuffer>,
    capture_buffers: Vec<ExportedBuffer>,
    capture_planes: usize,
    /// Per-plane stride from `G_FMT`.
    ///
    /// `v4l2_plane` has no stride field — the stride lives in the *format*,
    /// not the buffer, in `plane_fmt[i].bytesperline`. Passing a zero or
    /// guessed stride to the Vulkan importer produces a diagonally smeared
    /// image, which is a memorable way to learn this.
    capture_strides: Vec<u32>,
    format: Option<CaptureFormat>,
    /// Sequence counter used as the buffer timestamp, because `struct timeval`
    /// is what V4L2 offers and we want monotonic PTS passthrough.
    modifier: u64,
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// SAFETY contract for callers: `arg` must point at a correctly-typed,
/// correctly-sized struct for `request`. That is what `raw.rs` guarantees.
unsafe fn xioctl<T>(fd: RawFd, request: u32, arg: *mut T, name: &'static str) -> ApiResult<()> {
    loop {
        let r = libc::ioctl(fd, request as _, arg);
        if r >= 0 {
            return Ok(());
        }
        match errno() {
            // Signal during the call: retry, do not surface.
            libc::EINTR => continue,
            libc::EAGAIN => return Err(ApiError::WouldBlock),
            libc::EPIPE => return Err(ApiError::Drained),
            e => return Err(ApiError::Ioctl(name, e)),
        }
    }
}

fn buf_type(q: Queue) -> u32 {
    match q {
        Queue::Output => V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
        Queue::Capture => V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
    }
}

impl Device {
    /// Open a decoder node and subscribe to the events the state machine needs.
    ///
    /// Subscribing to `SOURCE_CHANGE` **before** streaming is mandatory: the
    /// event fires as soon as headers are parsed, and a subscription added
    /// afterwards misses it, leaving the decoder waiting forever for something
    /// that already happened.
    pub fn open(path: &str) -> ApiResult<Self> {
        let c = std::ffi::CString::new(path).map_err(|_| ApiError::Unsupported("bad path"))?;
        // SAFETY: c is a valid NUL-terminated string for the duration.
        let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
        if raw < 0 {
            return Err(ApiError::Ioctl("open", errno()));
        }
        // SAFETY: raw is a fresh, owned, valid fd.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let dev = Self {
            fd,
            output_buffers: Vec::new(),
            capture_buffers: Vec::new(),
            capture_planes: 0,
            capture_strides: Vec::new(),
            format: None,
            modifier: 0,
        };

        for (ty, id) in [(V4L2_EVENT_SOURCE_CHANGE, 0u32), (V4L2_EVENT_EOS, 0u32)] {
            let mut sub = v4l2_event_subscription {
                type_: ty,
                id,
                ..Default::default()
            };
            // SAFETY: correct struct for VIDIOC_SUBSCRIBE_EVENT.
            unsafe {
                xioctl(
                    dev.fd.as_raw_fd(),
                    VIDIOC_SUBSCRIBE_EVENT,
                    &mut sub,
                    "SUBSCRIBE_EVENT",
                )?
            };
        }
        Ok(dev)
    }

    /// The DRM format modifier of the capture buffers.
    ///
    /// Not obtainable from V4L2 itself. Real drivers either always produce
    /// linear (Venus with the default `V4L2_PIX_FMT_NV12`) or expose a
    /// vendor-specific tiled format such as `V4L2_PIX_FMT_QC08C`, which maps to
    /// a Qualcomm compressed modifier. Defaulting to linear is safe only for
    /// the former — see the warning in `vidkit-vk`.
    pub fn set_modifier(&mut self, modifier: u64) {
        self.modifier = modifier;
    }

    fn free_output_buffers(&mut self) {
        self.output_buffers.clear();
    }

    fn map_output_buffers(&mut self, count: u32) -> ApiResult<()> {
        self.free_output_buffers();
        for index in 0..count {
            let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
            let mut buf = v4l2_buffer {
                index,
                type_: V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
                memory: V4L2_MEMORY_MMAP,
                length: 1,
                m: v4l2_buffer_m {
                    planes: planes.as_mut_ptr(),
                },
                ..Default::default()
            };
            // SAFETY: `planes` outlives the call; buf is correctly typed.
            unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_QUERYBUF, &mut buf, "QUERYBUF")? };

            let len = planes[0].length as usize;
            // SAFETY: MMAP memory type means the mem_offset arm is the live one.
            let offset = unsafe { planes[0].m.mem_offset } as libc::off_t;
            // SAFETY: len/offset come straight from QUERYBUF for this fd.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    self.fd.as_raw_fd(),
                    offset,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(ApiError::Ioctl("mmap", errno()));
            }
            self.output_buffers.push(MappedBuffer {
                ptr: ptr as *mut u8,
                len,
                free: true,
            });
        }
        Ok(())
    }

    fn export_capture_buffers(&mut self, count: u32, planes_per_buf: usize) -> ApiResult<()> {
        if self.capture_strides.len() < planes_per_buf
            || self.capture_strides.iter().take(planes_per_buf).any(|&s| s == 0)
        {
            // Refuse rather than export buffers we cannot describe. The state
            // machine always calls G_FMT before REQBUFS(CAPTURE), so reaching
            // here means that ordering was violated.
            return Err(ApiError::Unsupported(
                "capture strides unknown; call G_FMT before REQBUFS(CAPTURE)",
            ));
        }
        self.capture_buffers.clear();
        self.capture_planes = planes_per_buf;

        for index in 0..count {
            // QUERYBUF first, for the per-plane strides the importer needs.
            let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
            let mut buf = v4l2_buffer {
                index,
                type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                memory: V4L2_MEMORY_MMAP,
                length: planes_per_buf as u32,
                m: v4l2_buffer_m {
                    planes: planes.as_mut_ptr(),
                },
                ..Default::default()
            };
            // SAFETY: as above.
            unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_QUERYBUF, &mut buf, "QUERYBUF")? };

            let mut fds = Vec::with_capacity(planes_per_buf);
            let mut strides = Vec::with_capacity(planes_per_buf);
            let mut offsets = Vec::with_capacity(planes_per_buf);

            for p in 0..planes_per_buf {
                let mut exp = v4l2_exportbuffer {
                    type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                    index,
                    plane: p as u32,
                    // O_CLOEXEC matters: these fds outlive any exec in a
                    // sandboxed player and leak into helper processes without it.
                    flags: libc::O_CLOEXEC as u32,
                    fd: -1,
                    ..Default::default()
                };
                // SAFETY: correct struct for VIDIOC_EXPBUF.
                unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_EXPBUF, &mut exp, "EXPBUF")? };
                // SAFETY: EXPBUF returned a fresh owned fd.
                fds.push(unsafe { OwnedFd::from_raw_fd(exp.fd) });
                // Stride from the format (captured by the preceding G_FMT),
                // offset from the buffer.
                strides.push(self.capture_strides.get(p).copied().unwrap_or(0));
                offsets.push(planes[p].data_offset);
            }

            self.capture_buffers.push(ExportedBuffer {
                fds,
                strides,
                offsets,
            });
        }
        Ok(())
    }
}

impl V4l2Api for Device {
    fn set_output_format(&mut self, fourcc: u32, width: u32, height: u32) -> ApiResult<()> {
        let mut fmt = v4l2_format::new_mplane(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
        // SAFETY: the union was zeroed by new_mplane; writing the pix_mp arm is
        // the documented use.
        unsafe {
            fmt.fmt.pix_mp.width = width;
            fmt.fmt.pix_mp.height = height;
            fmt.fmt.pix_mp.pixelformat = fourcc;
            fmt.fmt.pix_mp.num_planes = 1;
            // Bitstream buffer size. Drivers clamp to their own minimum, but
            // passing 0 makes some of them allocate nothing and then fail QBUF
            // with EINVAL, which is an unhelpful place to discover it.
            fmt.fmt.pix_mp.plane_fmt[0].sizeimage = (width * height).max(1024 * 1024);
        }
        // SAFETY: correct struct for VIDIOC_S_FMT.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_S_FMT, &mut fmt, "S_FMT") }
    }

    fn request_buffers(&mut self, queue: Queue, count: u32) -> ApiResult<u32> {
        let mut req = v4l2_requestbuffers {
            count: if queue == Queue::Output {
                OUTPUT_BUFFER_COUNT.min(count.max(1))
            } else {
                count
            },
            type_: buf_type(queue),
            memory: V4L2_MEMORY_MMAP,
            ..Default::default()
        };
        // SAFETY: correct struct for VIDIOC_REQBUFS.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_REQBUFS, &mut req, "REQBUFS")? };

        match queue {
            Queue::Output => {
                if req.count == 0 {
                    self.free_output_buffers();
                } else {
                    self.map_output_buffers(req.count)?;
                }
            }
            Queue::Capture => {
                if req.count == 0 {
                    // Dropping the ExportedBuffers closes the dma-buf fds. Any
                    // Vulkan images built from them stay valid — the importer
                    // dup'd or the memory holds its own reference — but nothing
                    // new can be imported from them.
                    self.capture_buffers.clear();
                } else {
                    let planes = self.capture_planes.max(1);
                    self.export_capture_buffers(req.count, planes)?;
                }
            }
        }
        Ok(req.count)
    }

    fn stream_on(&mut self, queue: Queue) -> ApiResult<()> {
        let mut ty = buf_type(queue) as i32;
        // SAFETY: STREAMON takes an int.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_STREAMON, &mut ty, "STREAMON") }
    }

    fn stream_off(&mut self, queue: Queue) -> ApiResult<()> {
        let mut ty = buf_type(queue) as i32;
        // SAFETY: STREAMOFF takes an int.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_STREAMOFF, &mut ty, "STREAMOFF")? };
        if queue == Queue::Output {
            for b in &mut self.output_buffers {
                b.free = true;
            }
        }
        Ok(())
    }

    fn capture_format(&mut self) -> ApiResult<CaptureFormat> {
        let mut fmt = v4l2_format::new_mplane(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
        // SAFETY: correct struct for VIDIOC_G_FMT.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_G_FMT, &mut fmt, "G_FMT")? };
        // SAFETY: the driver filled the pix_mp arm for an mplane buffer type.
        // Fields are copied out individually because the struct is packed and
        // taking a reference to a packed field is not allowed.
        let (width, height, fourcc, num_planes) = unsafe {
            (
                fmt.fmt.pix_mp.width,
                fmt.fmt.pix_mp.height,
                fmt.fmt.pix_mp.pixelformat,
                fmt.fmt.pix_mp.num_planes as usize,
            )
        };
        self.capture_planes = num_planes.max(1);
        self.capture_strides.clear();
        for i in 0..self.capture_planes.min(VIDEO_MAX_PLANES) {
            // SAFETY: same union arm as above; copied out by value because the
            // struct is packed and references to packed fields are not allowed.
            let bpl = unsafe { fmt.fmt.pix_mp.plane_fmt[i].bytesperline };
            self.capture_strides.push(bpl);
        }
        let f = CaptureFormat {
            width,
            height,
            fourcc,
            modifier: self.modifier,
        };
        self.format = Some(f);
        Ok(f)
    }

    fn min_capture_buffers(&mut self) -> ApiResult<u32> {
        let mut ctrl = v4l2_control {
            id: V4L2_CID_MIN_BUFFERS_FOR_CAPTURE,
            value: 0,
        };
        // SAFETY: correct struct for VIDIOC_G_CTRL.
        match unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_G_CTRL, &mut ctrl, "G_CTRL") } {
            Ok(()) => Ok(ctrl.value.max(1) as u32),
            // Not every driver implements the control. Falling back to a fixed
            // guess is wrong in the dangerous direction (too few buffers
            // deadlocks), so guess generously.
            Err(ApiError::Ioctl(_, libc::EINVAL)) => Ok(8),
            Err(e) => Err(e),
        }
    }

    fn queue_output(&mut self, data: &[u8], pts: Option<Duration>) -> ApiResult<bool> {
        let index = match self.output_buffers.iter().position(|b| b.free) {
            Some(i) => i,
            None => return Ok(false),
        };
        {
            let b = &self.output_buffers[index];
            if data.len() > b.len {
                return Err(ApiError::Unsupported("packet larger than output buffer"));
            }
            // SAFETY: b.ptr is a live mapping of b.len bytes and data.len() fits.
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), b.ptr, data.len());
            }
        }

        let ts = pts.unwrap_or(Duration::ZERO);
        let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
        planes[0].bytesused = data.len() as u32;
        planes[0].length = self.output_buffers[index].len as u32;

        let mut buf = v4l2_buffer {
            index: index as u32,
            type_: V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
            memory: V4L2_MEMORY_MMAP,
            length: 1,
            // The timestamp round-trips to the matching CAPTURE buffer, which
            // is how PTS survives decode reordering. It is not a wall clock,
            // and drivers that "help" by rewriting it break A/V sync.
            timestamp: timeval {
                tv_sec: ts.as_secs() as i64,
                tv_usec: ts.subsec_micros() as i64,
            },
            m: v4l2_buffer_m {
                planes: planes.as_mut_ptr(),
            },
            ..Default::default()
        };
        // SAFETY: planes outlives the call.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_QBUF, &mut buf, "QBUF(output)")? };
        self.output_buffers[index].free = false;
        Ok(true)
    }

    fn dequeue_output(&mut self) -> ApiResult<bool> {
        let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
        let mut buf = v4l2_buffer {
            type_: V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
            memory: V4L2_MEMORY_MMAP,
            length: 1,
            m: v4l2_buffer_m {
                planes: planes.as_mut_ptr(),
            },
            ..Default::default()
        };
        // SAFETY: planes outlives the call.
        match unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_DQBUF, &mut buf, "DQBUF(output)") } {
            Ok(()) => {
                if let Some(b) = self.output_buffers.get_mut(buf.index as usize) {
                    b.free = true;
                }
                Ok(true)
            }
            Err(ApiError::WouldBlock) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn queue_capture(&mut self, index: u32) -> ApiResult<()> {
        let n = self.capture_planes.max(1);
        let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
        let mut buf = v4l2_buffer {
            index,
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            memory: V4L2_MEMORY_MMAP,
            length: n as u32,
            m: v4l2_buffer_m {
                planes: planes.as_mut_ptr(),
            },
            ..Default::default()
        };
        // SAFETY: planes outlives the call.
        unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_QBUF, &mut buf, "QBUF(capture)") }
    }

    fn dequeue_capture(&mut self) -> ApiResult<Option<DequeuedCapture>> {
        let n = self.capture_planes.max(1);
        let mut planes = [v4l2_plane::default(); VIDEO_MAX_PLANES];
        let mut buf = v4l2_buffer {
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            memory: V4L2_MEMORY_MMAP,
            length: n as u32,
            m: v4l2_buffer_m {
                planes: planes.as_mut_ptr(),
            },
            ..Default::default()
        };
        // SAFETY: planes outlives the call.
        match unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_DQBUF, &mut buf, "DQBUF(capture)") } {
            Ok(()) => {}
            Err(ApiError::WouldBlock) => return Ok(None),
            // EPIPE after a drain: the driver is telling us there is nothing
            // more, which the LAST flag has usually already said.
            Err(ApiError::Drained) => return Ok(None),
            Err(e) => return Err(e),
        }

        let exported = self
            .capture_buffers
            .get(buf.index as usize)
            .ok_or(ApiError::Unsupported("dequeued an unknown capture index"))?;

        let mut dma_planes = Vec::with_capacity(exported.fds.len());
        for (i, fd) in exported.fds.iter().enumerate() {
            // dup because the Frame owns its fds and may outlive the pool entry
            // if the caller holds it across a reconfiguration. The original
            // stays with the ExportedBuffer for the next dequeue.
            let dup = fd
                .try_clone()
                .map_err(|e| ApiError::Ioctl("dup", e.raw_os_error().unwrap_or(0)))?;
            dma_planes.push(DmaBufPlane {
                fd: dup,
                offset: exported.offsets.get(i).copied().unwrap_or(0),
                stride: exported.strides.get(i).copied().unwrap_or(0),
            });
        }

        let fourcc = self.format.map(|f| f.fourcc).unwrap_or(0);
        Ok(Some(DequeuedCapture {
            index: buf.index,
            data: FrameData::DmaBuf(DmaBufImage {
                planes: dma_planes,
                modifier: self.modifier,
                fourcc,
                owner: None,
            }),
            pts: Duration::new(
                buf.timestamp.tv_sec.max(0) as u64,
                (buf.timestamp.tv_usec.max(0) as u32) * 1000,
            ),
            last: buf.flags & V4L2_BUF_FLAG_LAST != 0,
            error: buf.flags & V4L2_BUF_FLAG_ERROR != 0,
        }))
    }

    fn dequeue_event(&mut self) -> ApiResult<Option<Event>> {
        let mut ev = v4l2_event::default();
        // SAFETY: correct struct for VIDIOC_DQEVENT.
        match unsafe { xioctl(self.fd.as_raw_fd(), VIDIOC_DQEVENT, &mut ev, "DQEVENT") } {
            Ok(()) => {}
            Err(ApiError::WouldBlock) => return Ok(None),
            // No events pending is reported as ENOENT, not EAGAIN, by some
            // drivers. Both mean "nothing to do".
            Err(ApiError::Ioctl(_, libc::ENOENT)) => return Ok(None),
            Err(e) => return Err(e),
        }

        Ok(match ev.type_ {
            V4L2_EVENT_SOURCE_CHANGE => {
                // SAFETY: the src_change arm is the one the kernel filled for
                // this event type.
                let changes = unsafe { ev.u.src_change.changes };
                // Resolution is the only change we act on. Others exist and
                // must not be mistaken for it.
                if changes & V4L2_EVENT_SRC_CH_RESOLUTION != 0 {
                    Some(Event::SourceChange)
                } else {
                    None
                }
            }
            V4L2_EVENT_EOS => Some(Event::Eos),
            _ => None,
        })
    }

    fn decoder_cmd_stop(&mut self) -> ApiResult<()> {
        let mut cmd = v4l2_decoder_cmd::new(V4L2_DEC_CMD_STOP);
        // SAFETY: correct struct for VIDIOC_DECODER_CMD.
        unsafe {
            xioctl(
                self.fd.as_raw_fd(),
                VIDIOC_DECODER_CMD,
                &mut cmd,
                "DECODER_CMD(stop)",
            )
        }
    }

    fn decoder_cmd_start(&mut self) -> ApiResult<()> {
        let mut cmd = v4l2_decoder_cmd::new(V4L2_DEC_CMD_START);
        // SAFETY: correct struct for VIDIOC_DECODER_CMD.
        unsafe {
            xioctl(
                self.fd.as_raw_fd(),
                VIDIOC_DECODER_CMD,
                &mut cmd,
                "DECODER_CMD(start)",
            )
        }
    }
}

// SAFETY: Device owns its fd and mappings exclusively; every method takes
// `&mut self`, so there is no shared access to the raw pointers. It is not
// `Sync` and must not be made so.
unsafe impl Send for Device {}
