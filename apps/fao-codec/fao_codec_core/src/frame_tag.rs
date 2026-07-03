//! Per-frame metadata that travels with frames through the encode pipeline.
//!
//! Every frame produced by the demuxer is tagged with one of these. The tag 
//! flows through worker → compress → disk write, then is re-read on decode 
//! to determine what kind of frame this was and how to handle it.
//!
//! # Disk format
//!
//! Fixed 16 bytes per tag. This matters because tags are stored interleaved 
//! with frame payloads in the .fao container, and a fixed size lets the 
//! reader seek by index without an offset table.

/// Per-frame metadata.
#[derive(Clone, Debug, Default)]
pub struct FrameTag {
    /// Absolute frame index in source video (0-based).
    pub abs_idx: u32,
    /// Which reader segment produced this frame (parallel-decode index).
    pub seg_id: u16,
    /// Local index within the segment.
    pub seg_local: u16,
    /// Encode tier: 0=skip, 1=passthrough, 2=full, 3=anchor (reserved).
    pub tier: u8,
    /// Anchor frame — forced full encode regardless of activity.
    pub is_anchor: bool,
    /// Scene-change counter (increments at each detected cut).
    pub scene_id: u16,
    /// Original compressed frame size from ffprobe (used to size budgets).
    pub source_bytes: u32,
}

impl FrameTag {
    pub fn skip(abs: usize, seg: usize, local: usize, src_bytes: u64) -> Self {
        Self {
            abs_idx: abs as u32, seg_id: seg as u16, seg_local: local as u16,
            tier: 0, is_anchor: false, scene_id: 0, source_bytes: src_bytes as u32,
        }
    }
    pub fn passthrough(abs: usize, seg: usize, local: usize, src_bytes: u64) -> Self {
        Self {
            abs_idx: abs as u32, seg_id: seg as u16, seg_local: local as u16,
            tier: 1, is_anchor: false, scene_id: 0, source_bytes: src_bytes as u32,
        }
    }
    pub fn full(abs: usize, seg: usize, local: usize, src_bytes: u64) -> Self {
        Self {
            abs_idx: abs as u32, seg_id: seg as u16, seg_local: local as u16,
            tier: 2, is_anchor: false, scene_id: 0, source_bytes: src_bytes as u32,
        }
    }
    pub fn anchor(abs: usize, seg: usize, local: usize, src_bytes: u64) -> Self {
        Self {
            abs_idx: abs as u32, seg_id: seg as u16, seg_local: local as u16,
            tier: 2, is_anchor: true, scene_id: 0, source_bytes: src_bytes as u32,
        }
    }
    
    /// Serialize to 16 bytes for disk storage.
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.abs_idx.to_le_bytes());
        b[4..6].copy_from_slice(&self.seg_id.to_le_bytes());
        b[6..8].copy_from_slice(&self.seg_local.to_le_bytes());
        b[8] = self.tier;
        b[9] = if self.is_anchor { 1 } else { 0 };
        b[10..12].copy_from_slice(&self.scene_id.to_le_bytes());
        b[12..16].copy_from_slice(&self.source_bytes.to_le_bytes());
        b
    }
    
    pub fn from_bytes(b: &[u8; 16]) -> Self {
        Self {
            abs_idx: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            seg_id: u16::from_le_bytes([b[4], b[5]]),
            seg_local: u16::from_le_bytes([b[6], b[7]]),
            tier: b[8],
            is_anchor: b[9] != 0,
            scene_id: u16::from_le_bytes([b[10], b[11]]),
            source_bytes: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn frame_tag_roundtrip() {
        let t = FrameTag::full(42, 3, 17, 5000);
        let b = t.to_bytes();
        let back = FrameTag::from_bytes(&b);
        assert_eq!(back.abs_idx, 42);
        assert_eq!(back.seg_id, 3);
        assert_eq!(back.seg_local, 17);
        assert_eq!(back.tier, 2);
        assert!(!back.is_anchor);
        assert_eq!(back.source_bytes, 5000);
    }
    
    #[test]
    fn anchor_tier_preserved() {
        let t = FrameTag::anchor(100, 0, 0, 8000);
        let b = t.to_bytes();
        let back = FrameTag::from_bytes(&b);
        assert!(back.is_anchor);
        assert_eq!(back.tier, 2);
    }
    
    #[test]
    fn frame_tag_size_is_16() {
        let t = FrameTag::default();
        assert_eq!(t.to_bytes().len(), 16);
    }
}
