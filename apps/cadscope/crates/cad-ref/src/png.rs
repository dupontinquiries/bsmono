//! png.rs — image dimensions from the IHDR chunk, without pulling in an image decoder. The corpus
//! only needs to know how big a texture is (and later, its hashes); decoding pixels is `cad-tex`'s
//! job at M4, and it should not be a cost every scan pays.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::{TextureFacts, TextureRole};

const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub fn facts(path: &Path) -> Option<TextureFacts> {
    // The header is the first 33 bytes; reading the whole file to learn its width would make a
    // scan of a texture directory needlessly expensive.
    let mut head = [0u8; 33];
    let mut f = File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    parse(&head[..n])
}

pub fn parse(b: &[u8]) -> Option<TextureFacts> {
    if b.len() < 24 || b[..8] != SIG || &b[12..16] != b"IHDR" {
        return None;
    }
    let rd = |o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    Some(TextureFacts { width: rd(16), height: rd(20), role: TextureRole::Base })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(w: u32, h: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&SIG);
        b.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]); // depth, colour type, compression, filter, interlace
        b
    }

    #[test]
    fn reads_dimensions() {
        let f = parse(&synthetic(1024, 512)).expect("valid png");
        assert_eq!((f.width, f.height), (1024, 512));
    }

    #[test]
    fn refuses_non_png_and_truncation() {
        assert!(parse(b"GIF89a....").is_none());
        let full = synthetic(64, 64);
        for n in 0..full.len() {
            let _ = parse(&full[..n]);
        }
    }
}
