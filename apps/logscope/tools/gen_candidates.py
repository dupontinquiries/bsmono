#!/usr/bin/env python3
"""Generate a candidate env-sweep file for tune_harness.sh.

Usage:
  gen_candidates.py <scene_key> [--n 12] [--seeds N] [--heights 1.0,1.7] [--scales 1.0,1.4]
                    [--hours 10.5,19] [--angles 0.7,0.28|1.3,0.34] [--season summer]
Writes tools/cand_<scene_key>.txt with lines `label|ENV...`. Random-samples the grid down to --n.
These are pure env overrides; the harness renders them into a scratch dir and never edits the tree.
"""
import sys, os, itertools, random, argparse

HERE = os.path.dirname(os.path.abspath(__file__))


def parse_list(s, conv=float):
    return [conv(x) for x in s.split(",") if x != ""]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scene")
    ap.add_argument("--n", type=int, default=12)
    ap.add_argument("--seeds", type=int, default=3)
    ap.add_argument("--heights", default="1.0,1.7")
    ap.add_argument("--scales", default="1.0,1.4")
    ap.add_argument("--hours", default="10.5,19.0")
    ap.add_argument("--angles", default="0.7,0.28|1.3,0.34")  # pipe-separated yaw,pitch pairs
    ap.add_argument("--season", default="summer")
    ap.add_argument("--exp", default="1.05")
    ap.add_argument("--sat", default="1.2")
    ap.add_argument("--seed0", type=lambda x: int(x, 0), default=0x1111)
    args = ap.parse_args()

    seeds = [args.seed0 + i * 0x1111 for i in range(args.seeds)]
    heights = parse_list(args.heights)
    scales = parse_list(args.scales)
    hours = parse_list(args.hours)
    angles = [a for a in args.angles.split("|") if a]

    grid = list(itertools.product(seeds, heights, scales, hours, angles))
    random.seed(0xC0FFEE)
    random.shuffle(grid)
    grid = grid[: args.n]

    lines = []
    for (sd, h, sc, hr, ang) in grid:
        label = f"s{sd:x}_h{h}_x{sc}_a{ang.replace(',', '-')}_t{hr}"
        env = (f"LOGSCOPE_SEED={sd} LOGSCOPE_TUNE_HEIGHT={h} LOGSCOPE_TUNE_SCALE={sc} "
               f"LOGSCOPE_HOUR={hr} LOGSCOPE_SEASON={args.season} POS_ANGLE_INIT={ang},0 "
               f"LOGSCOPE_SAT={args.sat} LOGSCOPE_EXP={args.exp}")
        lines.append(f"{label}|{env}")
    out = os.path.join(HERE, f"cand_{args.scene}.txt")
    open(out, "w").write("\n".join(lines) + "\n")
    print(f"wrote {len(lines)} candidates -> {out}")


if __name__ == "__main__":
    main()
