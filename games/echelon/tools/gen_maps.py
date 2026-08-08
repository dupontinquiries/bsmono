#!/usr/bin/env python3
"""Author Echelon maps as MapDef v2 RON. Four aligned char layers:

  height : '0'..'5' terrain level, '#' void chasm (air crosses, ground never)
  ramps  : '.' or digit k -> ramp joining level k and k+1 (tile terrain == k)
  bridges: '.' or digit b -> deck at level b (over lower terrain or void);
           ground units cross on top, or pass beneath when
           terrain + unit_depth <= b
  things : '.' none, 'a' alloy, 'f' flux, '1'/'2' starts

Everything is built in the top-left half and mirrored 180 degrees.
"""
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "data", "maps")


class M:
    def __init__(self, w, h, base="1"):
        self.w, self.h = w, h
        self.hgt = [[base] * w for _ in range(h)]
        self.rmp = [["."] * w for _ in range(h)]
        self.brg = [["."] * w for _ in range(h)]
        self.thg = [["."] * w for _ in range(h)]

    def g(self, layer):
        return {"h": self.hgt, "r": self.rmp, "b": self.brg, "t": self.thg}[layer]

    def rect(self, layer, x0, y0, x1, y1, c):
        g = self.g(layer)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                if 0 <= x < self.w and 0 <= y < self.h:
                    g[y][x] = c

    def disk(self, layer, cx, cy, r, c):
        g = self.g(layer)
        for y in range(cy - r, cy + r + 1):
            for x in range(cx - r, cx + r + 1):
                if (x - cx) ** 2 + (y - cy) ** 2 <= r * r and 0 <= x < self.w and 0 <= y < self.h:
                    g[y][x] = c

    def put(self, layer, x, y, c):
        self.g(layer)[y][x] = c

    def ramp(self, x0, y0, x1, y1, k):
        """ramp strip joining level k and k+1: sets terrain=k and ramp=k."""
        self.rect("h", x0, y0, x1, y1, str(k))
        self.rect("r", x0, y0, x1, y1, str(k))

    def deck(self, x0, y0, x1, y1, b):
        self.rect("b", x0, y0, x1, y1, str(b))

    def mirror180(self):
        for y in range(self.h):
            for x in range(self.w):
                if y * self.w + x < (self.h * self.w) // 2:
                    my, mx = self.h - 1 - y, self.w - 1 - x
                    self.hgt[my][mx] = self.hgt[y][x]
                    self.rmp[my][mx] = self.rmp[y][x]
                    self.brg[my][mx] = self.brg[y][x]
                    t = self.thg[y][x]
                    t = {"1": "2", "2": "1"}.get(t, t)
                    self.thg[my][mx] = t

    def ron(self, name, tileset):
        q = lambda rows: ",\n".join('        "%s"' % "".join(r) for r in rows)
        return (
            '(\n    name: "%s",\n    tileset: "%s",\n'
            "    alloy_amount: 20000,\n    flux_amount: 20000,\n"
            "    height: [\n%s,\n    ],\n    ramps: [\n%s,\n    ],\n"
            "    bridges: [\n%s,\n    ],\n    things: [\n%s,\n    ],\n)\n"
            % (name, tileset, q(self.hgt), q(self.rmp), q(self.brg), q(self.thg))
        )


def main_line(m, sx, sy):
    m.put("t", sx, sy, "1")
    for dx, dy in [(-4, -2), (-4, 0), (-4, 2), (-3, -3), (-3, 3), (-2, -4)]:
        m.put("t", sx + dx, sy + dy, "a")
    m.put("t", sx + 4, sy + 4, "f")


def confluence():
    """72x72, verdigris. Level-3 mains, level-1 plain, a level-0 canyon
    across the middle carrying contested flux, crossed by two level-3
    bridge spans (ramped approach pads). Armies choose: fight on the
    decks, or take the long canyon-floor flank and pass UNDERNEATH."""
    m = M(72, 72, base="1")
    m.rect("h", 0, 0, 71, 1, "#")
    m.rect("h", 0, 70, 71, 71, "#")
    m.rect("h", 0, 0, 1, 71, "#")
    m.rect("h", 70, 0, 71, 71, "#")

    # canyon (rows 32..39 after mirror; author top half rows 32..35)
    m.rect("h", 2, 32, 69, 35, "0")
    # canyon-entry ramp at the west end (mirror adds the east end)
    m.ramp(3, 31, 7, 31, 0)

    # bridge span A (x22..24) with ramped level-3 approach pad
    m.ramp(22, 26, 24, 26, 1)
    m.ramp(22, 27, 24, 27, 2)
    m.rect("h", 22, 28, 24, 31, "3")
    m.deck(22, 32, 24, 35, 3)
    # bridge span B (x47..49)
    m.ramp(47, 26, 49, 26, 1)
    m.ramp(47, 27, 49, 27, 2)
    m.rect("h", 47, 28, 49, 31, "3")
    m.deck(47, 32, 49, 35, 3)

    # main plateau (level 3) with a two-step ramp chain out
    m.rect("h", 4, 4, 20, 18, "3")
    m.ramp(21, 9, 21, 12, 2)
    m.ramp(22, 9, 22, 12, 1)
    main_line(m, 12, 10)

    # natural on the plain
    for dx, dy in [(-3, 0), (-3, 2), (-2, 4), (-1, -1), (0, 4), (-3, 4)]:
        m.put("t", 9 + dx, 24 + dy, "a")
    m.put("t", 14, 26, "f")

    # contested canyon riches (under the west span's shadow)
    m.put("t", 30, 34, "f")
    m.put("t", 33, 33, "a")
    m.put("t", 33, 35, "a")

    # air-only void chasm shaping the NE approach
    m.disk("h", 60, 10, 8, "#")
    m.disk("h", 52, 6, 6, "#")

    m.mirror180()
    return m.ron("Confluence", "verdigris")


def cauldron():
    """64x64, ashfall. Terraced kettle: level-1 field, level-2 corner
    mains, a level-3 terrace ring, a void moat crossed by four level-3
    bridges, a level-3 core ledge, and a level-5 crown holding the rich
    flux — climbed through 3->4->5 ramp notches."""
    m = M(64, 64, base="1")
    m.rect("h", 0, 0, 63, 1, "#")
    m.rect("h", 0, 62, 63, 63, "#")
    m.rect("h", 0, 0, 1, 63, "#")
    m.rect("h", 62, 0, 63, 63, "#")

    # terrace ring (level 3), band 20..43; hole overwritten below
    m.rect("h", 20, 20, 43, 43, "3")
    # ramp chains onto the terrace: north (x30..33) and west (y30..31)
    m.ramp(30, 18, 33, 18, 1)
    m.ramp(30, 19, 33, 19, 2)
    m.ramp(18, 30, 18, 31, 1)
    m.ramp(19, 30, 19, 31, 2)

    # void moat ring 24..39, then core ledge (3) and crown (5)
    m.rect("h", 24, 24, 39, 39, "#")
    m.rect("h", 26, 26, 37, 37, "3")
    m.rect("h", 28, 28, 35, 35, "5")
    # crown climb notches: ledge3 -> ramp3 -> ramp4 -> crown5 (N & W)
    m.ramp(31, 27, 32, 27, 3)
    m.ramp(31, 28, 32, 28, 4)
    m.ramp(27, 31, 27, 31, 3)
    m.ramp(28, 31, 28, 31, 4)

    # bridges over the moat: north (x31..32) and west (y31)
    m.deck(31, 24, 32, 25, 3)
    m.deck(24, 31, 25, 31, 3)

    # the prize on the crown
    m.put("t", 30, 30, "f")
    m.put("t", 33, 30, "a")
    m.put("t", 30, 33, "a")

    # corner main (level 2 mesa) with single-step ramp lips
    m.rect("h", 4, 4, 16, 16, "2")
    m.ramp(17, 8, 17, 11, 1)
    m.ramp(8, 17, 11, 17, 1)
    main_line(m, 10, 10)

    # NE void rib
    m.disk("h", 48, 14, 5, "#")

    m.mirror180()
    return m.ron("Cauldron", "ashfall")


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, ron in [("confluence", confluence()), ("cauldron", cauldron())]:
        with open(os.path.join(OUT, name + ".ron"), "w") as f:
            f.write(ron)
        print("wrote", name)
