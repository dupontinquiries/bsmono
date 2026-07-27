#!/usr/bin/env python3
"""texlearn — learn each scanned model's PALETTE from its texture set and write it into the
learned blueprint as appearance sidecar layers (';;P' palette entries + ';;C' per-cell tints).

Why palettes and not sampling: the IQM conversion zeroed every TEXCOORD (verified: 16,506
zeros on Morton alone) and stamped a placeholder material, so exact UV->texel lookup is
impossible. The textures still carry what the buildings actually LOOK like — their real
stone/wood/roof colors — so we learn a compact palette per model (median-cut over its bound
texture set) and drive it onto cells with the geometry layers mapstudy already learned
(';;H' height bands pick courses, ';;R' roughness jitters tone, roofs take the palette's
most hue-distinct entry). Binding is by archive naming, confidence-tagged in the header;
models with no plausible set keep the engine skin palette (no ';;P' written).

Run AFTER mapstudy: mapstudy writes the grid + H/S/R; this appends P/C.
Usage: texlearn.py <textures-dir> <blueprints-dir>
"""
import os, sys, colorsys
from PIL import Image

BIND = {  # slug -> (files-or-prefixes, confidence)
    'morton_castle_scotland':    (['MORTON.'], 'high'),
    'castillo_de_pambre':        (['PAMBRE.'], 'high'),
    'castillo_de_perarrua':      (['PERARRUA.'], 'high'),
    'castillo_barcience_castilla': (['BARCIENCE.'], 'high'),
    'piazza_san_pellegrino':     (['pellegrino.png'], 'high'),
    'haunted_parish_ps1':        (['wood1.png','wood2.png','wood3.png','stone.png','roof1.png','wall1.png',
                                   'stoneTrim.png','stoneTrim2.png','dirt.png','dirt2.png','grass.png',
                                   'church1.png','maria.png','cat_statue.png','re1.png'], 'high'),
    'cistern_citadel_of':        (['2_millions_model_'], 'medium — 26-tile sandstone UDIM scan; Amman by scale+hue'),
    'thian_hock_keng':           (['material_9.png','material_10.png','material_10_4.png','material_11.png',
                                   '_009.png','P_1.png','P_1_3.png','Stone_Brushed_Khaki.png','APP_.png','APP__1.png'],
                                  'medium — the warm ornate cluster'),
    'scifi_corridor_sky':        (['2.png','7.png','ngv1_Ch2_02.png','material_12.png',
                                   'material_12.inline_conv_generated.png'], 'medium — the grayscale metal cluster'),
    'castillo_de_pioz':          (['MORTON.','PAMBRE.','PERARRUA.','BARCIENCE.'],
                                  'fallback — no set of its own; sibling-castle union palette'),
}

def gather(workdir, pats):
    out = []
    for f in sorted(os.listdir(workdir)):
        if any(f == p or f.startswith(p) for p in pats):
            out.append(os.path.join(workdir, f))
    return out

def learn_palette(files, k=10):
    px = []
    for fp in files[:8]:
        img = Image.open(fp).convert('RGB').resize((96, 96))
        px.extend(list(img.getdata()))
    # median-cut
    boxes = [px]
    while len(boxes) < k:
        boxes.sort(key=lambda b: -len(b))
        b = boxes.pop(0)
        if len(b) < 8: boxes.append(b); break
        rng = [(max(p[c] for p in b) - min(p[c] for p in b)) for c in range(3)]
        ax = rng.index(max(rng))
        b.sort(key=lambda p: p[ax])
        boxes += [b[:len(b)//2], b[len(b)//2:]]
    pal = []
    for b in boxes:
        n = len(b)
        pal.append(tuple(sum(p[c] for p in b) // n for c in range(3)))
    # drop near-duplicates, sort by luminance (dark -> light)
    uniq = []
    for c in pal:
        if all(sum(abs(a - b) for a, b in zip(c, u)) > 24 for u in uniq):
            uniq.append(c)
    uniq.sort(key=lambda c: 0.299*c[0] + 0.587*c[1] + 0.114*c[2])
    return uniq[:12]

def roof_index(pal):
    if len(pal) < 3: return None
    hs = [colorsys.rgb_to_hsv(*[c/255 for c in p]) for p in pal]
    med = sorted(h for h, s, v in hs)[len(hs)//2]
    best, bd = None, 0.0
    for i, (h, s, v) in enumerate(hs):
        if s < 0.12: continue
        d = min(abs(h - med), 1 - abs(h - med))
        if d > bd: bd, best = d, i
    return best if bd > 0.08 else None

def main(workdir, bpdir):
    for slug, (pats, conf) in BIND.items():
        path = os.path.join(bpdir, slug + '.txt')
        if not os.path.exists(path): print('missing bp', slug); continue
        files = gather(workdir, pats)
        if not files: print('no textures bound for', slug); continue
        pal = learn_palette(files)
        txt = open(path).read()
        txt = '\n'.join(l for l in txt.split('\n') if not l.startswith(';;P') and not l.startswith(';;C')
                        and not l.startswith(';; palette'))
        if not txt.endswith('\n'): txt += '\n'
        grid = [l for l in txt.split('\n') if l and not l.startswith(';')]
        H = [l[4:] for l in txt.split('\n') if l.startswith(';;H ')]
        R = [l[4:] for l in txt.split('\n') if l.startswith(';;R ')]
        S = [l[4:] for l in txt.split('\n') if l.startswith(';;S ')]
        ri = roof_index(pal)
        n = len(pal)
        txt += f";; palette learned from {len(files)} texture file(s) — binding {conf}; roof entry: {ri}\n"
        for i, (r, g, b) in enumerate(pal):
            txt += f";;P {i:x} {r:02X}{g:02X}{b:02X}\n"
        wall_lo, wall_hi = max(1, n//5), n - 2  # skip darkest shadow + brightest highlight for courses
        for z, row in enumerate(grid):
            line = ';;C '
            for x, ch in enumerate(row):
                if not (ch.isdigit() or ch in 'rc'):
                    line += 'g'; continue
                h = int(H[z][x], 16) if z < len(H) and x < len(H[z]) else 8
                r15 = int(R[z][x], 16) if z < len(R) and x < len(R[z]) else 4
                s3 = int(S[z][x], 16) if z < len(S) and x < len(S[z]) else 0
                if s3 > 0 and ri is not None and h >= 10:
                    idx = ri
                else:
                    band = wall_lo + (h * (wall_hi - wall_lo)) // 15
                    jit = ((x * 7 + z * 13 + r15) % 3) - 1
                    idx = max(0, min(n - 1, band + jit))
                line += format(idx, 'x')
            txt += line + '\n'
        open(path, 'w').write(txt)
        print(f"{slug}: {n} colors, roof={ri}, C rows={len(grid)}")

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
