//! Bit-level reader and writer used by the entropy coder.
//!
//! Both are big-endian on the bitstream wire (MSB first within each byte). 
//! Round-trip safe: `BitReader::read(n)` undoes `BitWriter::write(value, n)`.

/// MSB-first bit writer. Buffers bits internally; call `flush()` to get 
/// the final byte vec (with any trailing bits zero-padded).
pub struct BitWriter {
    buf: Vec<u8>,
    bits: u32,
    n: u32,
}

impl Default for BitWriter {
    fn default() -> Self { Self::new() }
}

impl BitWriter {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(4096), bits: 0, n: 0 }
    }
    
    /// Write the low `nbits` of `value` to the stream.
    #[inline(always)]
    pub fn write(&mut self, value: u32, nbits: u32) {
        self.bits = (self.bits << nbits) | (value & ((1 << nbits) - 1));
        self.n += nbits;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push(((self.bits >> self.n) & 0xFF) as u8);
        }
    }
    
    /// Pad to byte boundary and return the buffer.
    pub fn flush(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.buf.push(((self.bits << (8 - self.n)) & 0xFF) as u8);
        }
        self.buf
    }
    
    pub fn bit_count(&self) -> usize {
        self.buf.len() * 8 + self.n as usize
    }
}

/// MSB-first bit reader. Reads beyond end-of-buffer return 0 bits (caller 
/// is responsible for not reading past the encoded length).
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u32,
    n: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bits: 0, n: 0 }
    }
    
    /// Read `nbits` bits and return as u32.
    #[inline(always)]
    pub fn read(&mut self, nbits: u32) -> u32 {
        while self.n < nbits {
            let byte = if self.pos < self.data.len() { self.data[self.pos] } else { 0 };
            self.pos += 1;
            self.bits = (self.bits << 8) | byte as u32;
            self.n += 8;
        }
        self.n -= nbits;
        (self.bits >> self.n) & ((1 << nbits) - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn bitwriter_roundtrip() {
        let mut w = BitWriter::new();
        // Variety of bit widths
        let pairs = [(0xA, 4), (0x55, 8), (0x3FF, 10), (1, 1), (0, 1), (0xFFFFF, 20)];
        for &(v, n) in &pairs { w.write(v, n); }
        let bytes = w.flush();
        let mut r = BitReader::new(&bytes);
        for &(v, n) in &pairs {
            assert_eq!(r.read(n), v, "{}-bit value {} mismatch", n, v);
        }
    }
    
    #[test]
    fn bitwriter_aligns_byte_boundary() {
        let mut w = BitWriter::new();
        w.write(0xFF, 8);
        w.write(0x00, 8);
        let bytes = w.flush();
        assert_eq!(bytes, vec![0xFF, 0x00]);
    }
    
    #[test]
    fn bitreader_returns_zero_past_end() {
        let bytes = vec![0xFF];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(8), 0xFF);
        // Reading past end yields zero bits
        assert_eq!(r.read(8), 0);
    }
}
