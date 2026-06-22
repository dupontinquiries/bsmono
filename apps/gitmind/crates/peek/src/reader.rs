//! File reader
use anyhow::Result;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct Reader { mmap: Mmap }

impl Reader {
    pub fn open<P: AsRef<Path>>(p: P) -> Result<Self> {
        let f = File::open(p.as_ref())?;
        let m = unsafe { Mmap::map(&f)? };
        Ok(Self { mmap: m })
    }

    pub fn read_range(&self, start: usize, end: usize) -> Result<Vec<String>> {
        let r = BufReader::new(&self.mmap[..]);
        let mut lines = Vec::new();
        let mut cur = 1;
        for ln in r.lines() {
            let l = ln?;
            if cur >= start && cur <= end { lines.push(l); }
            if cur > end { break; }
            cur += 1;
        }
        Ok(lines)
    }
}
