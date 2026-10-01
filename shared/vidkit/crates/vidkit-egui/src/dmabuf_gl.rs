//! DMA-BUF frames into GL without a copy: each plane imported as an
//! EGLImage (EGL_EXT_image_dma_buf_import) and bound to a GL_TEXTURE_2D
//! (GL_OES_EGL_image), sampled by the same shader as uploaded planes.
//!
//! EGL is reached through the libEGL eframe/glutin already loaded (dlopen
//! of the same soname returns it); nothing new is linked. Imports are cached
//! per buffer: decoders cycle a small fixed pool (V4L2 capture buffers, VA
//! surfaces), so after the first lap every frame is a cache hit and costs
//! no EGL call at all -- the texture already aliases the buffer the decoder
//! just wrote.
//!
//! When import is impossible (no extension, an unsupported modifier, a
//! driver that refuses single-plane imports of a multi-plane layout), a
//! LINEAR buffer can still be read through mmap ([`map_linear`]) and
//! uploaded like any CPU frame: correct, just not zero-copy.

use std::ffi::{c_void, CStr};
use std::os::fd::{AsRawFd, BorrowedFd};

use vidkit_core::dmabuf::{PlaneImport, DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR};
use vidkit_core::frame::{CpuPlanes, DmaBufImage};

const EGL_NONE: i32 = 0x3038;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_EXTENSIONS: i32 = 0x3055;
const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;

/// The attribute list for one plane. Pure.
///
/// The modifier is passed whenever it is explicit -- LINEAR included: a
/// driver told nothing may assume its own tiling. INVALID ("implicit") is
/// left out, as the extension requires.
pub fn image_attribs(p: &PlaneImport, fd: i32, modifier: u64) -> Vec<i32> {
    let mut a = vec![
        EGL_WIDTH, p.width as i32,
        EGL_HEIGHT, p.height as i32,
        EGL_LINUX_DRM_FOURCC_EXT, p.format as i32,
        EGL_DMA_BUF_PLANE0_FD_EXT, fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, p.offset as i32,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, p.stride as i32,
    ];
    if modifier != DRM_FORMAT_MOD_INVALID {
        a.extend([EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, modifier as u32 as i32, EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, (modifier >> 32) as u32 as i32]);
    }
    a.push(EGL_NONE);
    a
}

// c_char, not i8: it is u8 on aarch64 Linux (the phone AND the Asahi laptop), i8 on x86.
type GetProc = unsafe extern "C" fn(*const std::ffi::c_char) -> *mut c_void;
type GetCurrentDisplay = unsafe extern "C" fn() -> *mut c_void;
type QueryString = unsafe extern "C" fn(*mut c_void, i32) -> *const std::ffi::c_char;
type GetError = unsafe extern "C" fn() -> i32;
type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, *const i32) -> *mut c_void;
type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
type TargetTexture = unsafe extern "C" fn(u32, *mut c_void);

/// The EGL entry points for importing, bound to the CURRENT display.
pub struct Egl {
    display: *mut c_void,
    create: CreateImage,
    destroy: DestroyImage,
    target: TargetTexture,
    get_error: GetError,
    /// EGL_EXT_image_dma_buf_import_modifiers: explicit modifiers accepted.
    pub modifiers: bool,
}

// The pointers are process-global EGL entry points and a display handle;
// the presenter uses them only on the GL thread.
unsafe impl Send for Egl {}

impl Egl {
    /// # Safety
    /// A GL context made current through EGL must be current on this thread.
    pub unsafe fn load() -> Result<Self, String> {
        let lib = [c"libEGL.so.1", c"libEGL.so"]
            .iter()
            .map(|n| libc::dlopen(n.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL))
            .find(|h| !h.is_null())
            .ok_or("libEGL not loadable")?;
        let sym = |n: &CStr| libc::dlsym(lib, n.as_ptr());
        let (gp, gcd, qs, ge) = (sym(c"eglGetProcAddress"), sym(c"eglGetCurrentDisplay"), sym(c"eglQueryString"), sym(c"eglGetError"));
        if gp.is_null() || gcd.is_null() || qs.is_null() || ge.is_null() {
            return Err("libEGL lacks core entry points".into());
        }
        let gp: GetProc = std::mem::transmute(gp);
        let display = (std::mem::transmute::<*mut c_void, GetCurrentDisplay>(gcd))();
        if display.is_null() {
            return Err("no current EGL display (GLX context?)".into());
        }
        let ext = (std::mem::transmute::<*mut c_void, QueryString>(qs))(display, EGL_EXTENSIONS);
        let ext = if ext.is_null() { String::new() } else { CStr::from_ptr(ext).to_string_lossy().into_owned() };
        let has = |e: &str| ext.split_ascii_whitespace().any(|x| x == e);
        if !has("EGL_EXT_image_dma_buf_import") {
            return Err("EGL_EXT_image_dma_buf_import missing".into());
        }
        let (c, d, t) = (gp(c"eglCreateImageKHR".as_ptr()), gp(c"eglDestroyImageKHR".as_ptr()), gp(c"glEGLImageTargetTexture2DOES".as_ptr()));
        if c.is_null() || d.is_null() || t.is_null() {
            return Err("eglCreateImageKHR / glEGLImageTargetTexture2DOES unavailable".into());
        }
        Ok(Self {
            display,
            create: std::mem::transmute(c),
            destroy: std::mem::transmute(d),
            target: std::mem::transmute(t),
            get_error: std::mem::transmute(ge),
            modifiers: has("EGL_EXT_image_dma_buf_import_modifiers"),
        })
    }

    /// Import one plane. The fd is only read during the call (EGL takes its
    /// own reference to the buffer).
    ///
    /// # Safety
    /// As for [`Egl::load`].
    pub unsafe fn create_image(&self, p: &PlaneImport, fd: BorrowedFd<'_>, modifier: u64) -> Result<*mut c_void, String> {
        if modifier != DRM_FORMAT_MOD_INVALID && modifier != DRM_FORMAT_MOD_LINEAR && !self.modifiers {
            return Err(format!("modifier {modifier:#x} needs EGL_EXT_image_dma_buf_import_modifiers"));
        }
        let attribs = image_attribs(p, fd.as_raw_fd(), if self.modifiers { modifier } else { DRM_FORMAT_MOD_INVALID });
        let img = (self.create)(self.display, std::ptr::null_mut(), EGL_LINUX_DMA_BUF_EXT, std::ptr::null_mut(), attribs.as_ptr());
        if img.is_null() {
            return Err(format!("eglCreateImageKHR failed ({:#x}) for {}x{} {:?}", (self.get_error)(), p.width, p.height, fourcc_str(p.format)));
        }
        Ok(img)
    }

    /// Bind `img` as the storage of the currently bound GL_TEXTURE_2D.
    ///
    /// # Safety
    /// As for [`Egl::load`]; `img` from [`create_image`](Self::create_image).
    pub unsafe fn bind_texture_2d(&self, img: *mut c_void) {
        (self.target)(0x0DE1 /* GL_TEXTURE_2D */, img);
    }

    /// # Safety
    /// `img` from this display, not bound to a live texture's future use.
    pub unsafe fn destroy_image(&self, img: *mut c_void) {
        (self.destroy)(self.display, img);
    }
}

fn fourcc_str(f: u32) -> String {
    f.to_le_bytes().iter().map(|&b| b as char).collect()
}

/// A buffer's identity for the import cache: the same dma-buf (device,
/// inode) at the same offset and shape is the same import.
pub fn buffer_key(fd: BorrowedFd<'_>, p: &PlaneImport, modifier: u64) -> Option<(u64, u64, PlaneImport, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat on a borrowed fd into a local.
    (unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0).then(|| (st.st_dev as u64, st.st_ino as u64, *p, modifier))
}

/// Read a LINEAR dma-buf's planes through mmap: the fallback when import
/// fails. Brackets the read with DMA_BUF_IOCTL_SYNC so a cached mapping sees
/// the decoder's writes (ignored where the fd is not a dma-buf, e.g. the
/// memfd the tests use).
pub fn map_linear(img: &DmaBufImage, w: u32, h: u32) -> Result<(vidkit_core::frame::PlaneLayout, CpuPlanes), String> {
    if img.modifier != DRM_FORMAT_MOD_LINEAR {
        return Err(format!("modifier {:#x} is not linear; the CPU cannot untile it", img.modifier));
    }
    let (layout, imps) = vidkit_core::dmabuf::plane_imports(img, w, h)?;
    let bpt = |f: u32| match f {
        vidkit_core::dmabuf::DRM_FORMAT_R8 => 1usize,
        vidkit_core::dmabuf::DRM_FORMAT_GR88 | vidkit_core::dmabuf::DRM_FORMAT_R16 => 2,
        _ => 4,
    };
    let mut data = Vec::new();
    let mut strides = Vec::new();
    for p in &imps {
        let fd = img.planes[p.fd_index].fd.as_raw_fd();
        let row = p.width as usize * bpt(p.format);
        let len = p.offset as usize + p.stride as usize * (p.height as usize - 1) + row;
        // SAFETY: a read-only shared mapping of `len` bytes of the fd,
        // unmapped below; the slice does not outlive it.
        let base = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0) };
        if base == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        sync(fd, true);
        let src = unsafe { std::slice::from_raw_parts(base as *const u8, len) };
        let mut plane = Vec::with_capacity(row * p.height as usize);
        for y in 0..p.height as usize {
            let s = p.offset as usize + y * p.stride as usize;
            plane.extend_from_slice(&src[s..s + row]);
        }
        sync(fd, false);
        unsafe { libc::munmap(base, len) };
        data.push(plane);
        strides.push(row as u32);
    }
    Ok((layout, CpuPlanes { data, strides }))
}

/// DMA_BUF_IOCTL_SYNC (_IOW('b', 0, u64)) around CPU reads.
fn sync(fd: i32, start: bool) {
    const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
    let flags: u64 = if start { 1 /* READ|START */ } else { 1 | 4 /* READ|END */ };
    // SAFETY: the ioctl reads one u64; ENOTTY on a non-dma-buf is harmless.
    unsafe { libc::ioctl(fd, DMA_BUF_IOCTL_SYNC as _, &flags) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use vidkit_core::dmabuf::{DRM_FORMAT_GR88, DRM_FORMAT_NV12, DRM_FORMAT_R8};
    use vidkit_core::frame::{DmaBufPlane, PlaneLayout};

    #[test]
    fn attributes_carry_the_plane_and_an_explicit_modifier() {
        let p = PlaneImport { fd_index: 0, format: DRM_FORMAT_GR88, width: 960, height: 540, offset: 2_088_960, stride: 2048 };
        let a = image_attribs(&p, 7, 0x0100_0000_0000_0002);
        assert_eq!(&a[..12], &[EGL_WIDTH, 960, EGL_HEIGHT, 540, EGL_LINUX_DRM_FOURCC_EXT, DRM_FORMAT_GR88 as i32, EGL_DMA_BUF_PLANE0_FD_EXT, 7, EGL_DMA_BUF_PLANE0_OFFSET_EXT, 2_088_960, EGL_DMA_BUF_PLANE0_PITCH_EXT, 2048]);
        assert_eq!(&a[12..], &[EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, 2, EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, 0x0100_0000, EGL_NONE]);
        let implicit = image_attribs(&p, 7, DRM_FORMAT_MOD_INVALID);
        assert_eq!(implicit.len(), 13, "no modifier attributes for an implicit layout");
        assert_eq!(image_attribs(&p, 7, 0).len(), 17, "LINEAR is explicit");
    }

    /// A padded single-fd NV12 buffer (stride 80 for 64 wide) read back
    /// through mmap: rows unpadded, chroma found after luma.
    #[test]
    fn a_linear_buffer_maps_to_cpu_planes() {
        let (w, h, stride) = (64u32, 48u32, 80u32);
        let mut buf = vec![0u8; (stride * h + stride * h / 2) as usize];
        for y in 0..h {
            for x in 0..w {
                buf[(y * stride + x) as usize] = (x + y) as u8;
            }
        }
        for y in 0..h / 2 {
            for x in 0..w {
                buf[(stride * h + y * stride + x) as usize] = 100 + (x % 2) as u8 * 50;
            }
        }
        let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-dmabuf".as_ptr(), 0)) };
        std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone().unwrap()), &buf).unwrap();
        let img = DmaBufImage { planes: vec![DmaBufPlane { fd, offset: 0, stride }], modifier: 0, fourcc: DRM_FORMAT_NV12, owner: None };
        let (layout, cpu) = map_linear(&img, w, h).unwrap();
        assert_eq!(layout, PlaneLayout::Nv12);
        assert_eq!(cpu.strides, vec![64, 64]);
        assert_eq!(cpu.data[0][(10 * 64 + 5) as usize], 15);
        assert_eq!(&cpu.data[1][..4], &[100, 150, 100, 150]);
        assert_eq!(cpu.data[1].len(), 64 * 24);
        let tiled = DmaBufImage { modifier: 0x0100_0000_0000_0002, ..img };
        assert!(map_linear(&tiled, w, h).is_err(), "tiled buffers are not read as linear");
        let _ = DRM_FORMAT_R8;
    }
}
