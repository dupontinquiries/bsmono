//! FNV-1a 64 — the non-crypto hash this repo has hand-rolled at least ten
//! times (gitmind identity/encode, fao-codec block_dict/gpu_pipeline,
//! bo-suite device_driver/soundscape, paperbound-core's lockstep state hash…).
//!
//! Standard parameters: offset basis `0xcbf2_9ce4_8422_2325`, prime
//! `0x0000_0100_0000_01b3` — so replacing an existing copy with a call here is
//! bit-identical wherever those constants were used (which is all of the
//! surveyed sites). Migration rule for hashes pinned in protocols/tests:
//! replace the *implementation* with a call, never change the byte feed order.
//!
//! NOT for security. Keys, tokens, and digests live in `crypto-kit`.

pub const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
pub const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One-shot FNV-1a 64 over a byte slice.
#[inline]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = FNV1A64_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV1A64_PRIME);
    }
    h
}

/// Convenience: FNV-1a 64 over a str's UTF-8 bytes.
#[inline]
pub fn fnv1a_64_str(s: &str) -> u64 {
    fnv1a_64(s.as_bytes())
}

/// Streaming FNV-1a 64 — for state hashes fed incrementally (the lockstep
/// checkpoint pattern). Feed order is part of the contract.
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a64(u64);

impl Fnv1a64 {
    #[inline]
    pub fn new() -> Fnv1a64 {
        Fnv1a64(FNV1A64_OFFSET)
    }
    #[inline]
    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(FNV1A64_PRIME);
        }
        self
    }
    #[inline]
    pub fn write_u32(&mut self, v: u32) -> &mut Self {
        self.write(&v.to_le_bytes())
    }
    #[inline]
    pub fn write_u64(&mut self, v: u64) -> &mut Self {
        self.write(&v.to_le_bytes())
    }
    /// IEEE bit pattern — hashes f32 state deterministically (the lockstep way).
    #[inline]
    pub fn write_f32(&mut self, v: f32) -> &mut Self {
        self.write(&v.to_bits().to_le_bytes())
    }
    #[inline]
    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // canonical FNV-1a 64 test vectors
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x85944171f73967e8);
        assert_eq!(fnv1a_64_str("foobar"), fnv1a_64(b"foobar"));
    }

    #[test]
    fn streaming_equals_oneshot_and_feed_order_matters() {
        let mut h = Fnv1a64::new();
        h.write(b"foo").write(b"bar");
        assert_eq!(h.finish(), fnv1a_64(b"foobar"));

        let mut a = Fnv1a64::new();
        a.write_u32(1).write_u32(2);
        let mut b = Fnv1a64::new();
        b.write_u32(2).write_u32(1);
        assert_ne!(a.finish(), b.finish());

        // f32 goes in as its bit pattern
        let mut f = Fnv1a64::new();
        f.write_f32(1.5);
        let mut g = Fnv1a64::new();
        g.write(&1.5f32.to_bits().to_le_bytes());
        assert_eq!(f.finish(), g.finish());
    }
}
