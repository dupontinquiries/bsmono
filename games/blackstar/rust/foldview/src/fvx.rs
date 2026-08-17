//! FVX — the foldview extended mesh format.
//!
//! IQM stores what a game renders; FVX stores what a transformation needs:
//! the REPAIRED, SEGMENTED geometry (seam-snapped, welded, componentized)
//! that cohesion depends on. Baked once with `--bake`, loaded as a sidecar
//! (`model.fvx` beside `model.iqm`) in place of live segmentation.
//!
//! Layout (little-endian), version 1:
//!   [0..4)   magic  b"FVX1"
//!   [4..8)   version u32 = 1
//!   [8..12)  npieces u32
//!   [12..16) flags u32 (reserved: bit0 canonical-baked, bit1 roles present)
//!   [16..32) reserved
//!   then per piece: nverts u32, ntris u32
//!   then per piece: pos f32*3*nverts, nrm f32*3*nverts, idx u32*3*ntris
//!
//! Future versions reserve flags for baked orientation frames and per-piece
//! role/bone hints so hand-repairs can be stored, not recomputed.

use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::path::Path;

pub struct FvxPiece {
    pub pos: Vec<[f64; 3]>,
    pub nrm: Vec<[f64; 3]>,
    pub idx: Vec<u32>,
}

pub fn save(path: &Path, pieces: &[FvxPiece]) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"FVX1")?;
    f.write_all(&1u32.to_le_bytes())?;
    f.write_all(&(pieces.len() as u32).to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&[0u8; 16])?;
    for p in pieces {
        f.write_all(&(p.pos.len() as u32).to_le_bytes())?;
        f.write_all(&((p.idx.len() / 3) as u32).to_le_bytes())?;
    }
    for p in pieces {
        for q in &p.pos {
            for k in 0..3 { f.write_all(&(q[k] as f32).to_le_bytes())?; }
        }
        for q in &p.nrm {
            for k in 0..3 { f.write_all(&(q[k] as f32).to_le_bytes())?; }
        }
        for &i in &p.idx { f.write_all(&i.to_le_bytes())?; }
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Vec<FvxPiece>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    if buf.len() < 32 || &buf[0..4] != b"FVX1" { bail!("not an FVX file"); }
    let u32at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    if u32at(4) != 1 { bail!("unsupported FVX version {}", u32at(4)); }
    let np = u32at(8) as usize;
    let mut counts = Vec::with_capacity(np);
    let mut o = 32;
    for _ in 0..np {
        counts.push((u32at(o) as usize, u32at(o + 4) as usize));
        o += 8;
    }
    let f32at = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as f64;
    let mut pieces = Vec::with_capacity(np);
    for &(nv, nt) in &counts {
        let mut pos = Vec::with_capacity(nv);
        for _ in 0..nv { pos.push([f32at(o), f32at(o + 4), f32at(o + 8)]); o += 12; }
        let mut nrm = Vec::with_capacity(nv);
        for _ in 0..nv { nrm.push([f32at(o), f32at(o + 4), f32at(o + 8)]); o += 12; }
        let mut idx = Vec::with_capacity(nt * 3);
        for _ in 0..nt * 3 { idx.push(u32at(o)); o += 4; }
        pieces.push(FvxPiece { pos, nrm, idx });
    }
    Ok(pieces)
}
