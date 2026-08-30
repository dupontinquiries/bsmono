//! png.rs — writing the three maps out in the engine's convention.
//!
//! A real PNG encoder, but a deliberately small one: zlib *stored* blocks rather than deflate
//! compression. The files are larger than they need to be, and every viewer reads them, which for
//! a bake step that runs occasionally is the right trade against pulling in an image crate for
//! what amounts to a header, a CRC and an adler sum.

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut with_tag = Vec::with_capacity(4 + body.len());
    with_tag.extend_from_slice(tag);
    with_tag.extend_from_slice(body);
    out.extend_from_slice(&with_tag);
    out.extend_from_slice(&crc32(&with_tag).to_be_bytes());
}

/// Encode RGB8 rows as a PNG. `rgb` must be `w * h * 3` bytes.
pub fn encode_rgb(rgb: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if rgb.len() != (w as usize) * (h as usize) * 3 {
        return None;
    }
    // Filter byte 0 (None) per scanline.
    let mut raw = Vec::with_capacity(rgb.len() + h as usize);
    for y in 0..h as usize {
        raw.push(0);
        let row = y * w as usize * 3;
        raw.extend_from_slice(&rgb[row..row + w as usize * 3]);
    }

    // zlib stream: header, stored deflate blocks, adler.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_header_our_own_reader_understands() {
        let img = vec![128u8; 8 * 4 * 3];
        let png = encode_rgb(&img, 8, 4).expect("encodes");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        // Dimensions land where cad-ref's PNG reader looks for them.
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 8);
        assert_eq!(u32::from_be_bytes([png[20], png[21], png[22], png[23]]), 4);
        assert!(png.windows(4).any(|w| w == b"IEND"));
    }

    #[test]
    fn refuses_a_buffer_of_the_wrong_size() {
        assert!(encode_rgb(&[0u8; 10], 8, 4).is_none());
    }

    #[test]
    fn large_images_span_multiple_stored_blocks() {
        // 256x256 RGB is ~196KB of raw scanlines, so more than one 64KB block.
        let img = vec![7u8; 256 * 256 * 3];
        let png = encode_rgb(&img, 256, 256).expect("encodes");
        assert!(png.len() > 196_000, "unexpectedly small: {}", png.len());
    }
}
