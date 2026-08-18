//! sparse — Android sparse images, for payloads over the download limit.
//!
//! The FP5 reports `max-download-size:805306368` — 768 MiB. `boot_a` is 96 MiB
//! and fits; a rootfs does not, and `cockpit::session::upload` currently
//! refuses anything larger because there was nothing else it could honestly
//! do. This is the format that gets past that: the image is split into chunks
//! the bootloader reassembles, and runs of identical blocks travel as a
//! four-byte fill rather than as megabytes of zeros.
//!
//! ## Ported from a working flasher, not from the spec
//!
//! The layout here comes from `pm_flash/sparse.js` in the rusty-flash-fp5
//! tree — a browser flasher that really does flash this device. Two details in
//! it are the kind that only appear after something has been made to work:
//!
//! * **The header CRC is zero.** Its comment: *"AOSP docs specify that this
//!   should be a CRC32, but AOSP libsparse always sets 0 and puts the CRC in a
//!   final undocumented 0xCAC4 chunk instead."* Writing a real CRC32 in the
//!   header would follow the documentation and disagree with every tool that
//!   reads these files.
//! * **64 MiB raw chunks.** Not a round number for its own sake: it keeps each
//!   chunk comfortably under a 768 MiB download while leaving the count low
//!   enough that per-chunk overhead stays irrelevant.
//!
//! ## What this does not do
//!
//! It writes; it does not read. A flasher needs to produce sparse images and
//! the bootloader consumes them, so a parser would be code with no caller —
//! and this session has spent its length finding those.

/// `0xed26ff3a`, little-endian on the wire.
pub const FILE_MAGIC: u32 = 0xed26_ff3a;
pub const MAJOR_VERSION: u16 = 1;
pub const MINOR_VERSION: u16 = 0;
/// Bytes in the file header.
pub const FILE_HEADER_SIZE: u16 = 28;
/// Bytes in each chunk header.
pub const CHUNK_HEADER_SIZE: u16 = 12;
/// The largest raw chunk written. See the module note.
pub const RAW_CHUNK_SIZE: usize = 64 * 1024 * 1024;

/// Chunk kinds, with AOSP's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ChunkType {
    /// Literal bytes follow.
    Raw = 0xCAC1,
    /// One four-byte pattern, repeated to fill the blocks.
    Fill = 0xCAC2,
    /// Nothing follows; the bootloader leaves these blocks alone.
    Skip = 0xCAC3,
    /// The CRC AOSP puts here instead of in the header.
    Crc32 = 0xCAC4,
}

/// One chunk to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    Raw(Vec<u8>),
    /// A repeated four-byte pattern covering `blocks` blocks.
    Fill { pattern: [u8; 4], blocks: u32 },
    Skip { blocks: u32 },
}

impl Chunk {
    fn kind(&self) -> ChunkType {
        match self {
            Chunk::Raw(_) => ChunkType::Raw,
            Chunk::Fill { .. } => ChunkType::Fill,
            Chunk::Skip { .. } => ChunkType::Skip,
        }
    }

    /// Blocks this chunk covers on the device.
    pub fn blocks(&self, block_size: u32) -> u32 {
        match self {
            Chunk::Raw(d) => (d.len() as u32).div_ceil(block_size),
            Chunk::Fill { blocks, .. } | Chunk::Skip { blocks } => *blocks,
        }
    }

    /// Bytes this chunk occupies in the sparse FILE, header included.
    pub fn wire_len(&self) -> usize {
        CHUNK_HEADER_SIZE as usize
            + match self {
                Chunk::Raw(d) => d.len(),
                Chunk::Fill { .. } => 4,
                Chunk::Skip { .. } => 0,
            }
    }
}

/// Split an image into chunks.
///
/// Runs of a repeated four-byte pattern become `Fill`, which is the whole
/// reason the format exists: a freshly-made filesystem is mostly zeros, and
/// sending those as literal bytes wastes the transfer the size limit is
/// already straining.
///
/// A trailing partial block is emitted as `Raw` rather than padded here — the
/// caller knows whether the partition wants padding and this does not.
pub fn chunkify(image: &[u8], block_size: u32) -> Vec<Chunk> {
    let bs = block_size as usize;
    if bs == 0 || image.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Chunk> = Vec::new();
    let mut i = 0;
    while i < image.len() {
        let end = (i + bs).min(image.len());
        let block = &image[i..end];
        // A full block of one repeated 4-byte pattern is fillable.
        let fill = (block.len() == bs && bs % 4 == 0)
            .then(|| {
                let p: [u8; 4] = [block[0], block[1], block[2], block[3]];
                block.chunks_exact(4).all(|c| c == p).then_some(p)
            })
            .flatten();
        match fill {
            Some(p) => match out.last_mut() {
                // Extend a run rather than emitting a chunk per block: a 1 GiB
                // zero region is one Fill, not 262,144 of them.
                Some(Chunk::Fill { pattern, blocks }) if *pattern == p => *blocks += 1,
                _ => out.push(Chunk::Fill { pattern: p, blocks: 1 }),
            },
            None => match out.last_mut() {
                Some(Chunk::Raw(d)) if d.len() + block.len() <= RAW_CHUNK_SIZE => {
                    d.extend_from_slice(block)
                }
                _ => out.push(Chunk::Raw(block.to_vec())),
            },
        }
        i = end;
    }
    out
}

/// Serialise chunks into a sparse image.
pub fn write_image(chunks: &[Chunk], block_size: u32) -> Vec<u8> {
    let total_blocks: u32 = chunks.iter().map(|c| c.blocks(block_size)).sum();
    let mut out = Vec::with_capacity(
        FILE_HEADER_SIZE as usize + chunks.iter().map(Chunk::wire_len).sum::<usize>());

    out.extend_from_slice(&FILE_MAGIC.to_le_bytes());
    out.extend_from_slice(&MAJOR_VERSION.to_le_bytes());
    out.extend_from_slice(&MINOR_VERSION.to_le_bytes());
    out.extend_from_slice(&FILE_HEADER_SIZE.to_le_bytes());
    out.extend_from_slice(&CHUNK_HEADER_SIZE.to_le_bytes());
    out.extend_from_slice(&block_size.to_le_bytes());
    out.extend_from_slice(&total_blocks.to_le_bytes());
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    // ZERO, deliberately. AOSP documents a CRC32 here and libsparse always
    // writes 0, putting the real CRC in a trailing 0xCAC4 chunk. Following the
    // documentation would disagree with every tool that reads these.
    out.extend_from_slice(&0u32.to_le_bytes());

    for c in chunks {
        out.extend_from_slice(&(c.kind() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());   // reserved
        out.extend_from_slice(&c.blocks(block_size).to_le_bytes());
        out.extend_from_slice(&(c.wire_len() as u32).to_le_bytes());
        match c {
            Chunk::Raw(d) => out.extend_from_slice(d),
            Chunk::Fill { pattern, .. } => out.extend_from_slice(pattern),
            Chunk::Skip { .. } => {}
        }
    }
    out
}

/// Would this image need sparsing to reach a device with this limit?
pub fn needs_sparse(image_len: usize, max_download: u32) -> bool {
    image_len > max_download as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The FP5's reported limit, from `testdata/fp5-getvar-all.txt`.
    const FP5_MAX_DOWNLOAD: u32 = 805_306_368;

    #[test]
    fn the_fp5_limit_is_what_makes_this_necessary() {
        // boot_a is 96 MiB and fits; a rootfs does not. session::upload
        // refuses anything over the limit because there was nothing else it
        // could honestly do.
        assert!(!needs_sparse(96 * 1024 * 1024, FP5_MAX_DOWNLOAD), "boot fits whole");
        assert!(needs_sparse(3 * 1024 * 1024 * 1024, FP5_MAX_DOWNLOAD), "a rootfs does not");
    }

    #[test]
    fn the_header_crc_is_zero_because_libsparse_writes_zero() {
        // AOSP DOCUMENTS a CRC32 here and its own tool always writes 0, with
        // the real CRC in a trailing 0xCAC4 chunk. Following the documentation
        // would disagree with every reader.
        let img = write_image(&[Chunk::Skip { blocks: 1 }], 4096);
        assert_eq!(u32::from_le_bytes(img[24..28].try_into().unwrap()), 0);
    }

    #[test]
    fn the_header_matches_the_layout_a_real_flasher_writes() {
        let img = write_image(&[Chunk::Skip { blocks: 7 }], 4096);
        assert_eq!(u32::from_le_bytes(img[0..4].try_into().unwrap()), FILE_MAGIC);
        assert_eq!(u16::from_le_bytes(img[4..6].try_into().unwrap()), 1, "major");
        assert_eq!(u16::from_le_bytes(img[6..8].try_into().unwrap()), 0, "minor");
        assert_eq!(u16::from_le_bytes(img[8..10].try_into().unwrap()), 28, "file header size");
        assert_eq!(u16::from_le_bytes(img[10..12].try_into().unwrap()), 12, "chunk header size");
        assert_eq!(u32::from_le_bytes(img[12..16].try_into().unwrap()), 4096, "block size");
        assert_eq!(u32::from_le_bytes(img[16..20].try_into().unwrap()), 7, "total blocks");
        assert_eq!(u32::from_le_bytes(img[20..24].try_into().unwrap()), 1, "chunk count");
        // A Skip carries no payload, so the file is header + chunk header only.
        assert_eq!(img.len(), 28 + 12);
    }

    #[test]
    fn a_zero_region_becomes_one_fill_rather_than_thousands() {
        // The reason the format exists. A fresh filesystem is mostly zeros and
        // sending them literally wastes the transfer the limit is straining.
        let img = vec![0u8; 4096 * 500];
        let chunks = chunkify(&img, 4096);
        assert_eq!(chunks.len(), 1, "500 zero blocks should coalesce");
        assert_eq!(chunks[0], Chunk::Fill { pattern: [0; 4], blocks: 500 });
        // And it costs 4 bytes on the wire instead of 2 MiB.
        assert_eq!(chunks[0].wire_len(), 16);
        let written = write_image(&chunks, 4096);
        assert!(written.len() < 64, "a 2 MiB zero region wrote {} bytes", written.len());
    }

    #[test]
    fn a_fill_run_breaks_when_the_pattern_changes() {
        // Two different patterns must not merge, or the second region is
        // written with the first one's bytes — silent corruption of the kind
        // that only shows up after a flash.
        let mut img = vec![0u8; 4096 * 2];
        img.extend(std::iter::repeat(0xFF).take(4096 * 3));
        let chunks = chunkify(&img, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], Chunk::Fill { pattern: [0, 0, 0, 0], blocks: 2 });
        assert_eq!(chunks[1], Chunk::Fill { pattern: [0xFF; 4], blocks: 3 });
    }

    #[test]
    fn real_data_stays_raw_and_the_blocks_add_up() {
        // The invariant that matters: total_blocks in the header must equal
        // what the device will write, or the bootloader stops early or runs
        // past the partition.
        let mut img: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        img.extend(vec![0u8; 4096 * 4]);
        img.extend((0..4096u32).map(|i| (i % 7) as u8));
        let chunks = chunkify(&img, 4096);
        let blocks: u32 = chunks.iter().map(|c| c.blocks(4096)).sum();
        assert_eq!(blocks, 6, "one raw, four filled, one raw");
        let written = write_image(&chunks, 4096);
        assert_eq!(u32::from_le_bytes(written[16..20].try_into().unwrap()), 6);
        assert_eq!(u32::from_le_bytes(written[20..24].try_into().unwrap()),
                   chunks.len() as u32);
    }

    #[test]
    fn a_raw_chunk_is_capped_so_no_single_chunk_exceeds_the_limit() {
        // 64 MiB, from the flasher this was ported from: comfortably under a
        // 768 MiB download while keeping the chunk count low enough that
        // per-chunk overhead is irrelevant.
        let img: Vec<u8> = (0..RAW_CHUNK_SIZE + 8192).map(|i| (i % 253) as u8).collect();
        let chunks = chunkify(&img, 4096);
        assert!(chunks.len() >= 2, "a chunk over the cap must split");
        for c in &chunks {
            if let Chunk::Raw(d) = c {
                assert!(d.len() <= RAW_CHUNK_SIZE, "raw chunk of {} bytes", d.len());
            }
        }
        let total: usize = chunks.iter().map(|c| match c {
            Chunk::Raw(d) => d.len(),
            _ => 0,
        }).sum();
        assert_eq!(total, img.len(), "every byte survives the split");
    }

    #[test]
    fn a_partial_trailing_block_is_kept_rather_than_padded() {
        // Padding is the caller's decision: it knows whether the partition
        // wants it and this does not. Padding here would write bytes the image
        // never contained.
        let img = vec![0xABu8; 4096 + 100];
        let chunks = chunkify(&img, 4096);
        // The full block fills (0xAB repeats), the 100-byte tail stays raw.
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert_eq!(chunks[0], Chunk::Fill { pattern: [0xAB; 4], blocks: 1 });
        assert_eq!(chunks[1], Chunk::Raw(vec![0xAB; 100]));
        // And the tail is reported as one block, since a partial block still
        // occupies one on the device.
        assert_eq!(chunks[1].blocks(4096), 1);
    }

    #[test]
    fn an_empty_image_or_a_zero_block_size_yields_nothing() {
        assert!(chunkify(&[], 4096).is_empty());
        assert!(chunkify(&[1, 2, 3], 0).is_empty(), "a zero block size is not a division");
    }
}
