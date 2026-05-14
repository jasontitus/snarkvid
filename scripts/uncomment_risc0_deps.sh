#!/usr/bin/env bash
# Uncomment RISC Zero dependencies in the Cargo.toml files so the
# RISC Zero side will build. Run this after installing the toolchain.
set -euo pipefail

cd "$(dirname "$0")/.."

FILES=(
    "spike/risc0/host/Cargo.toml"
    "spike/risc0/methods/Cargo.toml"
    "spike/risc0/methods/guest/Cargo.toml"
)

# sed -i differs between BSD (macOS) and GNU (Linux): BSD requires a
# backup-suffix argument, GNU treats one as the script. Detect and branch.
if sed --version >/dev/null 2>&1; then
    SED_INPLACE=(-i)
else
    SED_INPLACE=(-i '')
fi

for f in "${FILES[@]}"; do
    echo "Uncommenting risc0 deps in $f"
    sed "${SED_INPLACE[@]}" -E 's/^# (risc0-zkvm|risc0-build|bincode|snarkvid-spike-risc0)/\1/' "$f"
done

echo ""
echo "Done. Verify with: grep '^risc0\|^bincode\|^snarkvid' spike/risc0/*/Cargo.toml spike/risc0/methods/guest/Cargo.toml"
