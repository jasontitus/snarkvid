// Bit reader for the H.264 RBSP byte stream.
//
// Operates over `&[u8]` (the de-emulation-prevention-byte-stripped
// RBSP, produced by `nal::strip_emulation_prevention`). Tracks a
// bit cursor inside the slice; reads in big-endian order (MSB-first
// within each byte), as the H.264 spec specifies for "raw" bits.
//
// Exposes the four primitives the rest of the decoder needs:
//
//   - read_bit()         u(1)        — one bit.
//   - read_bits(n)       u(n)        — n raw unsigned bits, n ≤ 32.
//   - read_ue()          ue(v)       — unsigned Exp-Golomb.
//   - read_se()          se(v)       — signed Exp-Golomb (mapped from ue).
//   - read_te(x)         te(v)       — truncated Exp-Golomb with max value x.
//
// All reads return `Result<_, DecodeError>` and never panic. EOF
// surfaces as `BitstreamTruncated`. An Exp-Golomb codeword that
// would require more than `MAX_EXP_GOLOMB_LEADING_ZEROS` leading
// zeros is rejected with `ExpGolombTooLong` — guards against pathological
// inputs blowing up cycle counts.

use crate::DecodeError;

/// Cap on Exp-Golomb leading-zero count. The spec allows up to 32 in
/// principle (yields a 33-bit value); an attacker-shaped bitstream
/// could chew through the whole RBSP otherwise. In practice baseline
/// I-frames don't go above ~16.
pub const MAX_EXP_GOLOMB_LEADING_ZEROS: u32 = 32;

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    /// Bit position within `bytes`. `bit_pos / 8` is the byte index;
    /// `bit_pos % 8` is the bit offset within that byte (0 = MSB).
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_pos: 0 }
    }

    /// Number of bits remaining (>= 0).
    pub fn remaining_bits(&self) -> usize {
        self.bytes.len() * 8 - self.bit_pos
    }

    /// Whether the reader is byte-aligned. Used by some H.264 syntax
    /// elements that require alignment before they parse.
    pub fn is_byte_aligned(&self) -> bool {
        self.bit_pos % 8 == 0
    }

    /// Discard bits until the cursor is byte-aligned.
    pub fn align_to_byte(&mut self) {
        let r = self.bit_pos % 8;
        if r != 0 {
            self.bit_pos += 8 - r;
        }
    }

    /// Read one bit. Returns 0 or 1.
    #[inline]
    pub fn read_bit(&mut self) -> Result<u32, DecodeError> {
        if self.bit_pos >= self.bytes.len() * 8 {
            return Err(DecodeError::BitstreamTruncated);
        }
        let byte = self.bytes[self.bit_pos / 8];
        let shift = 7 - (self.bit_pos % 8) as u32;
        let bit = ((byte >> shift) & 1) as u32;
        self.bit_pos += 1;
        Ok(bit)
    }

    /// Read `n` raw bits as a big-endian unsigned integer. `n ≤ 32`.
    pub fn read_bits(&mut self, n: u32) -> Result<u32, DecodeError> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        if (self.bit_pos + n as usize) > self.bytes.len() * 8 {
            return Err(DecodeError::BitstreamTruncated);
        }
        let mut v: u32 = 0;
        // Per-bit loop: simple, branchless-ish, plenty fast for the
        // sizes the spec uses (mostly 1–8 bits per call). A
        // byte-bulk-load fast path can come later if profiling
        // shows it matters.
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Ok(v)
    }

    /// Unsigned Exp-Golomb code, `ue(v)`. Encoded as N leading zero
    /// bits, a 1, then N more bits; the value is `2^N + bits - 1`.
    /// (Spec §9.1.)
    pub fn read_ue(&mut self) -> Result<u32, DecodeError> {
        let mut zeros: u32 = 0;
        while self.read_bit()? == 0 {
            zeros += 1;
            if zeros > MAX_EXP_GOLOMB_LEADING_ZEROS {
                return Err(DecodeError::ExpGolombTooLong);
            }
        }
        if zeros == 0 {
            // Codeword "1" → value 0.
            Ok(0)
        } else {
            // Bits-after-marker: `zeros` more bits.
            let suffix = self.read_bits(zeros)?;
            // (1 << zeros) - 1 + suffix
            Ok((1u32 << zeros) - 1 + suffix)
        }
    }

    /// Signed Exp-Golomb code, `se(v)`. Maps unsigned k to:
    ///   k=0 → 0, k=1 → 1, k=2 → -1, k=3 → 2, k=4 → -2, ...
    /// (Spec §9.1.1.)
    pub fn read_se(&mut self) -> Result<i32, DecodeError> {
        let k = self.read_ue()?;
        if k == 0 {
            Ok(0)
        } else if k & 1 == 1 {
            // Odd → positive. (k+1)/2.
            Ok(((k + 1) >> 1) as i32)
        } else {
            // Even → negative. -((k+1)/2) ... but careful: for even k,
            // (k+1)/2 needs (k>>1) since k+1 is odd; rust int div rounds
            // toward zero. The mapping is: even k → -(k/2). Let's
            // double-check: k=2 → -1 ✓ (-(2/2)=-1). k=4 → -2 ✓.
            Ok(-((k >> 1) as i32))
        }
    }

    /// Truncated Exp-Golomb, `te(v)`. With max value `x`:
    ///   if x == 1 → read 1 bit, return 1 - bit (1→0, 0→1)
    ///   else      → same as ue(v).
    /// (Spec §9.1.2.)
    pub fn read_te(&mut self, x: u32) -> Result<u32, DecodeError> {
        if x == 1 {
            Ok(1 - self.read_bit()?)
        } else {
            self.read_ue()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bit_walks_bytes_msb_first() {
        // 0xA5 = 1010_0101
        let mut br = BitReader::new(&[0xA5]);
        for expected in [1, 0, 1, 0, 0, 1, 0, 1] {
            assert_eq!(br.read_bit().unwrap(), expected);
        }
        assert!(matches!(br.read_bit(), Err(DecodeError::BitstreamTruncated)));
    }

    #[test]
    fn read_bits_concatenates_across_byte_boundary() {
        // 0xAB, 0xCD = 1010_1011 1100_1101.
        // Reading 12 bits from the start = 1010_1011_1100 = 0xABC.
        let mut br = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(br.read_bits(12).unwrap(), 0xABC);
        // Then 4 more = 1101 = 0xD.
        assert_eq!(br.read_bits(4).unwrap(), 0xD);
        assert!(matches!(br.read_bits(1), Err(DecodeError::BitstreamTruncated)));
    }

    #[test]
    fn read_bits_zero_returns_zero() {
        let mut br = BitReader::new(&[0xFF]);
        assert_eq!(br.read_bits(0).unwrap(), 0);
        // Cursor unchanged.
        assert_eq!(br.remaining_bits(), 8);
    }

    #[test]
    fn align_to_byte_no_op_when_aligned() {
        let mut br = BitReader::new(&[0xFF, 0x00]);
        br.align_to_byte();
        assert_eq!(br.bit_pos, 0);
        br.read_bits(8).unwrap();
        br.align_to_byte();
        assert_eq!(br.bit_pos, 8);
    }

    #[test]
    fn align_to_byte_advances_to_next_byte() {
        let mut br = BitReader::new(&[0xFF, 0x55]);
        br.read_bit().unwrap();
        assert!(!br.is_byte_aligned());
        br.align_to_byte();
        assert!(br.is_byte_aligned());
        assert_eq!(br.bit_pos, 8);
        // Next 8 bits should be 0x55.
        assert_eq!(br.read_bits(8).unwrap(), 0x55);
    }

    // Exp-Golomb test vectors from H.264 spec Table 9-1.
    //
    //   value | codeword     | bits
    //     0   | 1            |   1
    //     1   | 010          |   3
    //     2   | 011          |   3
    //     3   | 00100        |   5
    //     4   | 00101        |   5
    //     5   | 00110        |   5
    //     6   | 00111        |   5
    //     7   | 0001000      |   7
    //     8   | 0001001      |   7
    fn pack_bits(bits: &[u8]) -> Vec<u8> {
        // Pack a sequence of 0/1 bits into bytes, MSB-first. Pad the
        // last byte with zeros so the reader doesn't run out.
        let mut out = Vec::new();
        let mut acc: u8 = 0;
        let mut count = 0u8;
        for &b in bits {
            acc = (acc << 1) | (b & 1);
            count += 1;
            if count == 8 {
                out.push(acc);
                acc = 0;
                count = 0;
            }
        }
        if count > 0 {
            acc <<= 8 - count;
            out.push(acc);
        }
        out
    }

    #[test]
    fn read_ue_known_values() {
        // 1   = 0
        // 010 = 1
        // 011 = 2
        // 00100 = 3
        // 00101 = 4
        // 0001000 = 7
        let bits = [1, 0,1,0, 0,1,1, 0,0,1,0,0, 0,0,1,0,1, 0,0,0,1,0,0,0];
        let bytes = pack_bits(&bits);
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_ue().unwrap(), 0);
        assert_eq!(br.read_ue().unwrap(), 1);
        assert_eq!(br.read_ue().unwrap(), 2);
        assert_eq!(br.read_ue().unwrap(), 3);
        assert_eq!(br.read_ue().unwrap(), 4);
        assert_eq!(br.read_ue().unwrap(), 7);
    }

    #[test]
    fn read_se_known_mapping() {
        // se mapping: ue 0→0, 1→1, 2→-1, 3→2, 4→-2.
        // Build a stream of ue codes 0,1,2,3,4 and read as se.
        let bits = [1, 0,1,0, 0,1,1, 0,0,1,0,0, 0,0,1,0,1];
        let bytes = pack_bits(&bits);
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_se().unwrap(), 0);
        assert_eq!(br.read_se().unwrap(), 1);
        assert_eq!(br.read_se().unwrap(), -1);
        assert_eq!(br.read_se().unwrap(), 2);
        assert_eq!(br.read_se().unwrap(), -2);
    }

    #[test]
    fn read_te_x_eq_1_inverts_one_bit() {
        // x=1: 0-bit → returns 1, 1-bit → returns 0.
        let bytes = pack_bits(&[0, 1, 0, 1]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_te(1).unwrap(), 1);
        assert_eq!(br.read_te(1).unwrap(), 0);
        assert_eq!(br.read_te(1).unwrap(), 1);
        assert_eq!(br.read_te(1).unwrap(), 0);
    }

    #[test]
    fn read_te_x_gt_1_falls_back_to_ue() {
        // x=2 → ue. Codeword "1" → 0; codeword "010" → 1.
        let bytes = pack_bits(&[1, 0, 1, 0]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_te(2).unwrap(), 0);
        assert_eq!(br.read_te(2).unwrap(), 1);
    }

    #[test]
    fn truncated_stream_propagates_error() {
        // Empty buffer: any read fails.
        let mut br = BitReader::new(&[]);
        assert!(matches!(br.read_bit(), Err(DecodeError::BitstreamTruncated)));
        assert!(matches!(br.read_ue(),  Err(DecodeError::BitstreamTruncated)));
        assert!(matches!(br.read_se(),  Err(DecodeError::BitstreamTruncated)));
    }

    #[test]
    fn pathological_long_zeros_rejected() {
        // 33 zero bits then a 1: zeros=33 > MAX_EXP_GOLOMB_LEADING_ZEROS.
        let mut bits = vec![0u8; 33];
        bits.push(1);
        let bytes = pack_bits(&bits);
        let mut br = BitReader::new(&bytes);
        assert!(matches!(br.read_ue(), Err(DecodeError::ExpGolombTooLong)));
    }
}
