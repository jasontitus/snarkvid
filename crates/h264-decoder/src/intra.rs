// Intra prediction for H.264 baseline I-frames (spec §8.3).
//
// Once a macroblock's residual is decoded (CAVLC → dequant → IDCT),
// the spec adds the residual to a *predicted* block formed from the
// already-decoded neighboring pixels. This module produces those
// predicted blocks.
//
// What's here this session:
//   - All 9 Intra_4×4 modes (spec §8.3.1.2).
//
// What's deferred to a follow-up session:
//   - Intra_16×16 (4 modes; spec §8.3.2)
//   - Chroma 8×8 (4 modes for 4:2:0; spec §8.3.3)
//
// All three families share the same architectural shape: take a
// `NeighborSamples` struct describing what's available + the values,
// dispatch on a mode enum, return a small fixed-size pixel block.
//
// Neighbor availability: a 4×4 block at the top edge of a slice has
// no top neighbors; one at the left edge has no left. Modes that
// require unavailable neighbors are not legal there (the encoder
// won't emit them per spec §8.3.1.1). The DC mode falls back to
// 128 when neither side is available.
//
// no_std-pure.

use crate::DecodeError;

// ─────────────────────────────────────────────────────────────────────
// Intra_4×4 mode enum (spec Table 8-2)
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intra4x4Mode {
    Vertical,
    Horizontal,
    Dc,
    DiagonalDownLeft,
    DiagonalDownRight,
    VerticalRight,
    HorizontalDown,
    VerticalLeft,
    HorizontalUp,
}

impl Intra4x4Mode {
    /// Decode the spec's `Intra4x4PredMode` integer (0..=8).
    pub fn from_index(idx: u8) -> Result<Self, DecodeError> {
        match idx {
            0 => Ok(Self::Vertical),
            1 => Ok(Self::Horizontal),
            2 => Ok(Self::Dc),
            3 => Ok(Self::DiagonalDownLeft),
            4 => Ok(Self::DiagonalDownRight),
            5 => Ok(Self::VerticalRight),
            6 => Ok(Self::HorizontalDown),
            7 => Ok(Self::VerticalLeft),
            8 => Ok(Self::HorizontalUp),
            _ => Err(DecodeError::OutOfScope("Intra4x4 mode out of range")),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Neighbor samples for Intra_4×4
//
// Layout (spec §8.3.1, p, q, etc. names):
//
//         tl   t0  t1  t2  t3   tr0 tr1 tr2 tr3
//     l0  ──── ─── ─── ─── ───  ─── ─── ─── ───
//     l1   |   x00 x01 x02 x03   .   .   .   .
//     l2   |   x10 x11 x12 x13
//     l3   |   x20 x21 x22 x23
//          |   x30 x31 x32 x33
//
// where the `x` 4×4 grid is the block being predicted, and the rest
// are neighbor samples we can read. `tl` is the top-left corner pixel.
// `t0..t3` is the row directly above (4 samples). `tr0..tr3` is the
// row above and to the right (top-right). `l0..l3` is the column
// directly to the left (4 samples).
//
// Some modes (3 = Diagonal_Down_Left, 7 = Vertical_Left) need the
// 4 top-right samples; others don't. Some modes (4, 5, 6) need the
// top-left corner. We carry all of them here; per-mode availability
// requirements are enforced inside the predict functions.
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct Neighbors4x4 {
    pub top_left: Option<u8>,
    pub top: Option<[u8; 4]>,
    pub top_right: Option<[u8; 4]>,
    pub left: Option<[u8; 4]>,
}

impl Neighbors4x4 {
    /// All four neighbors available. Convenience for tests.
    pub fn all(top: [u8; 4], top_right: [u8; 4], left: [u8; 4], top_left: u8) -> Self {
        Self {
            top: Some(top),
            top_right: Some(top_right),
            left: Some(left),
            top_left: Some(top_left),
        }
    }

    /// No neighbors available — top-left corner of frame.
    pub const NONE: Self = Self {
        top: None,
        top_right: None,
        left: None,
        top_left: None,
    };
}

// ─────────────────────────────────────────────────────────────────────
// Predict 4×4 — dispatcher
// ─────────────────────────────────────────────────────────────────────

/// Return the 4×4 predicted block (16 u8 samples in raster order).
/// Errors if the requested mode needs a neighbor that isn't available.
pub fn predict_4x4(mode: Intra4x4Mode, n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    match mode {
        Intra4x4Mode::Vertical          => pred_vertical(n),
        Intra4x4Mode::Horizontal        => pred_horizontal(n),
        Intra4x4Mode::Dc                => pred_dc(n),
        Intra4x4Mode::DiagonalDownLeft  => pred_diag_down_left(n),
        Intra4x4Mode::DiagonalDownRight => pred_diag_down_right(n),
        Intra4x4Mode::VerticalRight     => pred_vertical_right(n),
        Intra4x4Mode::HorizontalDown    => pred_horizontal_down(n),
        Intra4x4Mode::VerticalLeft      => pred_vertical_left(n),
        Intra4x4Mode::HorizontalUp      => pred_horizontal_up(n),
    }
}

#[inline] fn at(out: &mut [u8; 16], x: usize, y: usize, v: u8) { out[y * 4 + x] = v; }

fn err_neighbor_missing(_mode: &str) -> DecodeError {
    DecodeError::OutOfScope("intra4x4: required neighbor unavailable")
}

// ─────────────────────────────────────────────────────────────────────
// Mode 0: Vertical — copy top row down.
// ─────────────────────────────────────────────────────────────────────

fn pred_vertical(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("vertical"))?;
    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            at(&mut out, x, y, t[x]);
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Mode 1: Horizontal — copy left column right.
// ─────────────────────────────────────────────────────────────────────

fn pred_horizontal(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let l = n.left.ok_or_else(|| err_neighbor_missing("horizontal"))?;
    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            at(&mut out, x, y, l[y]);
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Mode 2: DC — mean of available top + left neighbors.
//
//   both: dc = (Σ top + Σ left + 4) >> 3
//   top only: dc = (Σ top + 2) >> 2
//   left only: dc = (Σ left + 2) >> 2
//   neither: dc = 128
// ─────────────────────────────────────────────────────────────────────

fn pred_dc(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let dc: u32 = match (n.top, n.left) {
        (Some(t), Some(l)) => {
            let sum: u32 = t.iter().map(|&x| x as u32).sum::<u32>()
                         + l.iter().map(|&x| x as u32).sum::<u32>();
            (sum + 4) >> 3
        }
        (Some(t), None) => {
            let sum: u32 = t.iter().map(|&x| x as u32).sum();
            (sum + 2) >> 2
        }
        (None, Some(l)) => {
            let sum: u32 = l.iter().map(|&x| x as u32).sum();
            (sum + 2) >> 2
        }
        (None, None) => 128,
    };
    Ok([dc as u8; 16])
}

// ─────────────────────────────────────────────────────────────────────
// Helpers: 3-tap filter `(a + 2*b + c + 2) >> 2`. Used by all the
// diagonal / vertical / horizontal directional modes per spec.
// ─────────────────────────────────────────────────────────────────────

#[inline]
fn tap3(a: u8, b: u8, c: u8) -> u8 {
    ((a as u32 + 2 * b as u32 + c as u32 + 2) >> 2) as u8
}

#[inline]
fn tap2(a: u8, b: u8) -> u8 {
    ((a as u32 + b as u32 + 1) >> 1) as u8
}

// ─────────────────────────────────────────────────────────────────────
// Mode 3: Diagonal_Down_Left — uses top + top-right (8 samples).
//
// Build a smoothed 8-sample horizontal axis from t0..tr3, then
// pred[x, y] = filtered[x + y]   for (x, y) ≠ (3, 3)
// pred[3, 3] = filtered[6,7,7]   (the spec's special-case last pixel)
//
// Spec §8.3.1.2.4.
// ─────────────────────────────────────────────────────────────────────

fn pred_diag_down_left(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("ddl: top"))?;
    let tr = n.top_right.ok_or_else(|| err_neighbor_missing("ddl: top-right"))?;
    let p: [u8; 8] = [t[0], t[1], t[2], t[3], tr[0], tr[1], tr[2], tr[3]];
    // Smoothed samples a[k] = tap3(p[k-1], p[k], p[k+1]) for k=0..6,
    // a[7] = (p[6] + 3*p[7] + 2) >> 2.
    let mut a = [0u8; 8];
    a[0] = tap3(p[0], p[0], p[1]); // p[-1] = p[0] (clamp)
    for k in 1..7 {
        a[k] = tap3(p[k - 1], p[k], p[k + 1]);
    }
    a[7] = ((p[6] as u32 + 3 * p[7] as u32 + 2) >> 2) as u8;

    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            let v = if x == 3 && y == 3 { a[7] } else { a[x + y] };
            at(&mut out, x, y, v);
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Mode 4: Diagonal_Down_Right — uses top + left + top-left.
// pred[x, y] depends on the diagonal index (x - y).
// Spec §8.3.1.2.5.
// ─────────────────────────────────────────────────────────────────────

fn pred_diag_down_right(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("ddr: top"))?;
    let l = n.left.ok_or_else(|| err_neighbor_missing("ddr: left"))?;
    let tl = n.top_left.ok_or_else(|| err_neighbor_missing("ddr: top-left"))?;
    // Linear sample stream from bottom-left up-and-to-the-right:
    //   p[0]=l[3], p[1]=l[2], p[2]=l[1], p[3]=l[0], p[4]=tl,
    //   p[5]=t[0], p[6]=t[1], p[7]=t[2], p[8]=t[3]
    let p: [u8; 9] = [l[3], l[2], l[1], l[0], tl, t[0], t[1], t[2], t[3]];
    let mut a = [0u8; 7];
    for k in 0..7 {
        a[k] = tap3(p[k], p[k + 1], p[k + 2]);
    }
    // pred[x, y] = a[4 + x - y] (so the diagonal at x=y reads a[4]).
    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            // Diagonal index in 0..=6, reading a[6 - (y - x) + (x - y)?]
            // Actually: position in `a` is 3 + x - y, where:
            //   y - x = 3  → index 0 (lowest, l[3] side)
            //   y - x = 0  → index 3
            //   y - x = -3 → index 6 (highest, t[3] side)
            let idx = (3 + x as isize - y as isize) as usize;
            at(&mut out, x, y, a[idx]);
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Modes 5/6/7/8: Vertical_Right, Horizontal_Down, Vertical_Left,
// Horizontal_Up. Spec §§8.3.1.2.6 – 8.3.1.2.9.
//
// Each is a structured weighted-average pattern with hand-coded per-
// pixel rules — the spec writes them as a 16-line case table. We
// transcribe directly so the diff against the spec is line-for-line
// auditable.
// ─────────────────────────────────────────────────────────────────────

fn pred_vertical_right(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("vr: top"))?;
    let l = n.left.ok_or_else(|| err_neighbor_missing("vr: left"))?;
    let tl = n.top_left.ok_or_else(|| err_neighbor_missing("vr: top-left"))?;

    let mut out = [0u8; 16];
    // Per spec §8.3.1.2.6 zVR = 2*x - y:
    //   zVR == 0, 2, 4, 6: pred[x,y] = (p[x-y/2-1, -1] + p[x-y/2, -1] + 1) >> 1
    //   zVR == 1, 3, 5, 7: pred[x,y] = (p[x-y/2-2,-1] + 2*p[x-y/2-1,-1] + p[x-y/2,-1] + 2) >> 2
    //   zVR == -1: pred[x,y] = (p[-1,0] + 2*p[-1,-1] + p[0,-1] + 2) >> 2
    //   zVR == -2 or -3: pred[x,y] = (p[-1,y-1] + 2*p[-1,y-2] + p[-1,y-3] + 2) >> 2
    // where p[i, -1] reads the top row (top-left at i=-1, t[0..3] at i=0..3).
    let top_at = |i: isize| -> u8 {
        if i == -1 { tl } else { t[i as usize] }
    };
    let left_at = |j: isize| -> u8 {
        // j ∈ -1..=3. j=-1 → top-left (corner); j>=0 → l[j].
        if j == -1 { tl } else { l[j as usize] }
    };

    for y in 0..4 {
        for x in 0..4 {
            let zvr = 2 * x as isize - y as isize;
            let v = if zvr == 0 || zvr == 2 || zvr == 4 || zvr == 6 {
                let i = x as isize - (y as isize >> 1) - 1;
                tap2(top_at(i), top_at(i + 1))
            } else if zvr == 1 || zvr == 3 || zvr == 5 || zvr == 7 {
                let i = x as isize - (y as isize >> 1) - 1;
                tap3(top_at(i - 1), top_at(i), top_at(i + 1))
            } else if zvr == -1 {
                tap3(left_at(0), tl, top_at(0))
            } else {
                // zvr == -2 or -3
                tap3(left_at(y as isize - 1), left_at(y as isize - 2), left_at(y as isize - 3))
            };
            at(&mut out, x, y, v);
        }
    }
    Ok(out)
}

fn pred_horizontal_down(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("hd: top"))?;
    let l = n.left.ok_or_else(|| err_neighbor_missing("hd: left"))?;
    let tl = n.top_left.ok_or_else(|| err_neighbor_missing("hd: top-left"))?;

    let top_at = |i: isize| -> u8 {
        if i == -1 { tl } else { t[i as usize] }
    };
    let left_at = |j: isize| -> u8 {
        if j == -1 { tl } else { l[j as usize] }
    };

    let mut out = [0u8; 16];
    // Per spec §8.3.1.2.7 zHD = 2*y - x:
    //   zHD == 0, 2, 4, 6: pred[x,y] = (p[-1, y-x/2-1] + p[-1, y-x/2] + 1) >> 1
    //   zHD == 1, 3, 5, 7: pred[x,y] = (p[-1, y-x/2-2] + 2*p[-1,y-x/2-1] + p[-1,y-x/2] + 2) >> 2
    //   zHD == -1: pred[x,y] = (p[-1,0] + 2*p[-1,-1] + p[0,-1] + 2) >> 2
    //   zHD == -2 or -3: pred[x,y] = (p[x-1,-1] + 2*p[x-2,-1] + p[x-3,-1] + 2) >> 2
    for y in 0..4 {
        for x in 0..4 {
            let zhd = 2 * y as isize - x as isize;
            let v = if zhd == 0 || zhd == 2 || zhd == 4 || zhd == 6 {
                let j = y as isize - (x as isize >> 1) - 1;
                tap2(left_at(j), left_at(j + 1))
            } else if zhd == 1 || zhd == 3 || zhd == 5 || zhd == 7 {
                let j = y as isize - (x as isize >> 1) - 1;
                tap3(left_at(j - 1), left_at(j), left_at(j + 1))
            } else if zhd == -1 {
                tap3(left_at(0), tl, top_at(0))
            } else {
                tap3(top_at(x as isize - 1), top_at(x as isize - 2), top_at(x as isize - 3))
            };
            at(&mut out, x, y, v);
        }
    }
    Ok(out)
}

fn pred_vertical_left(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let t = n.top.ok_or_else(|| err_neighbor_missing("vl: top"))?;
    let tr = n.top_right.ok_or_else(|| err_neighbor_missing("vl: top-right"))?;
    let p: [u8; 8] = [t[0], t[1], t[2], t[3], tr[0], tr[1], tr[2], tr[3]];

    // Per spec §8.3.1.2.8:
    //   pred[x,y] for y==0 or 2: (p[x+y/2,-1] + p[x+y/2+1,-1] + 1) >> 1
    //   pred[x,y] for y==1 or 3: (p[x+y/2,-1] + 2*p[x+y/2+1,-1] + p[x+y/2+2,-1] + 2) >> 2
    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            let v = if y % 2 == 0 {
                let i = x + (y >> 1);
                tap2(p[i], p[i + 1])
            } else {
                let i = x + (y >> 1);
                tap3(p[i], p[i + 1], p[i + 2])
            };
            at(&mut out, x, y, v);
        }
    }
    Ok(out)
}

fn pred_horizontal_up(n: &Neighbors4x4) -> Result<[u8; 16], DecodeError> {
    let l = n.left.ok_or_else(|| err_neighbor_missing("hu: left"))?;

    // Per spec §8.3.1.2.9 (and matching libavcodec / JM reference):
    //   zHU = x + 2*y, valid range 0..=8.
    //   zHU == 0, 2, 4: pred[x,y] = (l[y+x/2]   + l[y+x/2+1] + 1) >> 1
    //   zHU == 1, 3:    pred[x,y] = (l[y+x/2]   + 2*l[y+x/2+1] + l[y+x/2+2] + 2) >> 2
    //   zHU == 5:       pred[x,y] = (l[2] + 3*l[3] + 2) >> 2  (no l[4]; degenerate tap3)
    //   zHU == 6:       pred[x,y] = (l[2] + 3*l[3] + 2) >> 2
    //   zHU >= 7:       pred[x,y] = l[3]
    //
    // The asymmetry at zHU=5/6 reflects that the 4×4 block's left
    // column has only 4 samples (l[0..3]); positions that would
    // reference l[4] clamp to l[3].
    let mut out = [0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            let zhu = x as isize + 2 * y as isize;
            let v: u8 = match zhu {
                0 | 2 | 4 => {
                    let j = y + (x >> 1);
                    tap2(l[j], l[j + 1])
                }
                1 | 3 => {
                    let j = y + (x >> 1);
                    tap3(l[j], l[j + 1], l[j + 2])
                }
                5 | 6 => ((l[2] as u32 + 3 * l[3] as u32 + 2) >> 2) as u8,
                _ => l[3],
            };
            at(&mut out, x, y, v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neighbors_constant(c: u8) -> Neighbors4x4 {
        Neighbors4x4::all([c; 4], [c; 4], [c; 4], c)
    }

    #[test]
    fn vertical_copies_top_row_down() {
        let n = Neighbors4x4 {
            top: Some([10, 20, 30, 40]),
            top_right: None, left: None, top_left: None,
        };
        let out = predict_4x4(Intra4x4Mode::Vertical, &n).unwrap();
        for y in 0..4 {
            assert_eq!(&out[y*4 .. y*4+4], &[10, 20, 30, 40]);
        }
    }

    #[test]
    fn vertical_errors_when_top_unavailable() {
        let n = Neighbors4x4::NONE;
        assert!(matches!(
            predict_4x4(Intra4x4Mode::Vertical, &n),
            Err(DecodeError::OutOfScope(_))
        ));
    }

    #[test]
    fn horizontal_copies_left_column_right() {
        let n = Neighbors4x4 {
            top: None, top_right: None, top_left: None,
            left: Some([10, 20, 30, 40]),
        };
        let out = predict_4x4(Intra4x4Mode::Horizontal, &n).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y*4 + x], [10, 20, 30, 40][y]);
            }
        }
    }

    #[test]
    fn dc_with_both_neighbors_uses_8_sample_average() {
        // top + left = 32 samples summed... wait, only 4 of each = 8 total.
        // top=[10,10,10,10], left=[20,20,20,20]: sum=120, dc=(120+4)>>3=15.
        let n = Neighbors4x4 {
            top: Some([10, 10, 10, 10]),
            left: Some([20, 20, 20, 20]),
            top_right: None, top_left: None,
        };
        let out = predict_4x4(Intra4x4Mode::Dc, &n).unwrap();
        assert!(out.iter().all(|&v| v == 15));
    }

    #[test]
    fn dc_top_only_uses_4_sample_average() {
        // top=[10,10,10,10]: sum=40, dc=(40+2)>>2=10.
        let n = Neighbors4x4 {
            top: Some([10, 10, 10, 10]),
            left: None, top_right: None, top_left: None,
        };
        let out = predict_4x4(Intra4x4Mode::Dc, &n).unwrap();
        assert!(out.iter().all(|&v| v == 10));
    }

    #[test]
    fn dc_left_only_uses_4_sample_average() {
        // left=[20,20,20,20]: sum=80, dc=(80+2)>>2=20.
        let n = Neighbors4x4 {
            top: None, top_right: None, top_left: None,
            left: Some([20, 20, 20, 20]),
        };
        let out = predict_4x4(Intra4x4Mode::Dc, &n).unwrap();
        assert!(out.iter().all(|&v| v == 20));
    }

    #[test]
    fn dc_no_neighbors_falls_back_to_128() {
        let n = Neighbors4x4::NONE;
        let out = predict_4x4(Intra4x4Mode::Dc, &n).unwrap();
        assert!(out.iter().all(|&v| v == 128));
    }

    #[test]
    fn diag_down_left_constant_input_yields_constant_output() {
        // All neighbors = c → all filtered samples = c → all predictions = c.
        let n = neighbors_constant(50);
        let out = predict_4x4(Intra4x4Mode::DiagonalDownLeft, &n).unwrap();
        assert!(out.iter().all(|&v| v == 50));
    }

    #[test]
    fn diag_down_right_constant_input_yields_constant_output() {
        let n = neighbors_constant(75);
        let out = predict_4x4(Intra4x4Mode::DiagonalDownRight, &n).unwrap();
        assert!(out.iter().all(|&v| v == 75));
    }

    #[test]
    fn vertical_right_constant_input_yields_constant_output() {
        let n = neighbors_constant(100);
        let out = predict_4x4(Intra4x4Mode::VerticalRight, &n).unwrap();
        assert!(out.iter().all(|&v| v == 100));
    }

    #[test]
    fn horizontal_down_constant_input_yields_constant_output() {
        let n = neighbors_constant(60);
        let out = predict_4x4(Intra4x4Mode::HorizontalDown, &n).unwrap();
        assert!(out.iter().all(|&v| v == 60));
    }

    #[test]
    fn vertical_left_constant_input_yields_constant_output() {
        let n = neighbors_constant(200);
        let out = predict_4x4(Intra4x4Mode::VerticalLeft, &n).unwrap();
        assert!(out.iter().all(|&v| v == 200));
    }

    #[test]
    fn horizontal_up_constant_input_yields_constant_output() {
        let n = neighbors_constant(33);
        let out = predict_4x4(Intra4x4Mode::HorizontalUp, &n).unwrap();
        assert!(out.iter().all(|&v| v == 33));
    }

    #[test]
    fn from_index_maps_all_nine_modes() {
        for i in 0..=8u8 {
            assert!(Intra4x4Mode::from_index(i).is_ok(), "idx {} should be valid", i);
        }
        assert!(Intra4x4Mode::from_index(9).is_err());
    }

    #[test]
    fn diagonal_modes_propagate_top_right_neighbor_to_corner() {
        // DDL: pred[3,3] reads from p[6,7,7] = (tr[2] + 3*tr[3] + 2) >> 2.
        // Set top=[0,0,0,0], top_right=[0,0,8,32]: a[7] = (8+96+2)>>2 = 26.
        let n = Neighbors4x4 {
            top: Some([0, 0, 0, 0]),
            top_right: Some([0, 0, 8, 32]),
            left: None,
            top_left: None,
        };
        let out = predict_4x4(Intra4x4Mode::DiagonalDownLeft, &n).unwrap();
        assert_eq!(out[15], 26, "pred[3,3] should be (tr[2]+3*tr[3]+2)>>2");
    }
}
