#!/usr/bin/env python3
"""A/B benchmark: a baseline commit against the working tree.

The GPU's clocks drift by tens of percent within seconds, so separate runs barely compare.

- Shader changes (the default): the working tree's benchmark compiles BASE's shaders next to its
  own and alternates single dispatches between them, reporting the median paired ratio.
- Host changes (--binaries): BASE is built in a scratch worktree and whole runs alternate in
  ABBA order. Much noisier; use more rounds.

    crates/anzen-vanity/bench/ab.py nonce
    crates/anzen-vanity/bench/ab.py both --base HEAD~1 --pairs 60
    crates/anzen-vanity/bench/ab.py key --binaries --rounds 8
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


def base_shaders(rev: str) -> Path:
    directory = REPO / "target" / "bench-base-shaders"
    directory.mkdir(parents=True, exist_ok=True)
    for name in ["constants.wgsl", "common.wgsl", "field13.wgsl", "key.wgsl", "nonce.wgsl"]:
        shown = subprocess.run(
            ["git", "show", f"{rev}:crates/anzen-vanity/src/{name}"],
            cwd=REPO, capture_output=True, text=True,
        )
        # Files a baseline predates are left out (the benchmark treats them as empty).
        if shown.returncode == 0:
            (directory / name).write_text(shown.stdout)
        elif (directory / name).exists():
            (directory / name).unlink()
    return directory


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
    parser.add_argument("--binaries", action="store_true")
    parser.add_argument("--pairs", type=int, default=40)
    args = parser.parse_args()
    kernels = ["key", "nonce"] if args.kernel == "both" else [args.kernel]

    if not args.binaries:
        candidate, shaders = build_candidate(), base_shaders(args.base)
        for kernel in kernels:
            output = subprocess.run(
                [str(candidate), "pair", kernel, "--base-shaders", str(shaders), "--pairs", str(args.pairs)],
                capture_output=True, text=True, check=True,
            ).stdout
            print(output.strip().splitlines()[-1])
            sys.stdout.flush()
        return

    base, candidate = build_base(args.base), build_candidate()
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
