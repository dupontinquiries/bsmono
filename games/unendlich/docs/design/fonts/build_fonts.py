#!/usr/bin/env python3
"""
Build two TTF fonts for unendlich from a pixel-grid description, matching
the menu design language (blocky / voxel / sharp-cornered):

  1. UnendlichBlocks-Regular.ttf — a readable blocky Latin game font
     (uppercase, lowercase folded to caps, digits, basic punctuation).
  2. UnendlichGlyphs-Regular.ttf — UI/bending glyphs (diamond, brackets,
     chevrons, runic-style marks) mapped onto private-use + ASCII slots.

Each glyph is a small bitmap; every filled cell becomes a square contour,
so the result is genuinely "voxel" — square pixels, hard corners. Built
with fontTools.fontBuilder (no external design tools needed).
"""
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPM = 1000          # units per em
COLS, ROWS = 5, 7   # pixel grid for the Latin font
PIX = 110           # size of one pixel cell in font units
PIX_GAP = 18        # v3: gap inset per cell → thinner strokes, more legible
ORIGIN_X = 60       # left bearing
BASELINE = 0
ASCENT = 800
DESCENT = -200

# ── 5x7 pixel font (rows top->bottom, '#' = filled) ─────────────────────
# A compact, readable blocky face. Lowercase is folded to the same caps.
GLYPHS_5x7 = {
 'A': ["01110","10001","10001","11111","10001","10001","10001"],
 'B': ["11110","10001","10001","11110","10001","10001","11110"],
 'C': ["01111","10000","10000","10000","10000","10000","01111"],
 'D': ["11110","10001","10001","10001","10001","10001","11110"],
 'E': ["11111","10000","10000","11110","10000","10000","11111"],
 'F': ["11111","10000","10000","11110","10000","10000","10000"],
 'G': ["01111","10000","10000","10111","10001","10001","01111"],
 'H': ["10001","10001","10001","11111","10001","10001","10001"],
 'I': ["11111","00100","00100","00100","00100","00100","11111"],
 'J': ["00111","00010","00010","00010","00010","10010","01100"],
 'K': ["10001","10010","10100","11000","10100","10010","10001"],
 'L': ["10000","10000","10000","10000","10000","10000","11111"],
 'M': ["10001","11011","10101","10101","10001","10001","10001"],
 'N': ["10001","11001","10101","10011","10001","10001","10001"],
 'O': ["01110","10001","10001","10001","10001","10001","01110"],
 'P': ["11110","10001","10001","11110","10000","10000","10000"],
 'Q': ["01110","10001","10001","10001","10101","10010","01101"],
 'R': ["11110","10001","10001","11110","10100","10010","10001"],
 'S': ["01111","10000","10000","01110","00001","00001","11110"],
 'T': ["11111","00100","00100","00100","00100","00100","00100"],
 'U': ["10001","10001","10001","10001","10001","10001","01110"],
 'V': ["10001","10001","10001","10001","10001","01010","00100"],
 'W': ["10001","10001","10001","10101","10101","11011","10001"],
 'X': ["10001","10001","01010","00100","01010","10001","10001"],
 'Y': ["10001","10001","01010","00100","00100","00100","00100"],
 'Z': ["11111","00001","00010","00100","01000","10000","11111"],
 '0': ["01110","10011","10101","10101","11001","10001","01110"],
 '1': ["00100","01100","00100","00100","00100","00100","01110"],
 '2': ["01110","10001","00001","00110","01000","10000","11111"],
 '3': ["11110","00001","00001","01110","00001","00001","11110"],
 '4': ["00010","00110","01010","10010","11111","00010","00010"],
 '5': ["11111","10000","11110","00001","00001","10001","01110"],
 '6': ["01110","10000","10000","11110","10001","10001","01110"],
 '7': ["11111","00001","00010","00100","01000","01000","01000"],
 '8': ["01110","10001","10001","01110","10001","10001","01110"],
 '9': ["01110","10001","10001","01111","00001","00001","01110"],
 '.': ["00000","00000","00000","00000","00000","01100","01100"],
 ',': ["00000","00000","00000","00000","01100","00100","01000"],
 ':': ["00000","01100","01100","00000","01100","01100","00000"],
 '-': ["00000","00000","00000","11111","00000","00000","00000"],
 '/': ["00001","00010","00010","00100","01000","01000","10000"],
 '!': ["00100","00100","00100","00100","00100","00000","00100"],
 '?': ["01110","10001","00001","00110","00100","00000","00100"],
 "'": ["00100","00100","01000","00000","00000","00000","00000"],
 '(': ["00010","00100","01000","01000","01000","00100","00010"],
 ')': ["01000","00100","00010","00010","00010","00100","01000"],
 '+': ["00000","00100","00100","11111","00100","00100","00000"],
 '%': ["11001","11010","00100","01000","10011","00011","00000"],
 '*': ["00000","10101","01110","11111","01110","10101","00000"],
}

def pixel_glyph(pen, grid, cols=COLS, rows=ROWS, pix=PIX, ox=ORIGIN_X):
    """Emit one square contour per filled cell. Top row sits high; we map
    grid row 0 -> top (near ASCENT-ish) downward."""
    top = ROWS * pix  # glyph height baseline-relative top
    for ry, line in enumerate(grid):
        for cx, ch in enumerate(line):
            if ch != '1' and ch != '#':
                continue
            x0 = ox + cx * pix
            y1 = top - ry * pix
            y0 = y1 - pix
            x1 = x0 + pix
            # v3: inset each filled cell by half the gap on every side so
            # the strokes are thinner and adjacent pixels read separately.
            g = PIX_GAP / 2
            x0 += g; x1 -= g; y0 += g; y1 -= g
            pen.moveTo((x0, y0))
            pen.lineTo((x1, y0))
            pen.lineTo((x1, y1))
            pen.lineTo((x0, y1))
            pen.closePath()

def build_blocks_font(path):
    cmap = {}
    glyph_order = ['.notdef', 'space']
    pens = {}
    # .notdef = a hollow box
    p = TTGlyphPen(None)
    pixel_glyph(p, ["11111","10001","10001","10001","10001","10001","11111"])
    pens['.notdef'] = p.glyph()
    # space
    pens['space'] = TTGlyphPen(None).glyph()

    advance = ORIGIN_X * 2 + COLS * PIX  # uniform advance
    metrics = {'.notdef': (advance, ORIGIN_X), 'space': (advance, 0)}

    def add(char, name, grid):
        pen = TTGlyphPen(None)
        pixel_glyph(pen, grid)
        pens[name] = pen.glyph()
        glyph_order.append(name)
        cmap[ord(char)] = name
        metrics[name] = (advance, ORIGIN_X)

    import unicodedata
    used = set()
    for char, grid in GLYPHS_5x7.items():
        name = {'.':'period',',':'comma',':':'colon','-':'hyphen','/':'slash',
                '!':'exclam','?':'question',"'":'quotesingle','(':'parenleft',
                ')':'parenright','+':'plus','%':'percent','*':'asterisk'}.get(char, char)
        if char.isalnum():
            name = ('uni%04X' % ord(char)) if not char.isascii() else char
            name = char if char.isalpha() else ('zero one two three four five six seven eight nine'.split()[int(char)] if char.isdigit() else name)
        if name in used: continue
        used.add(name)
        add(char, name, grid)
        # fold lowercase to the same caps glyph
        if char.isupper():
            cmap[ord(char.lower())] = name

    cmap[ord(' ')] = 'space'

    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(glyph_order)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf({n: pens[n] for n in glyph_order})
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ASCENT, descent=DESCENT)
    fb.setupNameTable({
        "familyName": "Unendlich Blocks", "styleName": "Regular",
        "fullName": "Unendlich Blocks Regular", "psName": "UnendlichBlocks-Regular",
        "version": "Version 1.0", "uniqueFontIdentifier": "UnendlichBlocks-Regular-1.0",
    })
    fb.setupOS2(sTypoAscender=ASCENT, sTypoDescender=DESCENT, usWinAscent=ASCENT, usWinDescent=-DESCENT)
    fb.setupPost()
    fb.save(path)
    print("wrote", path, "glyphs:", len(glyph_order))

# ── glyph font: UI / bending marks, drawn on a finer 7x7 grid ───────────
G7 = 7
GLYPHS_SYM = {
 # diamond (filled)
 'D': ["0001000","0011100","0111110","1111111","0111110","0011100","0001000"],
 # diamond (outline)
 'd': ["0001000","0010100","0100010","1000001","0100010","0010100","0001000"],
 # left bracket-chevron  <
 'L': ["0000100","0001000","0010000","0100000","0010000","0001000","0000100"],
 # right bracket-chevron  >
 'R': ["0010000","0001000","0000100","0000010","0000100","0001000","0010000"],
 # bracket [
 'B': ["0111000","0100000","0100000","0100000","0100000","0100000","0111000"],
 # rune-ish mark (asymmetric, "bending")
 'r': ["1000000","1100000","1011000","1001100","0001011","0000110","0000001"],
 # four-point spark / resonance
 's': ["0001000","0001000","0101010","0011100","0101010","0001000","0001000"],
 # leaf / root tick
 'l': ["0000010","0000100","0001000","0010100","0101000","1010000","0100000"],
 # gem cluster
 'g': ["0010100","0111110","1111111","0111110","0011100","0001000","0000000"],
}

def build_glyph_font(path):
    cmap = {}
    glyph_order = ['.notdef', 'space']
    pens = {'space': TTGlyphPen(None).glyph()}
    p = TTGlyphPen(None); pixel_glyph(p, ["1111111","1000001","1000001","1000001","1000001","1000001","1111111"], cols=7, rows=7); pens['.notdef']=p.glyph()
    advance = ORIGIN_X*2 + 7*PIX
    metrics = {'.notdef':(advance,ORIGIN_X),'space':(advance,0)}
    for ch, grid in GLYPHS_SYM.items():
        name = 'glyph_'+ch
        pen = TTGlyphPen(None); pixel_glyph(pen, grid, cols=7, rows=7)
        pens[name] = pen.glyph(); glyph_order.append(name)
        cmap[ord(ch)] = name; metrics[name]=(advance,ORIGIN_X)
    cmap[ord(' ')]='space'
    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(glyph_order); fb.setupCharacterMap(cmap)
    fb.setupGlyf({n:pens[n] for n in glyph_order})
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ASCENT, descent=DESCENT)
    fb.setupNameTable({
        "familyName":"Unendlich Glyphs","styleName":"Regular",
        "fullName":"Unendlich Glyphs Regular","psName":"UnendlichGlyphs-Regular",
        "version":"Version 1.0","uniqueFontIdentifier":"UnendlichGlyphs-Regular-1.0"})
    fb.setupOS2(sTypoAscender=ASCENT, sTypoDescender=DESCENT, usWinAscent=ASCENT, usWinDescent=-DESCENT)
    fb.setupPost(); fb.save(path)
    print("wrote", path, "glyphs:", len(glyph_order))

if __name__ == '__main__':
    import sys
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    build_blocks_font(out + '/UnendlichBlocks-Regular.ttf')
    build_glyph_font(out + '/UnendlichGlyphs-Regular.ttf')
