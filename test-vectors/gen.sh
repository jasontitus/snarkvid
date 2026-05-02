#!/usr/bin/env bash
# Generate the H.264 test corpus for milestone-3 decoder parity tests.
#
# For each fixture:
#   <name>.yuv     raw YUV 4:2:0 input fed to the encoder
#   <name>.h264    H.264 baseline-profile, I-frame-only, no deblock bitstream
#   <name>.dec.yuv reference-decoded YUV (ffmpeg's h264 decoder output)
#   <name>.meta    width height frame_count
#
# Our decoder must produce byte-identical output to <name>.dec.yuv when
# fed <name>.h264. (We are NOT comparing to <name>.yuv — the encoder is
# lossy.)

set -euo pipefail

cd "$(dirname "$0")"

# Encoder flags that pin x264 to the milestone-3 supported subset:
#   profile=baseline   no CABAC, no B-frames, no 8x8 DCT
#   bf=0               P frames only between Is (irrelevant when keyint=1 too)
#   refs=1             single reference picture
#   8x8dct=0           no 8x8 transform (baseline doesn't use it anyway)
#   weightb=0          no weighted B prediction (n/a, no Bs)
#   keyint=1           every frame is a keyframe (so single-frame inputs are pure I)
#   no-deblock=1       skip deblocking filter (we add it in milestone 4)
#   no-cabac           force CAVLC entropy
ENC_FLAGS=(
    -c:v libx264
    -profile:v baseline
    -preset slow
    -pix_fmt yuv420p
    -x264-params "keyint=1:min-keyint=1:bframes=0:ref=1:8x8dct=0:weightb=0:cabac=0:deblock=-1\,-1:no-deblock=1"
)

# $1 = name; $2 = width; $3 = height; $4 = frame count
encode_fixture() {
    local name="$1" w="$2" h="$3" frames="$4"
    echo ">> encoding $name (${w}x${h}, ${frames} frames)"
    ffmpeg -y -loglevel error \
        -f rawvideo -pix_fmt yuv420p -s "${w}x${h}" -framerate 30 \
        -i "$name.yuv" \
        "${ENC_FLAGS[@]}" \
        "$name.h264"
    # Reference-decode the resulting bitstream.
    ffmpeg -y -loglevel error \
        -i "$name.h264" \
        -f rawvideo -pix_fmt yuv420p \
        "$name.dec.yuv"
    printf '%s %s %s\n' "$w" "$h" "$frames" > "$name.meta"
    printf '   .h264 %s bytes, .dec.yuv %s bytes\n' \
        "$(stat -c%s "$name.h264")" \
        "$(stat -c%s "$name.dec.yuv")"
}

# A single 16x16 macroblock of constant 128 luma, constant 128 chroma.
# The trivial case — every MB is intra-DC predicted, all residuals zero.
gen_solid_16x16() {
    python3 -c "
import sys
W, H = 16, 16
sys.stdout.buffer.write(bytes([128]) * (W*H + 2*(W//2)*(H//2)))
" > solid_16x16.yuv
    encode_fixture solid_16x16 16 16 1
}

# 32x32 with a horizontal luma gradient. Tests intra prediction modes
# beyond pure DC.
gen_grad_32x32() {
    python3 -c "
import sys
W, H = 32, 32
y = bytes(((c * 8) & 0xff) for r in range(H) for c in range(W))
u = bytes([128]) * ((W//2)*(H//2))
v = bytes([128]) * ((W//2)*(H//2))
sys.stdout.buffer.write(y + u + v)
" > grad_32x32.yuv
    encode_fixture grad_32x32 32 32 1
}

# 64x64 with a 2D smooth radial-ish pattern. Multi-MB content,
# exercises neighbor-prediction across MB boundaries.
gen_smooth_64x64() {
    python3 -c "
import sys
W, H = 64, 64
y = bytearray(W*H)
for r in range(H):
    for c in range(W):
        y[r*W+c] = ((r*4 + c*4) ^ ((r*c) >> 3)) & 0xff
u = bytes([120 + (i % 16) for i in range((W//2)*(H//2))])
v = bytes([136 - (i % 16) for i in range((W//2)*(H//2))])
sys.stdout.buffer.write(bytes(y) + u + v)
" > smooth_64x64.yuv
    encode_fixture smooth_64x64 64 64 1
}

# Two checkerboard-ish patterns at different sizes for high-frequency
# stress (lots of non-zero AC coefficients).
gen_checker_32x16() {
    python3 -c "
import sys
W, H = 32, 16
y = bytes(((32 if ((r//2) ^ (c//2)) & 1 else 224) for r in range(H) for c in range(W)))
u = bytes([128]) * ((W//2)*(H//2))
v = bytes([128]) * ((W//2)*(H//2))
sys.stdout.buffer.write(y + u + v)
" > checker_32x16.yuv
    encode_fixture checker_32x16 32 16 1
}

# 16x16 single MB with an intentionally hostile pattern: every pixel
# value is a different constant. Forces real residual coding.
gen_diag_16x16() {
    python3 -c "
import sys
W, H = 16, 16
y = bytes((((r * 17) ^ (c * 23)) & 0xff) for r in range(H) for c in range(W))
u = bytes([128]) * ((W//2)*(H//2))
v = bytes([128]) * ((W//2)*(H//2))
sys.stdout.buffer.write(y + u + v)
" > diag_16x16.yuv
    encode_fixture diag_16x16 16 16 1
}

main() {
    rm -f *.yuv *.h264 *.dec.yuv *.meta
    gen_solid_16x16
    gen_grad_32x32
    gen_smooth_64x64
    gen_checker_32x16
    gen_diag_16x16
    echo "done."
    ls -la *.h264 | awk '{printf "  %-22s %s bytes\n", $9, $5}'
}

main "$@"
