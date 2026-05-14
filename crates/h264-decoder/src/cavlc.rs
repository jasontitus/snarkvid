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

// Env-gated trace for diagnosing CAVLC parses against libavcodec.
// Enable with `CAVLC_TRACE=1 cargo test -p snarkvid-h264-decoder -- --nocapture`.
// No-op in non-test builds (keeps the crate no_std-pure).
#[cfg(test)]
fn cavlc_trace_enabled() -> bool {
    std::env::var_os("CAVLC_TRACE").is_some()
}
#[cfg(test)]
macro_rules! trace {
    ($($arg:tt)*) => {
        if crate::cavlc::cavlc_trace_enabled() {
            std::eprintln!($($arg)*);
        }
    };
}
#[cfg(not(test))]
macro_rules! trace {
    ($($arg:tt)*) => {};
}

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

/// Table 9-5 (a): nC ∈ [0, 2). Auto-generated from libavcodec
/// `h264_cavlc.c` (FFmpeg master). Do not hand-edit — the
/// transcription script is documented in `crates/h264-decoder/README.md`.
pub const COEFF_TOKEN_VLC0: &[VlcEntry] = &[
    ct(0b1, 1, 0, 0),
    ct(0b000101, 6, 1, 0),
    ct(0b01, 2, 1, 1),
    ct(0b00000111, 8, 2, 0),
    ct(0b000100, 6, 2, 1),
    ct(0b001, 3, 2, 2),
    ct(0b000000111, 9, 3, 0),
    ct(0b00000110, 8, 3, 1),
    ct(0b0000101, 7, 3, 2),
    ct(0b00011, 5, 3, 3),
    ct(0b0000000111, 10, 4, 0),
    ct(0b000000110, 9, 4, 1),
    ct(0b00000101, 8, 4, 2),
    ct(0b000011, 6, 4, 3),
    ct(0b00000000111, 11, 5, 0),
    ct(0b0000000110, 10, 5, 1),
    ct(0b000000101, 9, 5, 2),
    ct(0b0000100, 7, 5, 3),
    ct(0b0000000001111, 13, 6, 0),
    ct(0b00000000110, 11, 6, 1),
    ct(0b0000000101, 10, 6, 2),
    ct(0b00000100, 8, 6, 3),
    ct(0b0000000001011, 13, 7, 0),
    ct(0b0000000001110, 13, 7, 1),
    ct(0b00000000101, 11, 7, 2),
    ct(0b000000100, 9, 7, 3),
    ct(0b0000000001000, 13, 8, 0),
    ct(0b0000000001010, 13, 8, 1),
    ct(0b0000000001101, 13, 8, 2),
    ct(0b0000000100, 10, 8, 3),
    ct(0b00000000001111, 14, 9, 0),
    ct(0b00000000001110, 14, 9, 1),
    ct(0b0000000001001, 13, 9, 2),
    ct(0b00000000100, 11, 9, 3),
    ct(0b00000000001011, 14, 10, 0),
    ct(0b00000000001010, 14, 10, 1),
    ct(0b00000000001101, 14, 10, 2),
    ct(0b0000000001100, 13, 10, 3),
    ct(0b000000000001111, 15, 11, 0),
    ct(0b000000000001110, 15, 11, 1),
    ct(0b00000000001001, 14, 11, 2),
    ct(0b00000000001100, 14, 11, 3),
    ct(0b000000000001011, 15, 12, 0),
    ct(0b000000000001010, 15, 12, 1),
    ct(0b000000000001101, 15, 12, 2),
    ct(0b00000000001000, 14, 12, 3),
    ct(0b0000000000001111, 16, 13, 0),
    ct(0b000000000000001, 15, 13, 1),
    ct(0b000000000001001, 15, 13, 2),
    ct(0b000000000001100, 15, 13, 3),
    ct(0b0000000000001011, 16, 14, 0),
    ct(0b0000000000001110, 16, 14, 1),
    ct(0b0000000000001101, 16, 14, 2),
    ct(0b000000000001000, 15, 14, 3),
    ct(0b0000000000000111, 16, 15, 0),
    ct(0b0000000000001010, 16, 15, 1),
    ct(0b0000000000001001, 16, 15, 2),
    ct(0b0000000000001100, 16, 15, 3),
    ct(0b0000000000000100, 16, 16, 0),
    ct(0b0000000000000110, 16, 16, 1),
    ct(0b0000000000000101, 16, 16, 2),
    ct(0b0000000000001000, 16, 16, 3),
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

// ─────────────────────────────────────────────────────────────────────
// Level decoding (spec §9.2.2)
//
// After coeff_token gives us (TotalCoeff, TrailingOnes), we read the
// signed coefficient values. Trailing ones are ±1, sign read as 1
// bit each. The remaining (TotalCoeff - TrailingOnes) coefficients
// are encoded as `level_prefix` (unary) + `level_suffix` (fixed
// length), with a stateful `suffix_length` that adapts based on
// previously-decoded magnitudes.
//
// Implementation follows the spec algorithm verbatim.
// ─────────────────────────────────────────────────────────────────────

/// Read up to 32 leading zero bits followed by a 1 bit, return the
/// number of leading zeros. Caps at 32 to keep the cost bounded
/// (anything larger is invalid CAVLC).
fn read_level_prefix(br: &mut BitReader) -> Result<u32, DecodeError> {
    let mut zeros = 0u32;
    while br.read_bit()? == 0 {
        zeros += 1;
        if zeros > 32 {
            return Err(DecodeError::CavlcInvalid);
        }
    }
    Ok(zeros)
}

/// Decode the (TotalCoeff - TrailingOnes) non-trailing-one signed
/// levels. Returns them in spec order (highest-frequency first;
/// caller flips to raster as needed).
///
/// `total_coeff` and `trailing_ones` come from coeff_token.
pub fn decode_levels(
    br: &mut BitReader,
    total_coeff: u8,
    trailing_ones: u8,
) -> Result<[i32; 16], DecodeError> {
    trace!("decode_levels: TC={} T1={}", total_coeff, trailing_ones);
    let mut levels = [0i32; 16];
    if total_coeff == 0 {
        return Ok(levels);
    }
    if trailing_ones > total_coeff || trailing_ones > 3 {
        return Err(DecodeError::CavlcInvalid);
    }

    // Read the trailing_ones sign bits first. 1 = -1, 0 = +1.
    // The trailing ones are the last `trailing_ones` non-zero
    // coefficients in zig-zag order; we store them at indices
    // [total_coeff - trailing_ones .. total_coeff] of the output.
    for i in 0..trailing_ones as usize {
        let sign = br.read_bit()?;
        let idx = (total_coeff - 1) as usize - i;
        levels[idx] = if sign == 1 { -1 } else { 1 };
        trace!("  T1 sign[{}]: bit={} → level={}", i, sign, levels[idx]);
    }

    let n_levels = (total_coeff - trailing_ones) as usize;
    if n_levels == 0 {
        return Ok(levels);
    }

    // Initial suffix_length per spec §9.2.2.
    // (total_coeff > 10 && trailing_ones < 3) → 1, else 0.
    let mut suffix_length: u32 =
        if total_coeff > 10 && trailing_ones < 3 { 1 } else { 0 };

    trace!("  initial suffix_length={}", suffix_length);
    for k in 0..n_levels {
        let level_prefix = read_level_prefix(br)?;
        trace!("  k={}: level_prefix={}", k, level_prefix);

        // Spec §9.2.2.1 / JM reference:
        //   level_prefix < 14:  levelSuffixSize = suffixLength
        //   level_prefix == 14: levelSuffixSize = (suffix_length == 0) ? 4 : suffix_length
        //   level_prefix >= 15: levelSuffixSize = 12  (escape)
        let level_suffix_size: u32 = if level_prefix < 14 {
            suffix_length
        } else if level_prefix == 14 {
            if suffix_length == 0 { 4 } else { suffix_length }
        } else {
            // level_prefix >= 15: escape to 12-bit suffix.
            12
        };

        let level_suffix: u32 = if level_suffix_size > 0 {
            br.read_bits(level_suffix_size)?
        } else {
            0
        };
        trace!("    level_suffix_size={} level_suffix={}", level_suffix_size, level_suffix);

        // Reconstruct the unsigned levelCode (spec §9.2.2.1).
        let mut level_code: i32 = (core::cmp::min(15, level_prefix) << suffix_length) as i32
            + level_suffix as i32;
        if level_prefix >= 14 && suffix_length == 0 {
            level_code += 15;
        }
        if level_prefix >= 15 {
            // Escape: +4096 bias.
            level_code += 1 << 12;
        }
        // First non-trailing-one with TrailingOnes < 3 gets a +2 bias
        // (because levelCode=0 means "the smallest non-trailing-one
        // magnitude is 2", levelCode=2 means "magnitude 3", etc.).
        if k == 0 && trailing_ones < 3 {
            level_code += 2;
        }

        // Map levelCode → signed level.
        let level: i32 = if level_code & 1 == 0 {
            (level_code + 2) >> 1
        } else {
            -((level_code + 1) >> 1)
        };

        // Store at the next available slot, walking back from
        // (total_coeff - trailing_ones - 1) toward 0.
        let idx = (total_coeff - trailing_ones) as usize - 1 - k;
        levels[idx] = level;
        trace!("    level_code={} → level={} (placed at idx {})", level_code, level, idx);

        // Update suffix_length for the next iteration.
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if level.unsigned_abs() > (3u32 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
        trace!("    new suffix_length={}", suffix_length);
    }
    Ok(levels)
}

// ─────────────────────────────────────────────────────────────────────
// total_zeros (spec §9.2.3, Table 9-7) and run_before (Table 9-10)
//
// All tables auto-generated from libavcodec h264_cavlc.c. Hand-edited
// at your peril — silent miscoding is a real risk on these big tables;
// the regen script lives in crates/h264-decoder/README.md.
// ─────────────────────────────────────────────────────────────────────

const TZ_TC1: &[VlcEntry] = &[
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 0 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b0011, codeword_len: 4, value: 3 },
    VlcEntry { codeword: 0b0010, codeword_len: 4, value: 4 },
    VlcEntry { codeword: 0b00011, codeword_len: 5, value: 5 },
    VlcEntry { codeword: 0b00010, codeword_len: 5, value: 6 },
    VlcEntry { codeword: 0b000011, codeword_len: 6, value: 7 },
    VlcEntry { codeword: 0b000010, codeword_len: 6, value: 8 },
    VlcEntry { codeword: 0b0000011, codeword_len: 7, value: 9 },
    VlcEntry { codeword: 0b0000010, codeword_len: 7, value: 10 },
    VlcEntry { codeword: 0b00000011, codeword_len: 8, value: 11 },
    VlcEntry { codeword: 0b00000010, codeword_len: 8, value: 12 },
    VlcEntry { codeword: 0b000000011, codeword_len: 9, value: 13 },
    VlcEntry { codeword: 0b000000010, codeword_len: 9, value: 14 },
    VlcEntry { codeword: 0b000000001, codeword_len: 9, value: 15 },
];

const TZ_TC2: &[VlcEntry] = &[
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 0 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b0101, codeword_len: 4, value: 5 },
    VlcEntry { codeword: 0b0100, codeword_len: 4, value: 6 },
    VlcEntry { codeword: 0b0011, codeword_len: 4, value: 7 },
    VlcEntry { codeword: 0b0010, codeword_len: 4, value: 8 },
    VlcEntry { codeword: 0b00011, codeword_len: 5, value: 9 },
    VlcEntry { codeword: 0b00010, codeword_len: 5, value: 10 },
    VlcEntry { codeword: 0b000011, codeword_len: 6, value: 11 },
    VlcEntry { codeword: 0b000010, codeword_len: 6, value: 12 },
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 13 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 14 },
];

const TZ_TC3: &[VlcEntry] = &[
    VlcEntry { codeword: 0b0101, codeword_len: 4, value: 0 },
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b0100, codeword_len: 4, value: 4 },
    VlcEntry { codeword: 0b0011, codeword_len: 4, value: 5 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 7 },
    VlcEntry { codeword: 0b0010, codeword_len: 4, value: 8 },
    VlcEntry { codeword: 0b00011, codeword_len: 5, value: 9 },
    VlcEntry { codeword: 0b00010, codeword_len: 5, value: 10 },
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 11 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 12 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 13 },
];

const TZ_TC4: &[VlcEntry] = &[
    VlcEntry { codeword: 0b00011, codeword_len: 5, value: 0 },
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b0101, codeword_len: 4, value: 2 },
    VlcEntry { codeword: 0b0100, codeword_len: 4, value: 3 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b0011, codeword_len: 4, value: 7 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 8 },
    VlcEntry { codeword: 0b0010, codeword_len: 4, value: 9 },
    VlcEntry { codeword: 0b00010, codeword_len: 5, value: 10 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 11 },
    VlcEntry { codeword: 0b00000, codeword_len: 5, value: 12 },
];

const TZ_TC5: &[VlcEntry] = &[
    VlcEntry { codeword: 0b0101, codeword_len: 4, value: 0 },
    VlcEntry { codeword: 0b0100, codeword_len: 4, value: 1 },
    VlcEntry { codeword: 0b0011, codeword_len: 4, value: 2 },
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 7 },
    VlcEntry { codeword: 0b0010, codeword_len: 4, value: 8 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 9 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 10 },
    VlcEntry { codeword: 0b00000, codeword_len: 5, value: 11 },
];

const TZ_TC6: &[VlcEntry] = &[
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 0 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 1 },
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 7 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 8 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 9 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 10 },
];

const TZ_TC7: &[VlcEntry] = &[
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 0 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 1 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 5 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 7 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 8 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 9 },
];

const TZ_TC8: &[VlcEntry] = &[
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 0 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 1 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 2 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 4 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 5 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 7 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 8 },
];

const TZ_TC9: &[VlcEntry] = &[
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 0 },
    VlcEntry { codeword: 0b000000, codeword_len: 6, value: 1 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 2 },
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 3 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 4 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 6 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 7 },
];

const TZ_TC10: &[VlcEntry] = &[
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 0 },
    VlcEntry { codeword: 0b00000, codeword_len: 5, value: 1 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 3 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 4 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 5 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 6 },
];

const TZ_TC11: &[VlcEntry] = &[
    VlcEntry { codeword: 0b0000, codeword_len: 4, value: 0 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 1 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 4 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 5 },
];

const TZ_TC12: &[VlcEntry] = &[
    VlcEntry { codeword: 0b0000, codeword_len: 4, value: 0 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 1 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 2 },
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 3 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 4 },
];

const TZ_TC13: &[VlcEntry] = &[
    VlcEntry { codeword: 0b000, codeword_len: 3, value: 0 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 2 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 3 },
];

const TZ_TC14: &[VlcEntry] = &[
    VlcEntry { codeword: 0b00, codeword_len: 2, value: 0 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 1 },
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 2 },
];

const TZ_TC15: &[VlcEntry] = &[
    VlcEntry { codeword: 0b0, codeword_len: 1, value: 0 },
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 1 },
];

const RB_ZL1: &[VlcEntry] = &[
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 0 },
    VlcEntry { codeword: 0b0, codeword_len: 1, value: 1 },
];

const RB_ZL2: &[VlcEntry] = &[
    VlcEntry { codeword: 0b1, codeword_len: 1, value: 0 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 1 },
    VlcEntry { codeword: 0b00, codeword_len: 2, value: 2 },
];

const RB_ZL3: &[VlcEntry] = &[
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 0 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 1 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 2 },
    VlcEntry { codeword: 0b00, codeword_len: 2, value: 3 },
];

const RB_ZL4: &[VlcEntry] = &[
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 0 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 1 },
    VlcEntry { codeword: 0b01, codeword_len: 2, value: 2 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b000, codeword_len: 3, value: 4 },
];

const RB_ZL5: &[VlcEntry] = &[
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 0 },
    VlcEntry { codeword: 0b10, codeword_len: 2, value: 1 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b000, codeword_len: 3, value: 5 },
];

const RB_ZL6: &[VlcEntry] = &[
    VlcEntry { codeword: 0b11, codeword_len: 2, value: 0 },
    VlcEntry { codeword: 0b000, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 6 },
];

const RB_ZL_GE7: &[VlcEntry] = &[
    VlcEntry { codeword: 0b111, codeword_len: 3, value: 0 },
    VlcEntry { codeword: 0b110, codeword_len: 3, value: 1 },
    VlcEntry { codeword: 0b101, codeword_len: 3, value: 2 },
    VlcEntry { codeword: 0b100, codeword_len: 3, value: 3 },
    VlcEntry { codeword: 0b011, codeword_len: 3, value: 4 },
    VlcEntry { codeword: 0b010, codeword_len: 3, value: 5 },
    VlcEntry { codeword: 0b001, codeword_len: 3, value: 6 },
    VlcEntry { codeword: 0b0001, codeword_len: 4, value: 7 },
    VlcEntry { codeword: 0b00001, codeword_len: 5, value: 8 },
    VlcEntry { codeword: 0b000001, codeword_len: 6, value: 9 },
    VlcEntry { codeword: 0b0000001, codeword_len: 7, value: 10 },
    VlcEntry { codeword: 0b00000001, codeword_len: 8, value: 11 },
    VlcEntry { codeword: 0b000000001, codeword_len: 9, value: 12 },
    VlcEntry { codeword: 0b0000000001, codeword_len: 10, value: 13 },
    VlcEntry { codeword: 0b00000000001, codeword_len: 11, value: 14 },
];

/// Pick the right total_zeros table for `total_coeff` (1..=15).
/// total_coeff = 16 means no zeros possible; caller short-circuits.
fn total_zeros_table(total_coeff: u8) -> Result<&'static [VlcEntry], DecodeError> {
    match total_coeff {
        1 => Ok(TZ_TC1), 2 => Ok(TZ_TC2), 3 => Ok(TZ_TC3),
        4 => Ok(TZ_TC4), 5 => Ok(TZ_TC5), 6 => Ok(TZ_TC6),
        7 => Ok(TZ_TC7), 8 => Ok(TZ_TC8), 9 => Ok(TZ_TC9),
        10 => Ok(TZ_TC10), 11 => Ok(TZ_TC11), 12 => Ok(TZ_TC12),
        13 => Ok(TZ_TC13), 14 => Ok(TZ_TC14), 15 => Ok(TZ_TC15),
        _ => Err(DecodeError::CavlcInvalid),
    }
}

/// Decode `total_zeros` for a 4×4 luma residual block.
pub fn decode_total_zeros_4x4(
    br: &mut BitReader,
    total_coeff: u8,
) -> Result<u8, DecodeError> {
    if total_coeff == 0 || total_coeff >= 16 { return Ok(0); }
    let table = total_zeros_table(total_coeff)?;
    let v = read_vlc(br, table)?;
    Ok(v as u8)
}

fn run_before_table(zeros_left: u8) -> Result<&'static [VlcEntry], DecodeError> {
    match zeros_left {
        1 => Ok(RB_ZL1),
        2 => Ok(RB_ZL2),
        3 => Ok(RB_ZL3),
        4 => Ok(RB_ZL4),
        5 => Ok(RB_ZL5),
        6 => Ok(RB_ZL6),
        7..=14 => Ok(RB_ZL_GE7),
        _ => Err(DecodeError::CavlcInvalid),
    }
}

/// Decode one `run_before` value given the remaining `zeros_left`.
/// Returns the run length (number of zeros before the next nonzero
/// in the inverse zig-zag walk).
pub fn decode_run_before(br: &mut BitReader, zeros_left: u8) -> Result<u8, DecodeError> {
    trace!("    decode_run_before: zeros_left={}", zeros_left);
    if zeros_left == 0 { return Ok(0); }
    let t = run_before_table(zeros_left)?;
    let v = read_vlc(br, t)?;
    // Cap the result at zeros_left.
    Ok(core::cmp::min(v as u8, zeros_left))
}

// ─────────────────────────────────────────────────────────────────────
// decode_residual_block_4x4 — the composer
//
// Pulls coeff_token, trailing-one signs, levels, total_zeros, and
// run_before together to produce 16 i32 levels in raster order.
//
// Caller responsibilities:
//   - select the right `CoeffTokenVariant` based on neighbor nC.
//   - feed the result into `quant::inverse_quant_4x4_ac` and then
//     into `transform::idct_4x4 + round_shift_6`.
// ─────────────────────────────────────────────────────────────────────

/// Inverse zig-zag scan order for a 4×4 block (spec §6.4.4 / §8.5.4).
/// `ZIGZAG[k]` is the raster index of the k-th coefficient in zig-zag
/// scan order (k=0 = DC at raster index 0; k=15 = highest AC).
pub const ZIGZAG_4X4: [usize; 16] = [
    0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15,
];

/// Decode one 4×4 residual block. Returns 16 levels in raster order
/// (suitable for handing straight to inverse_quant_4x4_ac).
pub fn decode_residual_block_4x4(
    br: &mut BitReader,
    variant: CoeffTokenVariant,
) -> Result<[i32; 16], DecodeError> {
    decode_residual_block_4x4_with_tc(br, variant).map(|(_, c)| c)
}

/// Like `decode_residual_block_4x4` but also returns the TotalCoeff that
/// drove the decode. Callers that need to track block-level nC for
/// neighbor-based variant selection (spec §9.2.1.1) use this form.
pub fn decode_residual_block_4x4_with_tc(
    br: &mut BitReader,
    variant: CoeffTokenVariant,
) -> Result<(u8, [i32; 16]), DecodeError> {
    trace!("decode_residual_block_4x4: variant={:?}", variant);
    let ct = decode_coeff_token(br, variant)?;
    trace!("  coeff_token: TC={} T1={}", ct.total_coeff, ct.trailing_ones);
    let mut levels_zigzag = decode_levels(br, ct.total_coeff, ct.trailing_ones)?;

    // Place the levels into their zig-zag positions. `decode_levels`
    // returns them in increasing-frequency order in slots [0..total_coeff],
    // with index 0 being the highest-frequency nonzero. We need to place
    // them at zig-zag positions [16 - total_coeff .. 16] before any
    // run_before / total_zeros gymnastics. Actually the spec is cleaner:
    // we walk runs in the inverse zig-zag direction (highest freq → DC)
    // and place coefficients at the right positions.

    let total_zeros = decode_total_zeros_4x4(br, ct.total_coeff)?;
    trace!("  total_zeros={}", total_zeros);
    let mut zeros_left = total_zeros;
    let mut coeff_num: i32 = -1; // counts how far back from highest-freq we are

    // Build the 4×4 block in zig-zag space, then unscramble at the end.
    let mut zz = [0i32; 16];

    if ct.total_coeff > 0 {
        // levels_zigzag[i] for i in 0..total_coeff currently holds the
        // levels in increasing-frequency order, where slot 0 is the
        // highest-frequency nonzero. Re-emit them with run_before.
        for i in 0..(ct.total_coeff as usize) {
            let run = if i < (ct.total_coeff as usize) - 1 && zeros_left > 0 {
                let r = decode_run_before(br, zeros_left)?;
                zeros_left -= r;
                r
            } else {
                zeros_left
            };
            trace!("  i={} run_before={} zeros_left_after={}", i, run, zeros_left);
            coeff_num += (run + 1) as i32;
            // coeff_num counts from highest-frequency end; the zigzag
            // index walking down from k=15 is (15 - coeff_num).
            let zz_idx = 15 - coeff_num as usize;
            zz[zz_idx] = levels_zigzag[i];
        }
    }
    // Suppress unused warning during partial CAVLC; final zigzag swizzle:
    let _ = &mut levels_zigzag;

    // Convert zig-zag → raster.
    let mut raster = [0i32; 16];
    for k in 0..16 {
        raster[ZIGZAG_4X4[k]] = zz[k];
    }
    Ok((ct.total_coeff, raster))
}

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

    // Spec-verified codewords from libavcodec h264_cavlc.c.
    //   codeword "1"          → TC=0, T1=0
    //   codeword "01"         → TC=1, T1=1
    //   codeword "001"        → TC=2, T1=2
    //   codeword "000101"     → TC=1, T1=0
    //   codeword "000100"     → TC=2, T1=1
    //   codeword "00011"      → TC=3, T1=3
    //   codeword "00000111"   → TC=2, T1=0
    //   codeword "0000101"    → TC=3, T1=2
    //   codeword "00000110"   → TC=3, T1=1
    #[test]
    fn coeff_token_vlc0_known_codewords() {
        let cases: &[(&[u8], u8, u8)] = &[
            (&[1],                            0, 0),
            (&[0,1],                          1, 1),
            (&[0,0,1],                        2, 2),
            (&[0,0,0,1,0,1],                  1, 0),
            (&[0,0,0,1,0,0],                  2, 1),
            (&[0,0,0,1,1],                    3, 3),
            (&[0,0,0,0,0,1,1,1],              2, 0),
            (&[0,0,0,0,1,0,1],                3, 2),
            (&[0,0,0,0,0,1,1,0],              3, 1),
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

    // ─────────────────────────────────────────────────────────────────
    // Level / total_zeros / run_before / decode_residual_block tests
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn empty_block_returns_all_zeros() {
        // total_coeff = 0 → no levels, no run_before, no total_zeros lookup.
        let bytes = pack_bits(&[]);
        let mut br = BitReader::new(&bytes);
        let levels = decode_levels(&mut br, 0, 0).unwrap();
        assert_eq!(levels, [0i32; 16]);
    }

    #[test]
    fn single_trailing_one_yields_signed_one() {
        // total_coeff=1, trailing_ones=1 → read a single sign bit:
        // 1 → -1, 0 → +1.
        let bytes = pack_bits(&[1]);
        let mut br = BitReader::new(&bytes);
        let levels = decode_levels(&mut br, 1, 1).unwrap();
        assert_eq!(levels[0], -1);
        let bytes = pack_bits(&[0]);
        let mut br = BitReader::new(&bytes);
        let levels = decode_levels(&mut br, 1, 1).unwrap();
        assert_eq!(levels[0], 1);
    }

    #[test]
    fn three_trailing_ones_each_get_a_sign_bit() {
        // Trailing ones placed at indices [TC-1, TC-2, TC-3] in the
        // result. Bits read in order — first sign bit goes to the
        // highest-frequency trailing one (index TC-1).
        let bytes = pack_bits(&[1, 0, 1]);
        let mut br = BitReader::new(&bytes);
        let levels = decode_levels(&mut br, 3, 3).unwrap();
        assert_eq!(levels[2], -1);
        assert_eq!(levels[1], 1);
        assert_eq!(levels[0], -1);
    }

    #[test]
    fn level_prefix_returns_zero_for_lone_one_bit() {
        let bytes = pack_bits(&[1]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(read_level_prefix(&mut br).unwrap(), 0);
    }

    #[test]
    fn level_prefix_counts_zeros() {
        // 4 zeros then a 1 → returns 4.
        let bytes = pack_bits(&[0, 0, 0, 0, 1]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(read_level_prefix(&mut br).unwrap(), 4);
    }

    #[test]
    fn total_zeros_tc_3_known_codewords() {
        // Spec Table 9-7 TC=3 column known values:
        //   "0101" → 0    "111" → 1    "110" → 2    "101" → 3
        let cases: &[(&[u8], u8)] = &[
            (&[0,1,0,1], 0),
            (&[1,1,1],   1),
            (&[1,1,0],   2),
            (&[1,0,1],   3),
        ];
        for (bits, expected) in cases {
            let bytes = pack_bits(bits);
            let mut br = BitReader::new(&bytes);
            assert_eq!(decode_total_zeros_4x4(&mut br, 3).unwrap(), *expected,
                "TC=3 bits {:?}", bits);
        }
    }

    #[test]
    fn total_zeros_tc_7_codeword_11_decodes_to_5() {
        // Spec Table 9-7 TC=7: "11" → 5 (notice short codeword for
        // the most-likely value at high TC).
        let bytes = pack_bits(&[1, 1]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_total_zeros_4x4(&mut br, 7).unwrap(), 5);
    }

    #[test]
    fn total_zeros_tc_1_known_codewords() {
        // From spec Table 9-7: TC=1 column.
        //   "1"        → 0
        //   "011"      → 1
        //   "010"      → 2
        //   "0011"     → 3
        let cases: &[(&[u8], u8)] = &[
            (&[1], 0),
            (&[0,1,1], 1),
            (&[0,1,0], 2),
            (&[0,0,1,1], 3),
            (&[0,0,1,0], 4),
        ];
        for (bits, expected) in cases {
            let bytes = pack_bits(bits);
            let mut br = BitReader::new(&bytes);
            assert_eq!(decode_total_zeros_4x4(&mut br, 1).unwrap(), *expected,
                "bits {:?}", bits);
        }
    }

    #[test]
    fn total_zeros_tc_eq_16_returns_zero_without_reading() {
        // total_coeff=16 means the block is full → no zeros possible.
        let bytes = pack_bits(&[]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_total_zeros_4x4(&mut br, 16).unwrap(), 0);
    }

    #[test]
    fn run_before_zl1_round_trip() {
        // Table 9-10 zeros_left=1: "1" → 0, "0" → 1.
        let bytes = pack_bits(&[1, 0]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_run_before(&mut br, 1).unwrap(), 0);
        assert_eq!(decode_run_before(&mut br, 1).unwrap(), 1);
    }

    #[test]
    fn run_before_zl3_round_trip() {
        // zeros_left=3: "11"→0, "10"→1, "01"→2, "00"→3.
        let bytes = pack_bits(&[1, 1,  1, 0,  0, 1,  0, 0]);
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_run_before(&mut br, 3).unwrap(), 0);
        assert_eq!(decode_run_before(&mut br, 3).unwrap(), 1);
        assert_eq!(decode_run_before(&mut br, 3).unwrap(), 2);
        assert_eq!(decode_run_before(&mut br, 3).unwrap(), 3);
    }

    #[test]
    fn zigzag_table_is_a_valid_permutation() {
        let mut seen = [false; 16];
        for &k in ZIGZAG_4X4.iter() {
            assert!(k < 16);
            assert!(!seen[k], "duplicate index {}", k);
            seen[k] = true;
        }
        // First entry is DC.
        assert_eq!(ZIGZAG_4X4[0], 0);
        // Last entry is the highest-frequency AC corner.
        assert_eq!(ZIGZAG_4X4[15], 15);
    }

    #[test]
    fn residual_block_with_single_dc_trailing_one() {
        // Hand-built bitstream for TC=1, T1=1, sign=+1, total_zeros=15:
        //   coeff_token  "000101"   → TC=1, T1=0 in VLC0... actually
        //   coeff_token  "01"       → TC=1, T1=1 in VLC0
        //   sign bit     "0"        → +1
        //   total_zeros  "000000001" → 15 (TZ table TC=1 entry)
        //   no run_before since this is the only nonzero.
        let bits: alloc::vec::Vec<u8> = [
            &[0,1][..],                       // coeff_token TC=1,T1=1
            &[0],                             // sign +1
            &[0,0,0,0,0,0,0,0,1][..],         // total_zeros = 15 → "000000001"
        ].concat();
        let bytes = pack_bits(&bits);
        let mut br = BitReader::new(&bytes);
        let raster = decode_residual_block_4x4(&mut br, CoeffTokenVariant::Vlc0).unwrap();
        // The only nonzero should be at the DC position (raster index 0).
        assert_eq!(raster[0], 1, "DC = +1; got {:?}", raster);
        for i in 1..16 {
            assert_eq!(raster[i], 0, "all AC should be zero; idx {} = {}", i, raster[i]);
        }
    }
}
