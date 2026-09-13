"""Extract English prose from the monorepo's markdown for vocab training.
Strips fenced code, inline code, URLs, table rows, and path-heavy lines so the
vocabulary learns words, not identifiers."""
import os, re, sys, random
root = sys.argv[1]
out = sys.argv[2]
fence = re.compile(r"```.*?```", re.S)
inline = re.compile(r"`[^`\n]*`")
url = re.compile(r"https?://\S+")
md_link = re.compile(r"\[([^\]]*)\]\([^)]*\)")
buf = []
n_files = 0
for dp, dn, fn in os.walk(root):
    dn[:] = [d for d in dn if d not in ("vendor", "target", "node_modules", ".git", "testdata")]
    for f in fn:
        if not f.endswith(".md"): continue
        p = os.path.join(dp, f)
        try: s = open(p, encoding="utf-8").read()
        except Exception: continue
        n_files += 1
        s = fence.sub(" ", s)
        s = inline.sub(" ", s)
        s = url.sub(" ", s)
        s = md_link.sub(r"\1", s)
        for line in s.split("\n"):
            t = line.strip()
            if not t: continue
            if t.startswith("|") or t.startswith("<"): continue
            t = re.sub(r"^[#>*\-\d.\s]+", "", t)          # heading/list markers
            t = re.sub(r"[*_]{1,3}", "", t)               # emphasis markers
            if len(t) < 20: continue
            # drop lines that are mostly code-ish (paths, snake_case, hex)
            words = t.split()
            codey = sum(1 for w in words if ("/" in w or "_" in w or "::" in w or "(" in w or w.startswith("0x")))
            if codey > len(words) * 0.15: continue
            buf.append(t)
random.seed(1)
random.shuffle(buf)
text = "\n".join(buf)
open(out, "w", encoding="utf-8").write(text)
print(f"{n_files} files -> {len(buf)} lines, {len(text.encode())} bytes")
