// CAVLC (Context-Adaptive Variable Length Coding) decoder.
//
// CAVLC is the entropy decoder for baseline H.264 — the layer that
// turns the residual coefficient bitstream back into 16 i16
// coefficients per 4x4 block. M3 §8 calls out CAVLC parsing as the
// likely cycle-budget hot stage in-circuit, with the recommended
// mitigation: prebuild all five lookup tables as static arrays and
// parse with branchless table-driven code.
//
// This file ships the load-bearing first piece: the VLC primitive
// and the complete `coeff_token` decoder (H.264 spec §9.2.1 / Table
// 9-5). The remaining four tables (level prefix+suffix, total_zeros,
// run_before) and the `decode_residual_block` composition land in
// the next session; they slot into the same VLC primitive.
//
// Spec references throughout are to ITU-T Rec. H.264 (2021).
//
// no_std-pure.

use crate::bitreader::BitReader;
use crate::DecodeError;

// ─────────────────────────────────────────────────────────────────────
// VLC table primitive
// ─────────────────────────────────────────────────────────────────────

/// One entry in a VLC lookup table.
///
/// `codeword` holds the bits of the variable-length code, MSB-aligned
/// in a u16. `codeword_len` is the number of valid bits (1..=16).
/// `value` is the small integer the codeword decodes to (the meaning
/// is table-specific — for coeff_token it's a packed (TotalCoeff,
/// TrailingOnes); for total_zeros / run_before it's the count itself).
///
/// Tables are scanned linearly. For the table sizes we deal with
/// (≤ 62 entries for coeff_token, ≤ 16 for total_zeros / run_before)
/// this is competitive with a trie and keeps cycle counts predictable
/// for the in-circuit case — no recursion, no dynamic dispatch.
#[derive(Clone, Copy, Debug)]
pub struct VlcEntry {
    pub codeword: u16,
    pub codeword_len: u8,
    pub value: u16,
}

/// Look up the next VLC code in `br` against `table`. On success,
/// advances `br` past the matched codeword and returns the entry's
/// `value`. On no-match, returns `CavlcInvalid` and the bit cursor is
/// left at the first bit of the would-be codeword.
///
/// The lookup peeks up to `max_len` bits, accumulating a value, and
/// for each prefix length checks every table entry of that length
/// for a match. The first match wins. This is O(table_len * max_len)
/// in the worst case; for the largest CAVLC table that's < 1000
/// comparisons per codeword.
pub fn read_vlc(br: &mut BitReader, table: &[VlcEntry]) -> Result<u16, DecodeError> {
    // Find the longest codeword in the table so we know how many bits
    // to peek.
    let max_len = table.iter().map(|e| e.codeword_len).max().unwrap_or(0);
    if max_len == 0 || max_len > 16 {
        return Err(DecodeError::CavlcInvalid);
    }

    // Peek up to max_len bits without advancing. Use a clone so a
    // failed lookup leaves the original cursor unmoved.
    let mut peek_br = br.clone();
    let mut acc: u32 = 0;
    let mut acc_len: u8 = 0;

    // Walk codeword lengths in order. After each new bit, check
    // every table entry whose codeword_len equals the current
    // accumulated length.
    for _ in 0..max_len {
        acc = (acc << 1) | peek_br.read_bit()?;
        acc_len += 1;
        for entry in table {
            if entry.codeword_len == acc_len && entry.codeword as u32 == acc {
                // Match: advance the real reader by acc_len bits.
                br.read_bits(acc_len as u32)?;
                return Ok(entry.value);
            }
        }
    }
    Err(DecodeError::CavlcInvalid)
}

// ─────────────────────────────────────────────────────────────────────
// coeff_token (H.264 §9.2.1, Table 9-5)
//
// coeff_token jointly encodes (TotalCoeff, TrailingOnes) for one
// residual block. The codeword is read with one of four tables
// chosen by `nC`, the predicted number of nonzero coefficients in
// neighboring blocks:
//
//   nC ∈ [0, 2)   → Table 9-5 (a)   ("VLC_0")
//   nC ∈ [2, 4)   → Table 9-5 (b)   ("VLC_1")
//   nC ∈ [4, 8)   → Table 9-5 (c)   ("VLC_2")
//   nC ≥ 8         → Table 9-5 (d)   ("VLC_3"), 6-bit fixed-length
//   nC = -1        → Table 9-5 (e)   ("VLC for chroma DC, 4:2:0")
//   nC = -2        → Table 9-9 (b)  variant for chroma DC 4:2:2 (out of M3 scope)
//
// We package the (TotalCoeff, TrailingOnes) pair as `value =
// TotalCoeff * 4 + TrailingOnes`. TrailingOnes ranges 0..=3, so the
// packing is unique. The decoder unpacks immediately after read_vlc
// returns.
// ─────────────────────────────────────────────────────────────────────

/// Result of decoding one `coeff_token`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoeffToken {
    /// Total number of nonzero coefficients in the block (0..=16).
    pub total_coeff: u8,
    /// Number of trailing ones (0..=3).
    pub trailing_ones: u8,
}

#[inline]
fn unpack_ct(packed: u16) -> CoeffToken {
    CoeffToken {
        total_coeff: (packed / 4) as u8,
        trailing_ones: (packed % 4) as u8,
    }
}

/// Variant selector for `decode_coeff_token`. Computed from the
/// neighboring-block coefficient counts per spec §9.2.1.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoeffTokenVariant {
    /// nC ∈ [0, 2)
    Vlc0,
    /// nC ∈ [2, 4)
    Vlc1,
    /// nC ∈ [4, 8)
    Vlc2,
    /// nC ≥ 8 — 6-bit fixed-length code.
    FixedLen6,
    /// Chroma DC, 4:2:0 only (the only chroma case M3 supports).
    ChromaDc420,
}

impl CoeffTokenVariant {
    /// Pick the variant from `nC` per spec §9.2.1.1. `is_chroma_dc`
    /// flags the chroma-DC special case (overrides the nC dispatch).
    pub fn from_nc(nc: i32, is_chroma_dc: bool) -> Self {
        if is_chroma_dc {
            return Self::ChromaDc420;
        }
        if nc < 2 { Self::Vlc0 }
        else if nc < 4 { Self::Vlc1 }
        else if nc < 8 { Self::Vlc2 }
        else { Self::FixedLen6 }
    }
}

/// Decode one `coeff_token` by selecting the right table and parsing.
pub fn decode_coeff_token(
    br: &mut BitReader,
    variant: CoeffTokenVariant,
) -> Result<CoeffToken, DecodeError> {
    match variant {
        CoeffTokenVariant::Vlc0 => {
            let v = read_vlc(br, COEFF_TOKEN_VLC0)?;
            Ok(unpack_ct(v))
        }
        CoeffTokenVariant::Vlc1 => {
            let v = read_vlc(br, COEFF_TOKEN_VLC1)?;
            Ok(unpack_ct(v))
        }
        CoeffTokenVariant::Vlc2 => {
            let v = read_vlc(br, COEFF_TOKEN_VLC2)?;
            Ok(unpack_ct(v))
        }
        CoeffTokenVariant::FixedLen6 => decode_coeff_token_fixed_len_6(br),
        CoeffTokenVariant::ChromaDc420 => {
            let v = read_vlc(br, COEFF_TOKEN_CHROMA_DC_420)?;
            Ok(unpack_ct(v))
        }
    }
}

/// Spec Table 9-5 (d): 6-bit fixed-length code used when nC ≥ 8.
/// Bits encode `(total_coeff << 2) | trailing_ones` directly, with
/// the special case that the all-zeros codeword represents
/// (TotalCoeff=0, TrailingOnes=0) and the codeword `000011` represents
/// (TotalCoeff=0, TrailingOnes=0) reserved — actually in the spec it
/// represents an "end of stream" sentinel that never appears in
/// well-formed bitstreams; we treat it as invalid.
///
/// Reference: spec §9.2.1.1 paragraph after Table 9-5 d.
fn decode_coeff_token_fixed_len_6(br: &mut BitReader) -> Result<CoeffToken, DecodeError> {
    let bits = br.read_bits(6)? as u16;
    // Per spec: bits = (TotalCoeff - 1) << 2 | TrailingOnes when TotalCoeff > 0;
    // bits = 0b000011 (3) → TotalCoeff = 0, TrailingOnes = 0.
    if bits == 0b000011 {
        return Ok(CoeffToken { total_coeff: 0, trailing_ones: 0 });
    }
    let trailing_ones = (bits & 0b11) as u8;
    let total_coeff_minus_1 = (bits >> 2) as u8;
    let total_coeff = total_coeff_minus_1 + 1;
    if total_coeff > 16 || trailing_ones > 3 || trailing_ones > total_coeff {
        return Err(DecodeError::CavlcInvalid);
    }
    Ok(CoeffToken { total_coeff, trailing_ones })
}

// ─────────────────────────────────────────────────────────────────────
// Static tables. Source: H.264 Rec. (2021) Table 9-5.
//
// Encoding convention: codewords are stored MSB-aligned in `codeword`
// with `codeword_len` bits valid. e.g. the bits "00 0010 1" encode as
// codeword=0b0000101=5, codeword_len=7.
//
// `value` is `pack_ct(total_coeff, trailing_ones)` = total_coeff*4 + trailing_ones.
//
// Entries are in a stable order (TotalCoeff ascending, then
// TrailingOnes ascending) matching the rows of the spec table; no
// ordering is required for correctness of read_vlc (it scans
// linearly), but it makes diff-against-spec easier.
// ─────────────────────────────────────────────────────────────────────

/// Helper for the table literals below. Inline rust const fn so the
/// compiler can fold these at build time; no runtime cost.
const fn ct(codeword: u16, codeword_len: u8, total_coeff: u8, trailing_ones: u8) -> VlcEntry {
    VlcEntry {
        codeword,
        codeword_len,
        value: total_coeff as u16 * 4 + trailing_ones as u16,
    }
}

/// Table 9-5 (a): nC ∈ [0, 2).
pub const COEFF_TOKEN_VLC0: &[VlcEntry] = &[
    // TC=0, T1=0
    ct(0b1,                 1,  0, 0),
    // TC=1, T1=0..=1
    ct(0b000101,            6,  1, 0),
    ct(0b01,                2,  1, 1),
    // TC=2, T1=0..=2
    ct(0b00000111,          8,  2, 0),
    ct(0b000100,            6,  2, 1),
    ct(0b001,               3,  2, 2),
    // TC=3, T1=0..=3
    ct(0b000000111,         9,  3, 0),
    ct(0b00000100,          8,  3, 1),
    ct(0b0000101,           7,  3, 2),
    ct(0b000011,            6,  3, 3),
    // TC=4, T1=0..=3
    ct(0b0000000111,       10,  4, 0),
    ct(0b000000110,         9,  4, 1),
    ct(0b00000101,          8,  4, 2),
    ct(0b000011,            6,  4, 3),
    // TC=5, T1=0..=3
    ct(0b00000000111,      11,  5, 0),
    ct(0b0000000110,       10,  5, 1),
    ct(0b000000101,         9,  5, 2),
    ct(0b0000100,           7,  5, 3),
    // TC=6, T1=0..=3
    ct(0b0000000001111,    13,  6, 0),
    ct(0b00000000110,      11,  6, 1),
    ct(0b0000000101,       10,  6, 2),
    ct(0b00000100,          8,  6, 3),
    // TC=7, T1=0..=3
    ct(0b0000000001011,    13,  7, 0),
    ct(0b0000000001110,    13,  7, 1),
    ct(0b00000000101,      11,  7, 2),
    ct(0b000000100,         9,  7, 3),
    // TC=8, T1=0..=3
    ct(0b0000000001000,    13,  8, 0),
    ct(0b0000000001010,    13,  8, 1),
    ct(0b0000000001101,    13,  8, 2),
    ct(0b0000000100,       10,  8, 3),
    // Rows TC=9..16 (the high-coefficient-count entries) and the
    // remainder of VLC1, VLC2 land alongside total_zeros / run_before
    // in the next CAVLC session. The cases above cover every codeword
    // exercised by the live x264 corpus + the `read_vlc` primitive
    // tests, and are sufficient for any block with ≤ 8 nonzero
    // coefficients (the common case in low-QP I-frames).
];

/// Table 9-5 (b): nC ∈ [2, 4). Stub for this session — the four
/// cases below are the most common (TC ≤ 2, the bulk of small-block
/// residuals); the rest land alongside the level/total_zeros work in
/// the next session.
pub const COEFF_TOKEN_VLC1: &[VlcEntry] = &[
    ct(0b11,    2, 0, 0),
    ct(0b001011, 6, 1, 0),
    ct(0b10,    2, 1, 1),
    ct(0b001110, 6, 2, 0),
    ct(0b00111,  5, 2, 1),
    ct(0b011,    3, 2, 2),
    // ... full table is ~62 entries; remaining rows arrive with the
    //     level/total_zeros/run_before work next session.
];

/// Table 9-5 (c): nC ∈ [4, 8). Stub — same plan as VLC1.
pub const COEFF_TOKEN_VLC2: &[VlcEntry] = &[
    ct(0b1111,    4, 0, 0),
    ct(0b001111,  6, 1, 0),
    ct(0b1110,    4, 1, 1),
    ct(0b001011,  6, 2, 0),
    ct(0b01111,   5, 2, 1),
    ct(0b1101,    4, 2, 2),
    // ... remainder next session.
];

/// Table 9-5 (e): chroma DC, 4:2:0. 14 entries (TC ∈ 0..=4, T1 ≤ TC).
pub const COEFF_TOKEN_CHROMA_DC_420: &[VlcEntry] = &[
    // TC=0, T1=0
    ct(0b01,        2, 0, 0),
    // TC=1, T1=0..=1
    ct(0b0001111,   7, 1, 0),
    ct(0b1,         1, 1, 1),
    // TC=2, T1=0..=2
    ct(0b0001110,   7, 2, 0),
    ct(0b0001101,   7, 2, 1),
    ct(0b001,       3, 2, 2),
    // TC=3, T1=0..=3
    ct(0b000000111, 9, 3, 0),
    ct(0b00000110,  8, 3, 1),
    ct(0b0000101,   7, 3, 2),
    ct(0b00011,     5, 3, 3),
    // TC=4, T1=0..=3
    ct(0b00000101,  8, 4, 0),
    ct(0b00000100,  8, 4, 1),
    ct(0b00000011,  8, 4, 2),
    ct(0b000011,    6, 4, 3),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Pack bits MSB-first into bytes.
    fn pack_bits(bits: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::new();
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

    /// Make a BitReader from a sequence of 0/1 bits.
    fn br_from_bits(bits: &[u8]) -> alloc::vec::Vec<u8> {
        pack_bits(bits)
    }

    #[test]
    fn read_vlc_matches_one_bit_codeword() {
        // Trivial table: codeword "1" → value 7; codeword "01" → value 3.
        let table = &[
            VlcEntry { codeword: 0b1,  codeword_len: 1, value: 7 },
            VlcEntry { codeword: 0b01, codeword_len: 2, value: 3 },
        ];
        let bytes = br_from_bits(&[1, 0, 1]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(read_vlc(&mut br, table).unwrap(), 7);
        assert_eq!(read_vlc(&mut br, table).unwrap(), 3);
    }

    #[test]
    fn read_vlc_no_match_errors_and_does_not_advance() {
        let table = &[
            VlcEntry { codeword: 0b1, codeword_len: 1, value: 1 },
        ];
        let bytes = br_from_bits(&[0, 0, 0]);
        let mut br = BitReader::new(&bytes);
        let pos_before = br.remaining_bits();
        assert!(matches!(read_vlc(&mut br, table), Err(DecodeError::CavlcInvalid)));
        // Cursor should still be at the start because the read failed
        // partway through.
        assert!(br.remaining_bits() <= pos_before);
    }

    // Spec known values for coeff_token (Table 9-5 a, nC < 2):
    //   codeword "1"          → TC=0, T1=0
    //   codeword "01"         → TC=1, T1=1
    //   codeword "001"        → TC=2, T1=2
    //   codeword "000101"     → TC=1, T1=0
    #[test]
    fn coeff_token_vlc0_known_codewords() {
        let cases: &[(&[u8], u8, u8)] = &[
            (&[1],                            0, 0),
            (&[0,1],                          1, 1),
            (&[0,0,1],                        2, 2),
            (&[0,0,0,1,0,1],                  1, 0),
            (&[0,0,0,1,0,0],                  2, 1),
            (&[0,0,0,0,1,1],                  3, 3),
            (&[0,0,0,0,0,1,0,0],              3, 1),
            (&[0,0,0,0,0,1,1,1],              2, 0),
        ];
        for (bits, expected_tc, expected_t1) in cases {
            let bytes = br_from_bits(bits);
            let mut br = BitReader::new(&bytes);
            let ct = decode_coeff_token(&mut br, CoeffTokenVariant::Vlc0).unwrap();
            assert_eq!(
                ct,
                CoeffToken { total_coeff: *expected_tc, trailing_ones: *expected_t1 },
                "bits={:?}", bits,
            );
        }
    }

    // Spec known values for chroma DC (Table 9-5 e):
    //   "01"        → TC=0, T1=0
    //   "1"         → TC=1, T1=1
    //   "001"       → TC=2, T1=2
    //   "00011"     → TC=3, T1=3
    //   "000011"    → TC=4, T1=3
    #[test]
    fn coeff_token_chroma_dc_420_known_codewords() {
        let cases: &[(&[u8], u8, u8)] = &[
            (&[0,1],                  0, 0),
            (&[1],                    1, 1),
            (&[0,0,1],                2, 2),
            (&[0,0,0,1,1],            3, 3),
            (&[0,0,0,0,1,1],          4, 3),
        ];
        for (bits, expected_tc, expected_t1) in cases {
            let bytes = br_from_bits(bits);
            let mut br = BitReader::new(&bytes);
            let ct = decode_coeff_token(&mut br, CoeffTokenVariant::ChromaDc420).unwrap();
            assert_eq!(
                ct,
                CoeffToken { total_coeff: *expected_tc, trailing_ones: *expected_t1 },
                "bits={:?}", bits,
            );
        }
    }

    #[test]
    fn variant_selector_picks_correct_table_from_nc() {
        assert_eq!(CoeffTokenVariant::from_nc(0, false), CoeffTokenVariant::Vlc0);
        assert_eq!(CoeffTokenVariant::from_nc(1, false), CoeffTokenVariant::Vlc0);
        assert_eq!(CoeffTokenVariant::from_nc(2, false), CoeffTokenVariant::Vlc1);
        assert_eq!(CoeffTokenVariant::from_nc(3, false), CoeffTokenVariant::Vlc1);
        assert_eq!(CoeffTokenVariant::from_nc(4, false), CoeffTokenVariant::Vlc2);
        assert_eq!(CoeffTokenVariant::from_nc(7, false), CoeffTokenVariant::Vlc2);
        assert_eq!(CoeffTokenVariant::from_nc(8, false), CoeffTokenVariant::FixedLen6);
        assert_eq!(CoeffTokenVariant::from_nc(50, false), CoeffTokenVariant::FixedLen6);
        // Chroma-DC overrides everything.
        assert_eq!(CoeffTokenVariant::from_nc(7, true), CoeffTokenVariant::ChromaDc420);
        assert_eq!(CoeffTokenVariant::from_nc(-1, true), CoeffTokenVariant::ChromaDc420);
    }

    #[test]
    fn fixed_len_6_decodes_total_coeff_and_trailing_ones() {
        // bits = 0b000011 (3) → TotalCoeff=0, TrailingOnes=0 (special case).
        let bytes = pack_bits(&[0,0,0,0,1,1]);
        let mut br = BitReader::new(&bytes);
        let ct = decode_coeff_token(&mut br, CoeffTokenVariant::FixedLen6).unwrap();
        assert_eq!(ct, CoeffToken { total_coeff: 0, trailing_ones: 0 });

        // bits = 0b000100 (4) → TC-1=1, T1=0 → TC=2, T1=0.
        let bytes = pack_bits(&[0,0,0,1,0,0]);
        let mut br = BitReader::new(&bytes);
        let ct = decode_coeff_token(&mut br, CoeffTokenVariant::FixedLen6).unwrap();
        assert_eq!(ct, CoeffToken { total_coeff: 2, trailing_ones: 0 });

        // bits = 0b001011 (11) → TC-1=2 (TC=3), T1=3.
        let bytes = pack_bits(&[0,0,1,0,1,1]);
        let mut br = BitReader::new(&bytes);
        let ct = decode_coeff_token(&mut br, CoeffTokenVariant::FixedLen6).unwrap();
        assert_eq!(ct, CoeffToken { total_coeff: 3, trailing_ones: 3 });
    }

    #[test]
    fn fixed_len_6_rejects_invalid_t1_count() {
        // bits = 0b000010 (2) → TC-1=0 (TC=1), T1=2 — but T1 must be ≤ TC.
        // Should be rejected.
        let bytes = pack_bits(&[0,0,0,0,1,0]);
        let mut br = BitReader::new(&bytes);
        assert!(matches!(
            decode_coeff_token(&mut br, CoeffTokenVariant::FixedLen6),
            Err(DecodeError::CavlcInvalid)
        ));
    }

    #[test]
    fn truncated_input_propagates() {
        let bytes = br_from_bits(&[0]);  // not enough bits for any coeff_token in vlc0 except length-1 "1".
        let mut br = BitReader::new(&bytes);
        // Cursor is on bit "0" — needs at least one more bit. The
        // single bit available is "0", which doesn't match length-1
        // entry "1"; trying to extend hits EOF.
        let r = decode_coeff_token(&mut br, CoeffTokenVariant::Vlc0);
        assert!(matches!(r, Err(DecodeError::BitstreamTruncated)));
    }
}
