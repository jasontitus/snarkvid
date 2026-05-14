# snarkvid — Windows + WSL2 + RTX 3090 Setup

This is the WSL-specific companion to `SETUP_LINUX_CUDA.md`. The Lambda
Labs path in that doc assumes a clean Ubuntu image with a system CUDA
toolkit; WSL2 needs a few extra steps (Windows-side driver, Docker
plumbing for SP1, optional memory cap). Once those are in place the
same `scripts/setup_linux_cuda.sh` and `scripts/full_test_gpu.sh` work
unchanged.

Reference hardware for this doc: **Windows 11 + WSL2 (Ubuntu 22.04),
RTX 3090 (24 GB), 64 GB host RAM.** A 3090 is ~2× an A10 on FP32 with
the same VRAM, so the M1 reference numbers in
`milestones/01-spike-results.md` should be a conservative ceiling.

## What runs here

Everything in `scripts/full_test_gpu.sh` (phases 1–10):

- Phase 4: SP1 + RISC Zero CUDA SHA-256 sweep (1 KB / 1 MB / 10 MB).
- Phase 5: SP1 toy-decode 3-way parity row (needs the SP1 RISC-V guest
  cross-compiled, which needs network access to GitHub).
- Phase 6: Jolt 1 MB / 10 MB rows (CPU-only).
- Phase 7: Sonobe Nova at 1024 fold steps (CPU-only).
- **Phase 8**: Sonobe Decider Groth16 wrap. Needs ≥ 32 GB system RAM;
  64 GB host clears it with margin.
- **Phase 10**: M2 prover end-to-end (`prover-host prove` — manifest +
  Merkle + decode_toy + PSNR inside an SP1 guest).

See `TESTING.md` for the full per-phase inventory and what each one
unblocks.

## Prerequisites (Windows side)

1. **Windows 11 or Windows 10 21H2+** with WSL2.
2. **Recent NVIDIA driver on Windows.** WSL2 CUDA is a driver feature;
   no separate "WSL driver" exists anymore. Get the latest Game Ready
   or Studio driver from nvidia.com.
3. **WSL2 kernel updated**: `wsl --update` from an elevated PowerShell.
4. **Ubuntu 22.04 distro**: `wsl --install -d Ubuntu-22.04` (or
   `Ubuntu` if 22.04 is already your default).
5. **Docker Desktop with WSL2 integration** — see "Docker for SP1"
   below. SP1 6.x's CUDA backend runs the prover in a container.

Verify the Windows-side install before continuing:

```powershell
# In PowerShell:
wsl --version
wsl --status        # should show default version 2
nvidia-smi          # should print the GPU table on Windows too
```

## One-time WSL configuration

### CUDA visibility inside WSL

From inside your WSL Ubuntu shell:

```bash
nvidia-smi
```

If this prints the same GPU table you saw on Windows, you're done —
the Windows driver exposes `/dev/dxg` to WSL automatically and CUDA
just works. If it errors, your Windows driver is too old; update it
and try again.

The CUDA *toolkit* (nvcc, libcudart, etc.) inside WSL is separate. On
Lambda this is preinstalled; on WSL you install it once:

```bash
sudo apt-get update
sudo apt-get install -y nvidia-cuda-toolkit
```

This pulls in `nvcc` and CUDA libs against the inbox CUDA version.
`scripts/setup_linux_cuda.sh` checks for `nvcc`, so install this first.
(NVIDIA's WSL-specific CUDA installer from developer.nvidia.com is an
alternative if you want a newer CUDA version than 22.04 ships, but the
inbox toolkit is enough for both SP1 6.1 and RISC Zero 3.x.)

### Memory and disk caps (`.wslconfig`)

WSL2 defaults to ~50% of host RAM. With 64 GB host that's 32 GB —
exactly at the Sonobe Decider OOM bar. Bump it before running phase 8.

Create or edit `%UserProfile%\.wslconfig` on Windows (e.g.
`C:\Users\<you>\.wslconfig`):

```ini
[wsl2]
memory=56GB
swap=16GB
processors=16
```

Then from PowerShell:

```powershell
wsl --shutdown
# next `wsl` command will pick up the new caps
```

Disk: `target/` dirs across the four spikes total ~10–15 GB; the
RISC Zero compile alone is ~600 crates. Make sure your WSL distro
isn't on a small VHDX — the default location is fine if your C: has
≥ 50 GB free, otherwise move the distro with `wsl --export` /
`wsl --import` to a larger drive.

### Docker for SP1

SP1 6.x's `cuda` backend launches an out-of-process `sp1-gpu-server`
in a Docker container. Without Docker the CUDA backend silently falls
back to CPU and you'll wonder why prove times match your M1.

Two paths, pick one:

**Option A: Docker Desktop (recommended).**

1. Install Docker Desktop on Windows.
2. Settings → Resources → WSL Integration → enable for your Ubuntu
   distro. Apply & restart.
3. Verify from WSL:
   ```bash
   docker run --rm --gpus all nvidia/cuda:12.4.0-base-ubuntu22.04 nvidia-smi
   ```
   Should print the GPU table from inside a container.

**Option B: `docker-ce` directly inside WSL2** plus the NVIDIA
container toolkit. Works but requires manually starting `dockerd`
(no `systemd` by default unless you set `[boot] systemd=true` in
`/etc/wsl.conf`). Use only if you don't want Docker Desktop.

Either way, the GPU-in-container check above is the gate. If it fails,
SP1's CUDA backend won't work.

## Running the test battery

Same as Lambda — clone and run:

```bash
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# Installs SP1, RISC Zero, build deps. Idempotent.
./scripts/setup_linux_cuda.sh

# 90–150 min: phases 1–10. PASS/FAIL summary at the end.
./scripts/full_test_gpu.sh 2>&1 | tee gpu-run.log
```

Outputs of interest land under `spike/bench/results/`:

- `sp1.json`, `risc0.json` — head-to-head SHA-256 numbers; expected
  to beat the Lambda A10 reference table in `01-spike-results.md`.
- `sp1-toy-decode.json` — the missing 3-way parity row from M1b.
- `sonobe-sha256-decider.json` — first real ~200 B Groth16 proof from
  the Sonobe pipeline. This is the **load-bearing browser-verifier
  evidence**; phase 8 OOM'd in the sandbox.
- `m2-proof.bin` — first end-to-end M2 proof (manifest + Merkle +
  decode_toy + PSNR inside SP1).

## WSL-specific gotchas

### `nvidia-smi` works on Windows but not in WSL

Update the Windows-side NVIDIA driver. WSL2 picks up `/dev/dxg` from
the host driver — there's no separate WSL install.

### `sp1-script` is fast but the prove time matches CPU

SP1's CUDA backend silently falls back to CPU if its Docker server
can't start. Check:

```bash
# Should show the sp1-gpu-server container running while a prove is in flight
docker ps
# Or look for it in the SP1 log:
RUST_LOG=debug spike/sp1/target/release/sp1-script bench --fixture-dir spike/common/bench-fixtures --out /tmp/x.json 2>&1 | grep -i 'cuda\|gpu'
```

If no container starts, see "Docker for SP1" above.

### Phase 8 still OOMs

You're still at the 32 GB WSL2 default cap. Edit `.wslconfig` and
`wsl --shutdown` to apply.

### `cargo build` for RISC Zero hangs the WSL VM

The RISC Zero compile is the heaviest step (~600 crates, CUDA kernels).
With `processors=16` and `memory=56GB` it finishes; on a stock WSL
config with default caps it can swap-thrash. Apply the `.wslconfig`
above before kicking it off.

### Networking from WSL is flaky / sp1up fails to fetch from GitHub

Common with corporate VPNs and split DNS. Test from WSL:

```bash
curl -I https://api.github.com
curl -I https://github.com/succinctlabs/sp1/releases
```

If these hang or 4xx, fix Windows-side networking (the WSL VM uses the
host's network stack by default) before running `setup_linux_cuda.sh`.

### Filesystem perf when the repo lives on `/mnt/c/...`

Don't. Clone into the WSL filesystem (`~/snarkvid`, i.e. `\\wsl$\Ubuntu-22.04\home\<you>\snarkvid`).
9p filesystem perf for `/mnt/c` is 5–10× slower for cargo workloads
and will make the RISC Zero compile painful.

## Comparing 3090 numbers to the M1 / A10 reference

Same JSON schema as everywhere else; pair them with `compare.py`:

```bash
# 3090 (this box) vs Lambda A10 (M1 reference, if you have its JSONs)
python3 spike/bench/compare.py --markdown \
    lambda/sp1.json spike/bench/results/sp1.json

# Local M1 CPU vs 3090 CUDA
python3 spike/bench/compare.py --markdown \
    mac/sp1.json spike/bench/results/sp1.json
```

The 10 MB SP1 row is the most interesting comparison — A10 measured
~4 min; a 3090 should beat that. If it doesn't, suspect SP1's Docker
server isn't actually using the GPU (see gotcha above).
