//! In-memory backend: for tests, and a headless simulator on the desktop.
//!
//! With `EINK_DUMP=<dir>` the runner-facing behaviour is identical to a
//! device, and every commit writes `frame-NNNN.ppm` plus a line in
//! `updates.log` (rect, waveform, colour, reason). That log is the refresh
//! policy made visible: you can watch what a device WOULD be asked to do.

use super::{Caps, Present};
use crate::format::{decode, PixelFormat};
use crate::policy::Update;
use std::io::{self, Write};
use std::path::PathBuf;

pub struct MemPresent {
    caps: Caps,
    buf: Vec<u8>,
    pub commits: Vec<Vec<Update>>,
    dump: Option<PathBuf>,
    frame_no: u32,
}

impl MemPresent {
    pub fn new(width: u32, height: u32, format: PixelFormat, per_update_waveform: bool, color_panel: bool) -> MemPresent {
        let stride = width as usize * format.bytes_per_pixel();
        MemPresent {
            caps: Caps { width, height, stride, format, per_update_waveform, color_panel },
            buf: vec![0; stride * height as usize],
            commits: Vec::new(),
            dump: None,
            frame_no: 0,
        }
    }

    pub fn with_dump(mut self, dir: PathBuf) -> io::Result<MemPresent> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("updates.log"), "")?;
        self.dump = Some(dir);
        Ok(self)
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let bpp = self.caps.format.bytes_per_pixel();
        let o = y as usize * self.caps.stride + x as usize * bpp;
        decode(self.caps.format, &self.buf[o..o + bpp])
    }

    pub fn write_ppm(&self, path: &std::path::Path) -> io::Result<()> {
        let mut f = io::BufWriter::new(std::fs::File::create(path)?);
        write!(f, "P6\n{} {}\n255\n", self.caps.width, self.caps.height)?;
        for y in 0..self.caps.height {
            for x in 0..self.caps.width {
                let p = self.pixel(x, y);
                f.write_all(&p[..3])?;
            }
        }
        Ok(())
    }
}

impl Present for MemPresent {
    fn name(&self) -> &'static str {
        "mem"
    }
    fn caps(&self) -> Caps {
        self.caps
    }
    fn buffer(&mut self) -> &mut [u8] {
        &mut self.buf
    }
    fn commit(&mut self, updates: &[Update]) -> io::Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        self.commits.push(updates.to_vec());
        if let Some(dir) = self.dump.clone() {
            self.frame_no += 1;
            self.write_ppm(&dir.join(format!("frame-{:04}.ppm", self.frame_no)))?;
            let mut log = std::fs::OpenOptions::new().append(true).open(dir.join("updates.log"))?;
            for u in updates {
                writeln!(log, "{:04} {:?} {:?} color={} {}", self.frame_no, u.rect, u.waveform, u.color, u.reason)?;
            }
        }
        Ok(())
    }
}
