#!/usr/bin/env bash
# Regenerate the H.264 test corpus in crates/h264-test-vectors/fixtures/.
# Requires x264 + ffmpeg (apt install x264 ffmpeg on Ubuntu).
#
# Each entry produces three artifacts:
#   <name>.yuv                         original raw YUV 4:2:0 input
#   <name>-qpNN.h264                   x264-encoded bitstream
#   <name>-qpNN-decoded.yuv            ffmpeg-decoded reference output
#
# All fixtures use the M3-restricted x264 flags: --profile baseline,
# --bframes 0, --ref 1, --weightp 0, --no-8x8dct, --no-deblock,
# --no-cabac, --frames 1, --keyint 1.

set -euo pipefail

cd "$(dirname "$0")/.."
DEST="crates/h264-test-vectors/fixtures"
mkdir -p "$DEST"

if ! command -v x264   >/dev/null; then echo "missing x264 (apt install x264)"; exit 1; fi
if ! command -v ffmpeg >/dev/null; then echo "missing ffmpeg (apt install ffmpeg)"; exit 1; fi

# M3 flags. Centralized here so re-runs stay consistent.
X264_FLAGS=(
    --profile baseline
    --bframes 0
    --ref 1
    --weightp 0
    --no-8x8dct
    --no-deblock
    --no-cabac
    --frames 1
    --keyint 1
    --input-csp i420
    --fps 30
)

# Synthesize a deterministic xorshift "noise" YUV at any size.
synth_noise () {
    local W=$1 H=$2 OUT=$3 SEED=${4:-3735928559}
    python3 - "$W" "$H" "$SEED" > "$OUT" <<'PYEOF'
import sys
w, h, seed = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
state = seed & 0xffffffff
def xs():
    global state
    state ^= (state << 13) & 0xffffffff
    state ^= state >> 17
    state ^= (state << 5) & 0xffffffff
    return state & 0xff
n = w*h + 2*(w//2)*(h//2)
sys.stdout.buffer.write(bytes(xs() for _ in range(n)))
PYEOF
}

encode_one () {
    local W=$1 H=$2 QP=$3 NAME=$4
    local YUV="$DEST/$NAME.yuv"
    local H264="$DEST/$NAME-qp${QP}.h264"
    local DEC="$DEST/$NAME-qp${QP}-decoded.yuv"
    synth_noise "$W" "$H" "$YUV"
    x264 "${X264_FLAGS[@]}" --qp "$QP" --output "$H264" --input-res "${W}x${H}" "$YUV" 2>&1 | grep -E "^x264 \[(info|error)\]" || true
    ffmpeg -y -i "$H264" -f rawvideo -pix_fmt yuv420p "$DEC" 2>/dev/null
    echo "  $NAME: $(stat -c %s "$H264") H.264 bytes, $(stat -c %s "$DEC") decoded YUV bytes"
}

echo "=== Generating M3 H.264 test corpus ==="
encode_one  16  16 18 noise-16x16

echo ""
echo "Done. Fixtures in $DEST/"
