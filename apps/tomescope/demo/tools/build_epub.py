#!/usr/bin/env python3
"""build_epub.py -- turn the Elementals docx (via pandoc markdown) into an
EPUB3 that tomescope can shelve, page, and outline.

The source docx defines Heading1..6 but never applies them -- every heading is
bold direct formatting -- so there is no structure to inherit. This script
infers it from the bold-only lines, then emits real <h1>/<h2>.

Targets read off apps/tomescope/src/formats_epub.rs:
  container.xml -> OPF -> metadata + manifest + spine
  TOC from the EPUB3 nav doc, NCX as fallback   (both written here)
  cover via meta[name=cover] -> id, or properties="cover-image"  (both set)
"""
import html, os, re, shutil, sys, zipfile

SRC_MD = "elementals_raw.md"
MEDIA = "elementals_media/media"
OUT = "Elementals.epub"

TITLE = "Elementals — Book Two: The Absence"
AUTHOR = "Kessler DuPont"
DESC = ("A story & screen development package: expanded chapters 11-13, "
        "film previsualization, and franchise concepts.")

# Bold-only lines that start a new spine document.
PART_RE = re.compile(r"^\*\*(PART [A-Z]+|Appendix.*)\*\*$")
CHAP_RE = re.compile(r"^\*\*(CHAPTER [A-Z]+)\*\*$")
BOLD_RE = re.compile(r"^\*\*(.+?)\*\*$")
IMG_RE = re.compile(r"^!\[\]\(([^)]+)\)")


def inline(s):
    """Markdown inline -> XHTML. Order matters: images already stripped."""
    s = html.escape(s, quote=False)
    s = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"(?<!\*)\*([^*]+?)\*(?!\*)", r"<em>\1</em>", s)
    s = s.replace("---", "\u2014").replace("--", "\u2013")
    s = s.replace("\\'", "\u2019").replace("\\\"", "\u201d")
    return s


def parse(md_path):
    """-> [ {title, level, blocks:[('h',lvl,txt)|('p',txt)|('img',src)|('li',txt)]} ]"""
    lines = open(md_path, encoding="utf-8").read().splitlines()
    docs, cur = [], None
    i, para = 0, []

    def flush():
        if para:
            cur["blocks"].append(("p", " ".join(para)))
            para.clear()

    def start(title, level):
        nonlocal cur
        flush()
        cur = {"title": title, "level": level, "blocks": []}
        docs.append(cur)

    start("Front Matter", 1)
    while i < len(lines):
        ln = lines[i].rstrip()

        m = IMG_RE.match(ln)
        if m:
            flush()
            # pandoc wraps the {width=...} attr onto the next line
            src = m.group(1)
            cur["blocks"].append(("img", os.path.basename(src)))
            while i + 1 < len(lines) and lines[i + 1].strip().startswith(("height=", "width=", "}")):
                i += 1
            i += 1
            continue

        if not ln.strip():
            flush()
            i += 1
            continue

        if ln.startswith(("- ", "* ")):
            flush()
            cur["blocks"].append(("li", ln[2:]))
            i += 1
            continue

        pm, cm, bm = PART_RE.match(ln), CHAP_RE.match(ln), BOLD_RE.match(ln)
        if pm or cm:
            label = (pm or cm).group(1)
            # the following bold line is the subtitle/title of the part/chapter
            sub = ""
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            if j < len(lines):
                sm = BOLD_RE.match(lines[j].rstrip())
                if sm:
                    sub = sm.group(1)
                    i = j
            label = label.replace("---", "\u2014").replace("--", "\u2013").strip()
            sub = sub.replace("---", "\u2014").replace("--", "\u2013").strip()
            full = f"{label} \u2014 {sub}" if sub else label
            start(full, 1)
            cur["blocks"].append(("h", 1, full))
            i += 1
            continue

        if bm and len(bm.group(1)) < 90:
            flush()
            cur["blocks"].append(("h", 2, bm.group(1)))
            i += 1
            continue

        para.append(ln.strip())
        i += 1
    flush()
    return [d for d in docs if d["blocks"]]


def xhtml(doc, n):
    out = ['<?xml version="1.0" encoding="utf-8"?>',
           '<!DOCTYPE html>',
           '<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">',
           f"<head><title>{html.escape(doc['title'])}</title>",
           '<meta charset="utf-8"/>',
           '<link rel="stylesheet" type="text/css" href="style.css"/></head>',
           '<body>']
    for b in doc["blocks"]:
        if b[0] == "h":
            lvl, txt = b[1], b[2]
            aid = f"h{n}-{abs(hash(txt)) % 10000}"
            out.append(f'<h{lvl} id="{aid}">{inline(txt)}</h{lvl}>')
        elif b[0] == "p":
            out.append(f"<p>{inline(b[1])}</p>")
        elif b[0] == "li":
            out.append(f"<ul><li>{inline(b[1])}</li></ul>")
        elif b[0] == "img":
            out.append(f'<div class="fig"><img src="images/{b[1]}" alt="Storyboard panel"/></div>')
    out.append("</body></html>")
    return "\n".join(out)


CSS = """\
body { font-family: Georgia, serif; line-height: 1.55; margin: 1em; }
h1 { font-size: 1.5em; margin: 1.6em 0 0.6em; page-break-before: always;
     border-bottom: 1px solid #b9a6c9; padding-bottom: .25em; }
h2 { font-size: 1.15em; margin: 1.4em 0 0.4em; color: #4a3b5c; }
p  { margin: 0 0 0.85em; text-align: justify; }
ul { margin: 0 0 0.85em 1.2em; }
.fig { margin: 1.2em 0; text-align: center; page-break-inside: avoid; }
.fig img { max-width: 100%; height: auto; }
"""


def make_cover(path):
    """Cover in the book's own palette. Pillow is already present."""
    from PIL import Image, ImageDraw
    W, H = 1200, 1800
    im = Image.new("RGB", (W, H), (14, 10, 20))
    d = ImageDraw.Draw(im)
    for y in range(H):                       # vertical gradient, pale -> void
        t = y / H
        d.line([(0, y), (W, y)],
               fill=(int(222 - 200 * t), int(216 - 200 * t), int(228 - 196 * t)))
    cx, cy = W // 2, int(H * 0.60)           # the Absence: concentric rings
    for r in range(520, 0, -26):
        a = 1.0 - r / 560
        d.ellipse([cx - r, cy - r // 2.4, cx + r, cy + r // 2.4],
                  outline=(int(120 + 90 * a), int(40 + 30 * a), int(150 + 80 * a)),
                  width=2)
    for r in range(90, 0, -8):
        v = int(255 * (1 - r / 95))
        d.ellipse([cx - r, cy - r // 2.4, cx + r, cy + r // 2.4],
                  fill=(min(255, 180 + v // 3), 60 + v // 2, min(255, 200 + v // 4)))
    d.rectangle([60, 60, W - 60, H - 60], outline=(90, 70, 110), width=3)
    im.save(path, "PNG")


def build():
    if not os.path.exists(SRC_MD):
        sys.exit(f"missing {SRC_MD} -- run pandoc first")
    docs = parse(SRC_MD)
    stage = "epub_stage"
    shutil.rmtree(stage, ignore_errors=True)
    for d in ("META-INF", "OEBPS/images"):
        os.makedirs(os.path.join(stage, d), exist_ok=True)

    make_cover(os.path.join(stage, "OEBPS/images/cover.png"))
    used = set()
    for d in docs:
        for b in d["blocks"]:
            if b[0] == "img":
                used.add(b[1])
    for f in used:
        src = os.path.join(MEDIA, f)
        if os.path.exists(src):
            shutil.copy(src, os.path.join(stage, "OEBPS/images", f))

    open(os.path.join(stage, "OEBPS/style.css"), "w").write(CSS)
    files = []
    for n, d in enumerate(docs):
        name = f"ch{n:02d}.xhtml"
        open(os.path.join(stage, "OEBPS", name), "w", encoding="utf-8").write(xhtml(d, n))
        files.append((name, d["title"]))

    # cover page (its own spine item so the shelf art also opens as page 1)
    open(os.path.join(stage, "OEBPS/cover.xhtml"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="utf-8"?><!DOCTYPE html>'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Cover</title>'
        '<meta charset="utf-8"/></head><body style="margin:0">'
        '<img src="images/cover.png" alt="Cover" style="width:100%"/>'
        "</body></html>")

    manifest = ['<item id="cover-image" href="images/cover.png" media-type="image/png" properties="cover-image"/>',
                '<item id="coverpage" href="cover.xhtml" media-type="application/xhtml+xml"/>',
                '<item id="css" href="style.css" media-type="text/css"/>',
                '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>',
                '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>']
    spine = ['<itemref idref="coverpage"/>']
    for n, (name, _t) in enumerate(files):
        manifest.append(f'<item id="c{n}" href="{name}" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="c{n}"/>')
    for f in sorted(used):
        manifest.append(f'<item id="img{abs(hash(f))%99999}" href="images/{f}" media-type="image/png"/>')

    opf = f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
 <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:identifier id="bookid">urn:uuid:elementals-book-two-absence</dc:identifier>
  <dc:title>{html.escape(TITLE)}</dc:title>
  <dc:creator>{html.escape(AUTHOR)}</dc:creator>
  <dc:language>en</dc:language>
  <dc:description>{html.escape(DESC)}</dc:description>
  <meta property="dcterms:modified">2026-09-16T00:00:00Z</meta>
  <meta name="cover" content="cover-image"/>
 </metadata>
 <manifest>
  {chr(10).join('  ' + m for m in manifest)}
 </manifest>
 <spine toc="ncx">
  {chr(10).join('  ' + s for s in spine)}
 </spine>
</package>"""
    open(os.path.join(stage, "OEBPS/content.opf"), "w", encoding="utf-8").write(opf)

    nav = ['<?xml version="1.0" encoding="utf-8"?><!DOCTYPE html>',
           '<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">',
           "<head><title>Contents</title><meta charset=\"utf-8\"/></head><body>",
           '<nav epub:type="toc" id="toc"><h1>Contents</h1><ol>']
    for name, t in files:
        nav.append(f'<li><a href="{name}">{html.escape(t)}</a></li>')
    nav.append("</ol></nav></body></html>")
    open(os.path.join(stage, "OEBPS/nav.xhtml"), "w", encoding="utf-8").write("\n".join(nav))

    ncx = ['<?xml version="1.0" encoding="utf-8"?>',
           '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">',
           '<head><meta name="dtb:uid" content="urn:uuid:elementals-book-two-absence"/></head>',
           f"<docTitle><text>{html.escape(TITLE)}</text></docTitle><navMap>"]
    for n, (name, t) in enumerate(files):
        ncx.append(f'<navPoint id="n{n}" playOrder="{n+1}">'
                   f"<navLabel><text>{html.escape(t)}</text></navLabel>"
                   f'<content src="{name}"/></navPoint>')
    ncx.append("</navMap></ncx>")
    open(os.path.join(stage, "OEBPS/toc.ncx"), "w", encoding="utf-8").write("\n".join(ncx))

    open(os.path.join(stage, "META-INF/container.xml"), "w").write(
        '<?xml version="1.0"?>\n<container version="1.0" '
        'xmlns="urn:oasis:names:tc:opendocument:xmlns:container">\n'
        '<rootfiles><rootfile full-path="OEBPS/content.opf" '
        'media-type="application/oebps-package+xml"/></rootfiles></container>')

    # mimetype MUST be first and STORED, per OCF
    if os.path.exists(OUT):
        os.remove(OUT)
    with zipfile.ZipFile(OUT, "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip",
                   compress_type=zipfile.ZIP_STORED)
        for dp, _dn, fn in os.walk(stage):
            for f in sorted(fn):
                p = os.path.join(dp, f)
                z.write(p, os.path.relpath(p, stage), zipfile.ZIP_DEFLATED)
    return docs, files, used


if __name__ == "__main__":
    docs, files, used = build()
    print(f"{len(files)} spine documents, {len(used)} images -> {OUT} "
          f"({os.path.getsize(OUT)//1024} KB)")
    for name, t in files:
        print(f"   {name}  {t}")
