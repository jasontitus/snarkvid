// SP1 guest for the M2 statement.
//
// Reads (from sp1_zkvm::io::read, in the order the host wrote them):
//   - signed:    SignedManifest
//   - bitstream: BqBitstream
//   - tolerance_db_scaled: i64
//   - original:  YuvFrame                    (private witness)
//   - block_paths: Vec<MerklePath>           (private witness)
//
// Calls verify_m2_claim. Commits two public outputs:
//   - digest:  SHA-256 over the public inputs (bitstream, manifest,
//              tolerance) — same shape the host computes via
//              fixture_digest, so the verifier can recompute and
//              cross-check
//   - status:  u8, 0 on success. Non-zero status codes encode which
//              ClaimError variant fired so a verifier can localize a
//              failed proof attempt without re-running prove
//
// The guest is essentially a 50-line wrapper around the
// snarkvid-m2-statement crate. All the actual M2 logic lives in
// crates/m2-statement so the same code runs natively (host smoke
// command, integration test) and in-circuit (here).

#![no_main]
sp1_zkvm::entrypoint!(main);

use snarkvid_m2_statement::{public_inputs_digest, verify_m2_claim, ClaimError};
use snarkvid_manifest::{MerklePath, SignedManifest};
use snarkvid_toy_codec::{BqBitstream, YuvFrame};

const STATUS_OK: u8 = 0;
const STATUS_MANIFEST_SIG: u8 = 1;
const STATUS_MERKLE_COUNT: u8 = 2;
const STATUS_MERKLE_INVALID: u8 = 3;
const STATUS_DECODE_FAILED: u8 = 4;
const STATUS_PSNR_LOW: u8 = 5;

pub fn main() {
    let signed: SignedManifest = sp1_zkvm::io::read::<SignedManifest>();
    let bitstream: BqBitstream = sp1_zkvm::io::read::<BqBitstream>();
    let tolerance_db_scaled: i64 = sp1_zkvm::io::read::<i64>();
    let original: YuvFrame = sp1_zkvm::io::read::<YuvFrame>();
    let block_paths: Vec<MerklePath> = sp1_zkvm::io::read::<Vec<MerklePath>>();

    // Public-input digest. Hashed before the claim check so a failing
    // prove still commits the digest of *what* it tried to prove.
    let digest = public_inputs_digest(&signed, &bitstream, tolerance_db_scaled);

    let status = match verify_m2_claim(
        &signed, &bitstream, &original, &block_paths, tolerance_db_scaled,
    ) {
        Ok(_) => STATUS_OK,
        Err(ClaimError::ManifestSignatureInvalid) => STATUS_MANIFEST_SIG,
        Err(ClaimError::MerklePathCountMismatch) => STATUS_MERKLE_COUNT,
        Err(ClaimError::MerklePathInvalid { .. }) => STATUS_MERKLE_INVALID,
        Err(ClaimError::DecodeFailed) => STATUS_DECODE_FAILED,
        Err(ClaimError::PsnrBelowTolerance(_)) => STATUS_PSNR_LOW,
    };

    sp1_zkvm::io::commit(&digest);
    sp1_zkvm::io::commit(&status);

    // Hard-fail on non-OK so the prover can't produce a "successful"
    // proof of a rejected claim. (Still commits the status byte so a
    // debugging caller can read it before the panic propagates.)
    if status != STATUS_OK {
        panic!("M2 claim rejected: status={}", status);
    }
}

