//! Bit-level reader for H.264 RBSP data.
//!
//! Reads in MSB-first order — the H.264 spec's bit ordering. Exp-Golomb
//! codes (`ue(v)` / `se(v)`) use the standard prefix-zeros encoding
//! defined in spec §9.1.
//!
//! The reader operates on a byte slice that is already free of emulation
//! prevention bytes (the NAL framer is responsible for stripping
//! `00 00 03 XX` → `00 00 XX` before feeding bits here).
//!
//! For the in-circuit decoder we want this code to be branch-light and
//! allocation-free. The current implementation prioritizes correctness
//! and clarity; we'll micro-optimize once it's parity-tested.

use crate::error::DecodeError;

#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bit offset measured from the start of `data`. Always
    /// `<= 8 * data.len()` outside of error paths.
    pos: u64,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position_bits(&self) -> u64 {
        self.pos
    }

    pub fn remaining_bits(&self) -> u64 {
        (self.data.len() as u64) * 8 - self.pos
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    /// Read `n` bits as an unsigned integer (0 ≤ n ≤ 32).
    pub fn read_bits(&mut self, n: u32) -> Result<u32, DecodeError> {
        if n > 32 {
            return Err(DecodeError::ReadTooWide);
        }
        if n == 0 {
            return Ok(0);
        }
        if self.remaining_bits() < n as u64 {
            return Err(DecodeError::EndOfStream);
        }
        let mut acc: u64 = 0;
        for _ in 0..n {
            let byte_idx = (self.pos / 8) as usize;
            let bit_in_byte = 7 - (self.pos % 8) as u8;
            let bit = (self.data[byte_idx] >> bit_in_byte) & 1;
            acc = (acc << 1) | bit as u64;
            self.pos += 1;
        }
        Ok(acc as u32)
    }

    /// Read a single bit. Returns 0 or 1.
    pub fn read_u1(&mut self) -> Result<u32, DecodeError> {
        self.read_bits(1)
    }

    /// Skip `n` bits without consuming the value.
    pub fn skip_bits(&mut self, n: u64) -> Result<(), DecodeError> {
        if self.remaining_bits() < n {
            return Err(DecodeError::EndOfStream);
        }
        self.pos += n;
        Ok(())
    }

    /// Skip to the next byte boundary if not already aligned.
    pub fn align_to_byte(&mut self) -> Result<(), DecodeError> {
        let drop = (8 - (self.pos % 8) as u32) % 8;
        self.skip_bits(drop as u64)
    }

    /// Read an unsigned Exp-Golomb code (`ue(v)`) — spec §9.1.
    ///
    /// The encoding is: `leadingZeroBits` zero bits, then a `1` bit,
    /// then `leadingZeroBits` more arbitrary bits. The decoded value is
    /// `2^leadingZeroBits - 1 + (those arbitrary bits)`.
    pub fn read_ue(&mut self) -> Result<u32, DecodeError> {
        let mut leading_zeros: u32 = 0;
        loop {
            if leading_zeros > 32 {
                return Err(DecodeError::ExpGolombOverflow);
            }
            match self.read_u1()? {
                0 => leading_zeros += 1,
                1 => break,
                _ => unreachable!(),
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zeros)?;
        // 2^leading_zeros - 1 + suffix; max = (1 << 32) - 2, fits u32 by
        // the loop guard above.
        let base = (1u64 << leading_zeros) - 1;
        let value = base + suffix as u64;
        if value > u32::MAX as u64 {
            return Err(DecodeError::ExpGolombOverflow);
        }
        Ok(value as u32)
    }

    /// Read a signed Exp-Golomb code (`se(v)`) — spec §9.1.1. Encoding
    /// is `ue(v)` then mapping unsigned `k` to signed
    /// `(-1)^(k+1) * ceil(k / 2)`.
    pub fn read_se(&mut self) -> Result<i32, DecodeError> {
        let code = self.read_ue()?;
        if code == 0 {
            return Ok(0);
        }
        // For code k:
        //   k odd  →  positive  (k+1)/2
        //   k even →  negative -k/2
        let half = (code + 1) / 2;
        if code & 1 == 1 {
            Ok(half as i32)
        } else {
            Ok(-(half as i32))
        }
    }

    /// True if more RBSP data follows the current position before the
    /// trailing-bits marker (spec §7.2). Used by syntax elements that
    /// have variable extent within a slice.
    ///
    /// Algorithm: there is more data iff at least one of the bits
    /// remaining before the *last* set bit in the buffer comes after
    /// our current position. The very last 1 bit in the buffer is the
    /// `rbsp_stop_one_bit`; everything past it is `rbsp_alignment_zero_bit`.
    pub fn more_rbsp_data(&self) -> bool {
        // Find the position of the trailing stop-one bit.
        if self.data.is_empty() {
            return false;
        }
        // Walk backwards over zero bits; the first one we hit is the
        // stop bit.
        let mut byte_idx = self.data.len() - 1;
        let mut last_byte = self.data[byte_idx];
        // Skip trailing zero bytes (alignment).
        while last_byte == 0 {
            if byte_idx == 0 {
                return false;
            }
            byte_idx -= 1;
            last_byte = self.data[byte_idx];
        }
        // Find the lowest set bit in last_byte — that's the stop bit.
        let stop_bit_in_byte = last_byte.trailing_zeros() as u64;
        let stop_pos = byte_idx as u64 * 8 + (7 - stop_bit_in_byte);
        self.pos < stop_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bits_msb_first() {
        // 0b1010_1100 = 0xAC
        let data = [0b1010_1100u8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(2).unwrap(), 0b10);
        assert_eq!(r.read_bits(4).unwrap(), 0b1100);
        assert_eq!(r.remaining_bits(), 0);
    }

    #[test]
    fn read_bits_across_byte_boundary() {
        let data = [0xFF, 0x00];
        let mut r = BitReader::new(&data);
        // Read 12 bits: should be 0xFF0 (high 12 bits of FF 00).
        assert_eq!(r.read_bits(12).unwrap(), 0xFF0);
    }

    #[test]
    fn read_bits_zero_returns_zero() {
        let data = [0xAAu8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(0).unwrap(), 0);
        assert_eq!(r.position_bits(), 0);
    }

    #[test]
    fn end_of_stream_error() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        r.read_bits(8).unwrap();
        assert_eq!(r.read_bits(1), Err(DecodeError::EndOfStream));
    }

    #[test]
    fn read_too_wide_error() {
        let data = [0u8; 8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(33), Err(DecodeError::ReadTooWide));
    }

    #[test]
    fn align_to_byte() {
        let data = [0xFF, 0xFF];
        let mut r = BitReader::new(&data);
        r.read_bits(3).unwrap();
        assert!(!r.is_byte_aligned());
        r.align_to_byte().unwrap();
        assert!(r.is_byte_aligned());
        assert_eq!(r.position_bits(), 8);
    }

    /// Spec §9.1 Table 9-1. Code = bits, value = decoded ue(v).
    #[test]
    fn ue_table_9_1() {
        // (encoding, expected unsigned value)
        let cases: &[(&[u8], u32)] = &[
            // "1" → 0
            (&[0b1000_0000], 0),
            // "010" → 1
            (&[0b0100_0000], 1),
            // "011" → 2
            (&[0b0110_0000], 2),
            // "00100" → 3
            (&[0b0010_0000], 3),
            // "00101" → 4
            (&[0b0010_1000], 4),
            // "00110" → 5
            (&[0b0011_0000], 5),
            // "00111" → 6
            (&[0b0011_1000], 6),
            // "0001000" → 7
            (&[0b0001_0000], 7),
            // "0001001" → 8
            (&[0b0001_0010], 8),
        ];
        for (i, (bits, expected)) in cases.iter().enumerate() {
            let mut r = BitReader::new(bits);
            let got = r.read_ue().unwrap();
            assert_eq!(got, *expected, "case {i}: encoding {bits:?}");
        }
    }

    #[test]
    fn se_round_trips_small() {
        // From spec §9.1.1: ue 0..6 maps to se 0,1,-1,2,-2,3,-3.
        let cases: &[(&[u8], i32)] = &[
            (&[0b1000_0000], 0),
            (&[0b0100_0000], 1),
            (&[0b0110_0000], -1),
            (&[0b0010_0000], 2),
            (&[0b0010_1000], -2),
            (&[0b0011_0000], 3),
            (&[0b0011_1000], -3),
        ];
        for (bits, expected) in cases {
            let mut r = BitReader::new(bits);
            assert_eq!(r.read_se().unwrap(), *expected);
        }
    }

    #[test]
    fn ue_excessive_zero_run_errors() {
        // 33 leading zeros — we cap at 32.
        let mut data = [0u8; 5];
        // Set the 34th bit (counting from 1) to 1.
        // 33 zeros means bit at index 33 (0-based) is 1 → byte 4 bit 7-(33%8)... ugh.
        // Easier: write 32 zeros, then a 1, then check it just succeeds (overflow guard hits at >32).
        // 33 leading zeros: bit 33 (1-indexed) is the '1'.
        // For bit (i_zero_based = 33), byte = 33/8 = 4, bit in byte = 7 - (33%8) = 7 - 1 = 6, mask 0x40.
        data[4] = 0x40;
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_ue(), Err(DecodeError::ExpGolombOverflow));
    }

    #[test]
    fn more_rbsp_data_basic() {
        // RBSP: payload bits then a stop-one bit then zero-padding.
        // 0b10101000 = data "1010" + stop bit "1" + alignment "000"
        let data = [0b1010_1000u8];
        let mut r = BitReader::new(&data);
        assert!(r.more_rbsp_data()); // we're at bit 0
        r.read_bits(4).unwrap(); // read the payload
        assert!(!r.more_rbsp_data()); // only the stop bit remains
    }

    #[test]
    fn more_rbsp_data_with_zero_padding() {
        // payload "1010" + stop "1" + four zeros + a full zero byte (over-padded).
        let data = [0b1010_1000u8, 0x00];
        let mut r = BitReader::new(&data);
        r.read_bits(4).unwrap();
        assert!(!r.more_rbsp_data());
    }
}
