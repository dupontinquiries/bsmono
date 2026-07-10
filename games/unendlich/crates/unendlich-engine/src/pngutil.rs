//! pngutil — a dependency-free RGBA8 → PNG encoder.
//!
//! The headless example/proof binaries each rasterize a scene on the CPU and dump it to a PNG so the
//! output can be eyeballed without a GPU. Every one of them had copy-pasted the same three helpers
//! (`write_png` + a stored-block zlib + a CRC32 chunk writer); this is that code, extracted once so the
//! examples call `unendlich_engine::pngutil::write_png(...)` instead of carrying their own copy.
//!
//! Store-only DEFLATE (no compression) keeps it tiny and dependency-free — fine for debug dumps.

use std::io::Write;

/// Write an RGBA8 buffer (`w*h*4` bytes, row-major, no padding) to `path` as a PNG.
pub fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0); // filter byte: none
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}

/// One PNG chunk: length + type + data + CRC32.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

/// zlib stream wrapping store-only (uncompressed) DEFLATE blocks + an Adler-32 trailer.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let fin = i + n >= data.len();
        z.push(if fin { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_png_signature_and_chunks() {
        let dir = std::env::temp_dir().join(format!("pngutil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.png");
        let rgba = vec![0u8; 4 * 4]; // 2x2 RGBA
        write_png(p.to_str().unwrap(), 2, 2, &rgba).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        // IHDR immediately after signature, IEND at the tail
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
