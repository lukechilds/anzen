#!/usr/bin/env python3
"""A/B benchmark: a baseline commit against the working tree.

The GPU clocks down under sustained load, so back-to-back runs of one build are not comparable
with runs of another taken minutes apart. This builds the benchmark at BASE (default HEAD) in a
scratch worktree and from the working tree, then alternates them in ABBA order and compares
medians.

    crates/anzen-vanity/bench/ab.py nonce --rounds 4 --seconds 3
    crates/anzen-vanity/bench/ab.py both --base HEAD~1
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
BUILD = ["cargo", "build", "-q", "--release", "-p", "anzen-vanity", "--example", "benchmark"]


def build_base(rev: str) -> Path:
    tree = REPO / "target" / "bench-base-tree"
    if not tree.exists():
        subprocess.run(["git", "worktree", "add", "-q", "--detach", str(tree), rev], cwd=REPO, check=True)
    subprocess.run(["git", "checkout", "-q", "--detach", rev], cwd=tree, check=True)
    env = dict(os.environ, CARGO_TARGET_DIR=str(REPO / "target" / "bench-base"))
    subprocess.run(BUILD, cwd=tree, env=env, check=True)
    return REPO / "target" / "bench-base" / "release" / "examples" / "benchmark"


def build_candidate() -> Path:
    env = dict(os.environ, CARGO_TARGET_DIR=str(REPO / "target" / "bench-candidate"))
    subprocess.run(BUILD, cwd=REPO, env=env, check=True)
    return REPO / "target" / "bench-candidate" / "release" / "examples" / "benchmark"


def run(binary: Path, kernel: str, seconds: float, warmup: float) -> float:
    output = subprocess.run(
        [str(binary), kernel, "--seconds", str(seconds), "--warmup", str(warmup)],
        capture_output=True, text=True, check=True,
    ).stdout
    return json.loads(output.strip().splitlines()[-1])["rate"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("kernel", choices=["key", "nonce", "both"])
    parser.add_argument("--base", default="HEAD")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--seconds", type=float, default=3.0)
    parser.add_argument("--warmup", type=float, default=1.0)
    args = parser.parse_args()

    base, candidate = build_base(args.base), build_candidate()
    kernels = ["key", "nonce"] if args.kernel == "both" else [args.kernel]
    for kernel in kernels:
        rates = {"base": [], "candidate": []}
        for round_index in range(args.rounds):
            order = [("base", base), ("candidate", candidate)]
            if round_index % 2:
                order.reverse()
            for name, binary in order:
                rates[name].append(run(binary, kernel, args.seconds, args.warmup))
        base_median = statistics.median(rates["base"])
        candidate_median = statistics.median(rates["candidate"])
        print(json.dumps({
            "kernel": kernel,
            "base": round(base_median),
            "candidate": round(candidate_median),
            "ratio": round(candidate_median / base_median, 4),
            "base_runs": [round(rate) for rate in rates["base"]],
            "candidate_runs": [round(rate) for rate in rates["candidate"]],
        }))
        sys.stdout.flush()


if __name__ == "__main__":
    main()
