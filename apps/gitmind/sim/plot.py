#!/usr/bin/env python3
# Turns sim_out/metrics.json into PNG "screenshots": storage-vs-git charts
# and a touch-UI design preview rendered from real checkpoint-style data.
import json, os
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch, Circle

OUT = "sim_out"
MB = 1e6
INK = "#0f1320"; PANEL = "#171c2b"; ACCENT = "#5ec8d8"
GREEN = "#56d364"; RED = "#f0796b"; MUTED = "#8a93a6"; TEXT = "#e6e9ef"

def cumsum(xs):
    t, out = 0, []
    for x in xs:
        t += x; out.append(t)
    return out

def storage_chart(name, rows, title, subtitle):
    vs = [r["version"] for r in rows]
    git_loose = [r["git_loose"]/MB for r in rows]
    git_packed = [r["git_packed"]/MB for r in rows]
    gm_logical = [v/MB for v in cumsum([r["gm_new_bytes"] for r in rows])]
    gm_file = [r["gm_cumulative"]/MB for r in rows]

    fig, ax = plt.subplots(figsize=(8.4, 4.8), dpi=150)
    fig.patch.set_facecolor("white")
    ax.plot(vs, git_loose, "-o", color=RED, lw=2, ms=4, label="git — loose (before gc)")
    ax.plot(vs, git_packed, "-o", color="#b08cff", lw=2, ms=4, label="git — packed (after gc)")
    ax.plot(vs, gm_logical, "-o", color=GREEN, lw=2.4, ms=4, label="gitmind — logical bytes stored")
    ax.plot(vs, gm_file, "--", color=MUTED, lw=1.4, label="gitmind — redb file on disk")
    ax.set_xlabel("version"); ax.set_ylabel("cumulative storage (MB)")
    ax.set_title(title, fontweight="bold", loc="left", pad=10)
    fig.text(0.012, 0.012, subtitle, color="#555", fontsize=8.5, style="italic")
    ax.grid(True, alpha=0.25); ax.legend(loc="center left", fontsize=8.5, framealpha=0.9)
    ax.set_ylim(bottom=0)
    fig.tight_layout(rect=(0, 0.03, 1, 1))
    p = os.path.join(OUT, f"chart_{name}.png")
    fig.savefig(p); plt.close(fig)
    return p

def incremental_chart(scen):
    fig, ax = plt.subplots(figsize=(8.4, 4.2), dpi=150)
    fig.patch.set_facecolor("white")
    for (name, rows, color, label) in scen:
        vs = [r["version"] for r in rows][1:]
        inc = [r["gm_new_bytes"]/MB for r in rows][1:]
        ax.plot(vs, inc, "-o", color=color, lw=2, ms=4, label=label)
    ax.set_xlabel("version"); ax.set_ylabel("new bytes stored this version (MB)")
    ax.set_title("Incremental cost per version — only changed chunks are stored",
                 fontweight="bold", loc="left")
    ax.grid(True, alpha=0.25); ax.legend(fontsize=8.5)
    ax.set_ylim(bottom=0)
    fig.tight_layout()
    p = os.path.join(OUT, "chart_incremental.png")
    fig.savefig(p); plt.close(fig)
    return p

def ui_preview():
    # A design preview of the planned touch UI rendered from sample data —
    # NOT a live Qt screenshot (Qt isn't built in this sandbox).
    fig = plt.figure(figsize=(4.6, 8.6), dpi=150)
    fig.patch.set_facecolor("#0a0d16")
    ax = fig.add_axes([0, 0, 1, 1]); ax.set_xlim(0, 100); ax.set_ylim(0, 188)
    ax.axis("off")

    def panel(x, y, w, h, color, r=2.4):
        ax.add_patch(FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0,rounding_size={r}",
                                    fc=color, ec="none"))

    panel(0, 0, 100, 188, INK, r=0)
    # header
    ax.text(6, 178, "gitmind", color=TEXT, fontsize=15, fontweight="bold")
    ax.text(6, 172, "on  main   ↑0 ↓0", color=ACCENT, fontsize=8)

    # status rows with swipe affordances
    rows = [("src/store.rs", "M", GREEN, "stage  →"),
            ("src/merge.rs", "M", GREEN, "stage  →"),
            ("assets/intro.mp4", "??", MUTED, "ingest →"),
            ("Cargo.toml", "●", RED, "←  unstage")]
    y = 158
    for name, tag, col, hint in rows:
        panel(6, y, 88, 12, PANEL)
        ax.text(10, y+7.4, name, color=TEXT, fontsize=8.2, va="center")
        ax.text(10, y+3.4, hint, color=MUTED, fontsize=6.3, style="italic", va="center")
        ax.text(89, y+6, tag, color=col, fontsize=8, ha="right", va="center", fontweight="bold")
        y -= 15
    ax.text(6, y+2, "swipe a row to stage · hold for menu", color=MUTED, fontsize=6.5, style="italic")

    # timeline strip
    ty = 70
    ax.text(6, ty+16, "timeline", color=TEXT, fontsize=11, fontweight="bold")
    panel(6, ty-26, 88, 38, PANEL)
    labels = ["init", "checkpoint", "first edit", "media ingest", "second edit", "now"]
    n = len(labels)
    x0, x1 = 14, 86
    xs = [x0 + (x1-x0)*i/(n-1) for i in range(n)]
    ax.plot(xs, [ty]*n, "-", color=MUTED, lw=1.6, zorder=1)
    for i, (xx, lab) in enumerate(zip(xs, labels)):
        c = ACCENT if i < n-1 else GREEN
        ax.add_patch(Circle((xx, ty), 1.7, fc=c, ec="none", zorder=2))
        ax.text(xx, ty-7, lab, color=MUTED, fontsize=5.2, ha="center", rotation=30)
    ax.text(6, ty-30, "pinch to zoom · tap a node to materialize a side worktree",
            color=MUTED, fontsize=6, style="italic")

    # bottom bar
    panel(6, 8, 88, 16, PANEL)
    for i, gl in enumerate(["↩ undo", "+ snapshot", "find", "⇄ sync"]):
        ax.text(14 + i*21, 14.5, gl, color=ACCENT, fontsize=7.5, ha="center")

    ax.text(50, 3, "design preview — not a live Qt screenshot", color="#444",
            fontsize=6, ha="center", style="italic")
    p = os.path.join(OUT, "ui_timeline_preview.png")
    fig.savefig(p, facecolor=fig.get_facecolor()); plt.close(fig)
    return p

def main():
    with open(os.path.join(OUT, "metrics.json")) as f:
        m = json.load(f)
    sc = m["scenarios"]
    p1 = storage_chart("small_edit", sc["small_edit_16MiB"],
                       "Small edit to a 16 MiB asset, 10 versions",
                       "gitmind reaches packed-git efficiency at write time — no gc step")
    p2 = storage_chart("append", sc["append_1MiB"],
                       "Append-only growth (8 MiB +1 MiB ×10)",
                       "chunk dedup keeps the unchanged prefix; git-loose climbs without gc")
    p3 = incremental_chart([
        ("small_edit", sc["small_edit_16MiB"], GREEN, "small edit (16 KiB change)"),
        ("append", sc["append_1MiB"], ACCENT, "append (1 MiB/version)"),
    ])
    p4 = ui_preview()
    print("throughput:", m["throughput_mb_s"], "MB/s")
    print("wrote:", p1, p2, p3, p4)

if __name__ == "__main__":
    main()
