#!/usr/bin/env bash
# Generate deterministic fixtures for the spike at three sizes.
#
# Output:
#   fixture-1k.bin    + fixture-1k.bin.sha256
#   fixture-1m.bin    + fixture-1m.bin.sha256
#   fixture-10m.bin   + fixture-10m.bin.sha256
#
# Deterministic so re-runs (and bench comparisons across machines)
# produce identical bytes. We derive a per-fixture AES-256-CTR keystream
# from a fixed seed string.

set -euo pipefail

cd "$(dirname "$0")"

SEED="snarkvid-spike-fixture-seed-v1"

# hex(sha256(s))
sha256_hex() {
    printf '%s' "$1" | openssl dgst -sha256 | awk '{print $NF}'
}

gen() {
    local label="$1" bytes="$2"
    local out="fixture-${label}.bin"
    local key
    key=$(sha256_hex "${SEED}-${label}")
    local iv="00000000000000000000000000000000"
    head -c "$bytes" /dev/zero \
        | openssl enc -aes-256-ctr -K "$key" -iv "$iv" -nopad \
        > "$out"
    sha256sum "$out" > "${out}.sha256"
    printf '  %-20s %12d bytes  sha256=%s...\n' "$out" "$bytes" "$(awk '{print substr($1,1,16)}' < "${out}.sha256")"
}

echo "generating fixtures (deterministic):"
gen "1k"   1024
gen "1m"   1048576
gen "10m"  10485760
echo "done."
