#!/usr/bin/env python3
"""Where a Studio render differs most from the artist's: mel bands x sliding windows.

usage: meldiff.py REFERENCE STUDIO [--out PREFIX] [--win 2] [--hop 1] [--bands 48] [--top 20]

Both files decoded (ffmpeg) to 32 kHz mono (L+R)/2, a 4096-point STFT (hop
1024) streamed in blocks, 48 mel bands 40 Hz..15 kHz (HTK), averaged over
WIN-second windows every HOP seconds, in dB. STUDIO is aligned to REFERENCE
by cross-correlating onset strength (+-1 s). The overall level difference is
reported, then removed, so the map shows BALANCE and TIMBRE, not loudness.
Cells where both are more than 60 dB under the reference's loudest window
are ignored (silence); either side is floored 90 dB under it, so a section
silent in one and sounding in the other reads as a big difference, not -200 dB.

Writes PREFIX.txt (report), PREFIX.csv (window x band, dB studio - ref after
the level match) and PREFIX.png (heatmap; red = Studio louder).
"""
import sys, argparse, subprocess, numpy as np

SR, NFFT, HOP = 32_000, 4096, 1024

def decode(path):
    raw = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", path, "-ac", "1", "-ar", str(SR), "-f", "f32le", "-"],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.float32)

def mel_fb(nb, lo=40.0, hi=15_000.0):
    mel = lambda f: 2595 * np.log10(1 + f / 700)
    imel = lambda m: 700 * (10 ** (m / 2595) - 1)
    edges = imel(np.linspace(mel(lo), mel(hi), nb + 2))
    f = np.fft.rfftfreq(NFFT, 1 / SR)
    fb = np.zeros((nb, len(f)))
    for b in range(nb):
        a, c, d = edges[b], edges[b + 1], edges[b + 2]
        fb[b] = np.clip(np.minimum((f - a) / (c - a), (d - f) / (d - c)), 0, None)
    return fb, edges

def mel_frames(x, fb):
    """mel power per STFT frame, computed in blocks (memory flat for long songs)"""
    w = np.hanning(NFFT).astype(np.float32)
    n = max(0, (len(x) - NFFT) // HOP + 1)
    out = np.empty((n, fb.shape[0]))
    block = 2048
    for s in range(0, n, block):
        k = np.arange(s, min(n, s + block))
        frames = np.stack([x[i * HOP:i * HOP + NFFT] * w for i in k])
        p = np.abs(np.fft.rfft(frames, axis=1)) ** 2
        out[s:s + len(k)] = p @ fb.T
    return out

def onset(m):
    lm = np.log(m.sum(1) + 1e-12)
    return np.maximum(np.diff(lm, prepend=lm[0]), 0)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ref"); ap.add_argument("studio")
    ap.add_argument("--out", default="meldiff"); ap.add_argument("--win", type=float, default=2.0)
    ap.add_argument("--hop", type=float, default=1.0); ap.add_argument("--bands", type=int, default=48)
    ap.add_argument("--top", type=int, default=20)
    a = ap.parse_args()
    fb, edges = mel_fb(a.bands)
    R, S = mel_frames(decode(a.ref), fb), mel_frames(decode(a.studio), fb)
    # align by onset strength, +-1 s
    fps = SR / HOP
    oR, oS = onset(R), onset(S)
    n = min(len(oR), len(oS)); M = int(fps)
    best = max(range(-M, M + 1), key=lambda L: np.dot(oR[max(0, L):n + min(0, L)], oS[max(0, -L):n - max(0, L)]) /
               (np.linalg.norm(oR[max(0, L):n + min(0, L)]) * np.linalg.norm(oS[max(0, -L):n - max(0, L)]) + 1e-12))
    if best > 0: R = R[best:]
    elif best < 0: S = S[-best:]
    n = min(len(R), len(S)); R, S = R[:n], S[:n]
    # sliding windows
    W, H = int(a.win * fps), max(1, int(a.hop * fps))
    starts = list(range(0, max(1, n - W + 1), H))
    dbw = lambda X: np.array([10 * np.log10(X[s:s + W].mean(0) + 1e-20) for s in starts])
    Rd, Sd = dbw(R), dbw(S)
    # a digitally silent side must not read as -200 dB: both clamped 90 dB
    # under the reference's loudest window, so "silent vs sounding" is ~90
    Rd, Sd = np.maximum(Rd, Rd.max() - 90), np.maximum(Sd, Rd.max() - 90)
    floor = Rd.max() - 60
    live = (Rd > floor) | (Sd > floor)
    diff = Sd - Rd
    level = float(np.median(diff[live])) if live.any() else 0.0
    bal = np.where(live, diff - level, np.nan)
    t0 = np.array(starts) / fps + (best / fps if best > 0 else 0)
    centers = np.sqrt(edges[1:-1] * edges[2:])  # geometric centre of each band's peak..next
    lines = [f"reference: {a.ref}", f"studio:    {a.studio}",
             f"alignment: studio shifted {best / fps * 1000:+.0f} ms", f"overall level: studio {level:+.1f} dB vs reference (removed below)",
             f"windows {a.win:g} s every {a.hop:g} s: {len(starts)};  mel bands {a.bands} (40 Hz..15 kHz)", "",
             "BALANCE by band (mean over the song, studio - ref after the level match):"]
    groups = [(40, 120, "sub/808"), (120, 350, "bass/low-mid"), (350, 1000, "mids"), (1000, 3000, "upper mids / vocal presence"), (3000, 7000, "presence/bite"), (7000, 15000, "air")]
    for lo, hi, name in groups:
        sel = (centers >= lo) & (centers < hi)
        v = np.nanmean(bal[:, sel]) if sel.any() else np.nan
        lines.append(f"  {lo:>5}-{hi:<5} Hz  {name:<28} {v:+6.1f} dB")
    lines += ["", "WORST SECTIONS (10 s, mean |difference| over live cells):"]
    sec = {}
    for i, t in enumerate(t0):
        k = int(t // 10)
        row = np.abs(bal[i][~np.isnan(bal[i])])
        if len(row): sec.setdefault(k, []).append(row.mean())
    for k, v in sorted(sec.items(), key=lambda kv: -np.mean(kv[1]))[:8]:
        m = k * 10
        lines.append(f"  {m // 60}:{m % 60:02d}-{(m + 10) // 60}:{(m + 10) % 60:02d}   {np.mean(v):5.1f} dB")
    lines += ["", f"WORST CELLS (time window x band), top {a.top}:"]
    flat = [(abs(bal[i, b]), i, b) for i in range(len(starts)) for b in range(a.bands) if not np.isnan(bal[i, b])]
    flat.sort(reverse=True)
    seen = set()
    for v, i, b in flat:
        key = (int(t0[i] // 4), b // 4)  # merge neighbours
        if key in seen: continue
        seen.add(key)
        t = t0[i]
        lines.append(f"  {int(t) // 60}:{int(t) % 60:02d}  {edges[b + 1]:7.0f} Hz  studio {bal[i, b]:+6.1f} dB  ({'too loud' if bal[i, b] > 0 else 'too quiet'})")
        if len(seen) >= a.top: break
    open(a.out + ".txt", "w").write("\n".join(lines) + "\n")
    np.savetxt(a.out + ".csv", np.column_stack([t0, np.nan_to_num(bal, nan=0.0)]), delimiter=",", fmt="%.2f",
               header="t_s," + ",".join(f"{c:.0f}Hz" for c in edges[1:-1]))
    try:
        import matplotlib; matplotlib.use("Agg"); import matplotlib.pyplot as plt
        fig, ax = plt.subplots(figsize=(max(8, len(starts) / 25), 4.5))
        im = ax.imshow(bal.T, origin="lower", aspect="auto", cmap="RdBu_r", vmin=-15, vmax=15,
                       extent=[t0[0], t0[-1] + a.hop, 0, a.bands])
        ax.set_yticks(range(0, a.bands, 6)); ax.set_yticklabels([f"{edges[b + 1]:.0f}" for b in range(0, a.bands, 6)])
        ax.set_xlabel("seconds"); ax.set_ylabel("Hz (mel)"); ax.set_title(f"Studio - reference, dB after {level:+.1f} dB level match (red = Studio louder)")
        fig.colorbar(im, ax=ax, label="dB"); fig.tight_layout(); fig.savefig(a.out + ".png", dpi=110)
    except Exception as e:
        lines.append(f"(no heatmap: {e})")
    print("\n".join(lines))

if __name__ == "__main__":
    main()
