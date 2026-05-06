// toy-encode: encode a raw YUV 4:2:0 frame into the BlockQuant toy codec.
//
// This produces test inputs for milestone 2. The output bitstream can be
// read back by decode_toy() in the zkVM guest.
//
// Usage:
//   toy-encode --input frame.yuv --width 1280 --height 720 --qp 8 --output compressed.bq
//
// The input is a raw YUV 4:2:0 file: Y plane (W×H), then U (W/2 × H/2),
// then V (W/2 × H/2). One byte per sample, planar.

use anyhow::{Context, Result};
use clap::Parser;
use snarkvid_toy_codec::{encode_toy, YuvFrame};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "toy-encode")]
#[command(about = "Encode a raw YUV frame into BlockQuant format")]
struct Cli {
    /// Input raw YUV 4:2:0 file
    #[arg(long)]
    input: PathBuf,

    /// Frame width (must be multiple of 16)
    #[arg(long)]
    width: u16,

    /// Frame height (must be multiple of 16)
    #[arg(long)]
    height: u16,

    /// Quantization parameter (0–51, 0 ≈ lossless)
    #[arg(long, default_value = "8")]
    qp: u8,

    /// Output BlockQuant bitstream file
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let data = std::fs::read(&cli.input).context("failed to read input YUV")?;

    let y_size = cli.width as usize * cli.height as usize;
    let uv_size = (cli.width as usize / 2) * (cli.height as usize / 2);
    let expected = y_size + 2 * uv_size;

    anyhow::ensure!(
        data.len() >= expected,
        "input too small: got {} bytes, need {} ({}x{} YUV420)",
        data.len(),
        expected,
        cli.width,
        cli.height
    );

    let y = data[0..y_size].to_vec();
    let u = data[y_size..y_size + uv_size].to_vec();
    let v = data[y_size + uv_size..y_size + 2 * uv_size].to_vec();

    let frame = YuvFrame {
        width: cli.width,
        height: cli.height,
        y,
        u,
        v,
    };

    let bitstream = encode_toy(&frame, cli.qp)?;

    // Simple binary format: header + raw coefficients
    let mut out = Vec::new();
    out.extend_from_slice(&bitstream.header.width.to_le_bytes());
    out.extend_from_slice(&bitstream.header.height.to_le_bytes());
    out.push(bitstream.header.qp);
    out.push(bitstream.header.chroma_format);
    // Coefficient counts
    out.extend_from_slice(&(bitstream.coeffs_y.len() as u32).to_le_bytes());
    out.extend_from_slice(&(bitstream.coeffs_u.len() as u32).to_le_bytes());
    out.extend_from_slice(&(bitstream.coeffs_v.len() as u32).to_le_bytes());
    // Coefficients
    for c in &bitstream.coeffs_y {
        out.extend_from_slice(&c.to_le_bytes());
    }
    for c in &bitstream.coeffs_u {
        out.extend_from_slice(&c.to_le_bytes());
    }
    for c in &bitstream.coeffs_v {
        out.extend_from_slice(&c.to_le_bytes());
    }

    std::fs::write(&cli.output, &out).context("failed to write output")?;

    println!(
        "Encoded {}x{} YUV420, QP={}: {} bytes compressed (from {} raw)",
        cli.width,
        cli.height,
        cli.qp,
        out.len(),
        data.len(),
    );

    Ok(())
}
