#!/usr/bin/env python3
"""Summarise tools/suite_vs_av1.sh output.

    tools/suite_summary.py <results.csv>

Prints the per-shot table (codec vs each anchor: bytes, luma MS-SSIM,
chroma PSNR), then totals with byte ratios and frame-weighted quality.
No interpolation: every number is a file that was decoded and scored
against the same cropped source.
"""
import csv
import sys


def main(path):
    rows = list(csv.DictReader(open(path)))
    if not rows:
        print("no rows")
        return
    cols = list(rows[0].keys())
    anchors = sorted({c[: -len("_bytes")] for c in cols if c.endswith("_bytes") and c != "codec_bytes"})

    def f(v, d=0.0):
        try:
            return float(v)
        except (TypeError, ValueError):
            return d

    frames = sum(int(r["frames"]) for r in rows)
    print(f"{len(rows)} shots, {frames} frames\n")
    hdr = f"{'shot':>4} {'n':>4} {'codec B':>9} {'MS-SSIM':>8} {'U':>5} {'V':>5}"
    for a in anchors:
        hdr += f" | {a[-14:]:>14} {'MS-SSIM':>8} {'U':>5} {'V':>5} {'x':>5}"
    print(hdr)
    tot = {"codec": 0}
    q = {"codec": 0.0}
    for a in anchors:
        tot[a] = 0
        q[a] = 0.0
    for r in rows:
        n = int(r["frames"])
        cb = int(f(r["codec_bytes"]))
        tot["codec"] += cb
        q["codec"] += f(r["codec_msssim"]) * n
        line = f"{r['shot']:>4} {n:>4} {cb:>9} {f(r['codec_msssim']):>8.5f} {f(r['codec_u']):>5.1f} {f(r['codec_v']):>5.1f}"
        for a in anchors:
            ab = int(f(r[a + "_bytes"]))
            tot[a] += ab
            q[a] += f(r[a + "_msssim"]) * n
            ratio = cb / ab if ab else float("inf")
            line += f" | {ab:>14} {f(r[a + '_msssim']):>8.5f} {f(r[a + '_u']):>5.1f} {f(r[a + '_v']):>5.1f} {ratio:>5.1f}"
        print(line)
    print()
    print(f"{'TOTAL codec':<28} {tot['codec']:>12} B   frame-weighted MS-SSIM {q['codec'] / frames:.5f}")
    for a in anchors:
        print(
            f"{'TOTAL ' + a[-20:]:<28} {tot[a]:>12} B   frame-weighted MS-SSIM {q[a] / frames:.5f}"
            f"   codec is {tot['codec'] / tot[a]:.1f}x the bytes"
        )
    # where the codec is closest / furthest
    for a in anchors:
        ratios = [(int(f(r["codec_bytes"])) / max(1, int(f(r[a + "_bytes"]))), r["shot"], int(r["frames"])) for r in rows]
        ratios = [x for x in ratios if x[2] >= 10]
        ratios.sort()
        print(f"\nvs {a[-20:]}: best shots {[(s, round(x, 1)) for x, s, _ in ratios[:3]]}  worst {[(s, round(x, 1)) for x, s, _ in ratios[-3:]]}")


if __name__ == "__main__":
    main(sys.argv[1])
