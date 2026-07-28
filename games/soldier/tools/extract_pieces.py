#!/usr/bin/env python3
"""extract_pieces — cut reusable SET-PIECES from the learned blueprints (the Dead-by-Daylight
'palette set' idea): connected massing structures — a tower, a gatehouse, an arcade bay — as
stampable multi-cell prefabs any map can place, carrying their full appearance (grid + H/S/R/G
layers + the site palette + per-cell tints) so a piece dropped into a procedural biome still
wears its model's photograph and silhouette.

Run AFTER mapstudy + texlearn (reads the final blueprints). Writes blueprints/pieces/<slug>_<k>.txt:
  ; piece from <slug> (site <idx>) — WxH, N cells
  <grid rows>            ;;H/;;S/;;R/;;G rows (cropped)   ;;P palette lines (copied)  ;;C rows (cropped)
"""
import os, sys

SLUGS = ['church_of_st','haunted_parish_ps1','hercules_colossus_fountain','piazza_san_pellegrino',
         'castillo_de_pioz','castillo_barcience_castilla','cistern_citadel_of','morton_castle_scotland',
         'castillo_de_pambre','castillo_de_perarrua','lost_palace_church','thian_hock_keng','scifi_corridor_sky']

def main(bpdir, outdir):
    total = 0
    for site, slug in enumerate(SLUGS):
        p = os.path.join(bpdir, slug + '.txt')
        txt = open(p).read().split('\n')
        grid = [l for l in txt if l and not l.startswith(';')]
        H = [l[4:] for l in txt if l.startswith(';;H ')]
        S = [l[4:] for l in txt if l.startswith(';;S ')]
        R = [l[4:] for l in txt if l.startswith(';;R ')]
        G = [l[4:] for l in txt if l.startswith(';;G ')]
        C = [l[4:] for l in txt if l.startswith(';;C ')]
        P = [l for l in txt if l.startswith(';;P ')]
        gh, gw = len(grid), max(len(r) for r in grid)
        solid = lambda ch: ch.isdigit() or ch in 'rc'
        seen = [[False]*gw for _ in range(gh)]
        comps = []
        for z0 in range(gh):
            for x0 in range(len(grid[z0])):
                if seen[z0][x0] or not solid(grid[z0][x0]): continue
                st, cells = [(x0, z0)], []
                while st:
                    x, z = st.pop()
                    if x < 0 or z < 0 or z >= gh or x >= len(grid[z]) or seen[z][x] or not solid(grid[z][x]): continue
                    seen[z][x] = True; cells.append((x, z))
                    st += [(x+1,z),(x-1,z),(x,z+1),(x,z-1)]
                if 25 <= len(cells) <= 1400: comps.append(cells)
        comps.sort(key=len, reverse=True)
        for k, cells in enumerate(comps[:4]):
            xs = [c[0] for c in cells]; zs = [c[1] for c in cells]
            x0, x1 = max(0, min(xs)-1), min(gw-1, max(xs)+1)
            z0, z1 = max(0, min(zs)-1), min(gh-1, max(zs)+1)
            inside = set(cells)
            out = [f"; piece from {slug} (site {site}) — {x1-x0+1}x{z1-z0+1}, {len(cells)} cells"]
            def crop(rows, keep_all=False):
                o = []
                for z in range(z0, z1+1):
                    row = rows[z] if z < len(rows) else ''
                    line = ''
                    for x in range(x0, x1+1):
                        ch = row[x] if x < len(row) else ('^' if not keep_all else '0')
                        # outside the component: neutralize massing so the piece is JUST the structure
                        if not keep_all and solid(ch) and (x, z) not in inside: ch = '^'
                        line += ch
                    o.append(line)
                return o
            out += crop(grid)
            for tag, rows in [(';;H ', H), (';;S ', S), (';;R ', R), (';;G ', G), (';;C ', C)]:
                for line in crop(rows, keep_all=True): out.append(tag + line)
            out += P
            open(os.path.join(outdir, f"{slug}_{k}.txt"), 'w').write('\n'.join(out) + '\n')
            total += 1
        print(f"{slug}: {min(4, len(comps))} pieces")
    print(f"total {total} pieces")

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
