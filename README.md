# snarkvid

build a tool to generate proofs that a h264 compressed video is derived from a already verified video. 

When building a secure evidence-collection pipeline where hardware-backed cryptographic signatures validate the original media, this architecture allows you to maintain that chain of trust even after compressing the file for web delivery.
### zk-SNARK Video Recompression Proof: Required Components
**1. The Circuit (The Mathematical Logic)**
This is the hardest part of the process. You must translate the core logic of the H.264 encoder into an arithmetic circuit (a massive system of polynomial equations).
 * **What it does:** It mathematically dictates that "Input A (Original Video) passed through Transformation B (H.264 compression constraints) results exactly in Output C (Compressed Video)."
 * **Tooling:** **Circom** is the standard language for defining these circuits.
 * **Documentation:** docs.circom.io
**2. The Prover (Heavy Computation)**
This is the server-side component that actually generates the cryptographic proof. It requires substantial compute power (like the Nvidia T4 GPUs discussed earlier).
 * **Inputs:** * *Private Input:* The original, cryptographically verified high-resolution video.
   * *Public Input:* The compressed H.264 web-friendly video.
 * **What it does:** It runs the inputs through the Circuit to generate a tiny .json proof file confirming the compression was executed perfectly, without revealing the original file.
 * **Tooling:** C++ or Rust-based provers are best for heavy workloads. snarkjs can be used here for testing, but production video processing will likely require lower-level GPU-accelerated libraries like Bellman or Rapidsnark.
**3. The Verifier (Lightweight Client/Browser)**
This is the component that runs on the end-user's device, allowing anyone to independently verify the video's authenticity without needing heavy compute or access to the original file.
 * **Inputs:** The compressed video and the generated zk-SNARK proof.
 * **What it does:** It runs a fast mathematical check (usually taking milliseconds) to confirm the proof is valid.
 * **Tooling:** **snarkjs** is the standard for implementing verifiers directly in JavaScript or WebAssembly.
 * **Repository & Tutorials:** github.com/iden3/snarkjs
**4. The Trusted Setup (Optional but Common)**
Depending on the specific zero-knowledge proof system you use (like Groth16, which produces the smallest and fastest proofs), you may need a "trusted setup" phase to generate the cryptographic keys used by the Prover and Verifier. Newer systems like PLONK or STARKs bypass this requirement, though their proofs can be larger or slightly slower to verify.
To start experimenting, the most practical first step is to follow the snarkjs GitHub README. It walks through creating a basic Circom circuit, running the setup, generating a proof, and verifying it in Node.js. Once you understand that lifecycle, you can start conceptualizing how to represent simpler media transformations before tackling full H.264 encoding.
