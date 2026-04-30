#!/usr/bin/env python3
"""Combine the two zkVM sides' bench output into one report.

Reads two JSON files (risc0, sp1) emitted by each side's `bench`
subcommand. Writes either:

  - JSON: a single object with both sides keyed by name (default).
  - Markdown: the decision-matrix template from milestones/01-spike.md §6
    with the metric cells filled in.

Usage:
  compare.py risc0.json sp1.json
  compare.py --markdown risc0.json sp1.json
"""

import argparse
import json
import sys
from pathlib import Path


METRICS = [
    ("prove_ms_10m", "Prove 10MB (ms)", "min"),
    ("prove_ms_1m", "Prove 1MB (ms)", "min"),
    ("proof_bytes", "Proof size (bytes)", "min"),
    ("verify_native_ms", "Verify native (ms)", "min"),
    ("verify_browser_ms", "Verify browser (ms)", "min"),
    ("verifier_wasm_gz_bytes", "Verifier bundle gz (bytes)", "min"),
]


def load(path: Path) -> dict:
    with path.open() as f:
        return json.load(f)


def pick(side: dict, key: str):
    """Look up a metric across the side's rows; return None if missing."""
    if key.startswith("prove_ms_"):
        size = key.split("_")[-1]  # 1k / 1m / 10m
        for row in side.get("rows", []):
            if row.get("size_label") == size:
                return row.get("prove_ms")
        return None
    return side.get(key)


def winner(a, b, direction: str) -> str:
    if a is None and b is None:
        return "—"
    if a is None:
        return "sp1"
    if b is None:
        return "risc0"
    if direction == "min":
        return "risc0" if a < b else "sp1" if b < a else "tie"
    return "risc0" if a > b else "sp1" if b > a else "tie"


def render_markdown(risc0: dict, sp1: dict) -> str:
    lines = [
        "# Milestone 1 — Decision",
        "",
        "| Metric | RISC Zero | SP1 | Winner |",
        "|---|---|---|---|",
    ]
    for key, label, direction in METRICS:
        a, b = pick(risc0, key), pick(sp1, key)
        lines.append(f"| {label} | {a} | {b} | {winner(a, b, direction)} |")
    lines += [
        "",
        "Decision: <fill in>",
        "Rationale: <2–3 sentences>",
        "Risks accepted: <bullets>",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--markdown", action="store_true")
    parser.add_argument("risc0", type=Path)
    parser.add_argument("sp1", type=Path)
    args = parser.parse_args()

    risc0, sp1 = load(args.risc0), load(args.sp1)

    if args.markdown:
        sys.stdout.write(render_markdown(risc0, sp1))
    else:
        json.dump({"risc0": risc0, "sp1": sp1}, sys.stdout, indent=2)
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
