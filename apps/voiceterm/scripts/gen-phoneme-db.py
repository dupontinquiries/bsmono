#!/usr/bin/env python3
"""
gen-phoneme-db.py — generate data/phoneme-db.txt from bundled manpages.

PA3 of the Phoneme alignment + tiered reading model arc (v0.1.246).

For each bundled manpage in data/manpages/, walks the paragraphs
that will become selectable at runtime, computes IPA phoneme
transcriptions via espeak-ng, and writes a line-based text file
that the runtime loader (src/phoneme_db.rs) reads via include_str!.

Output format (line-based, no external deps to parse):

  # phoneme-db v1 (generator: ...)
  M <manpage_name> <source_checksum>
  P <paragraph_id> <paragraph_text>
  W <word>\t<phonemes>
  W <word>\t<phonemes>
  ...
  P <next_id> ...
  ...
  M <next_manpage> ...
  ...

Lines:
  '#' = comment
  'M' = manpage record (single space sep)
  'P' = paragraph record (paragraph_id + text, single space then text)
  'W' = word record (word + TAB + phonemes; tab chosen so it can't
        appear inside either field naturally)

Why this format instead of JSON: the project deliberately avoids
serde_json (see Cargo.toml; existing JSON paths in archive.rs hand-
roll for flat objects only). Line-based parsing in Rust is trivial
and matches the project's "minimal deps" philosophy. jq won't work
on this file but `grep '^W'` and `awk -F'\\t'` do.

Run this when manpage source files change. The Rust runtime
compares text checksums and warns loudly if the data file is stale.

Requires: espeak-ng on $PATH. On Fedora/Asahi: `dnf install espeak-ng`.
On Ubuntu/Debian: `apt-get install espeak-ng`.

Idempotent: rerunning with no source changes produces identical
output (same paragraph order, same checksums).
"""
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANPAGES_DIR = ROOT / "data" / "manpages"
OUTPUT_PATH = ROOT / "data" / "phoneme-db.txt"

# Same paragraph word-count bounds as qml_gen::extended_word_bounds()
# in src/qml_gen.rs. Paragraphs outside this range aren't selectable
# at runtime so generating phonemes for them is wasted work.
WC_MIN = 3
WC_MAX = 200


def espeak_version() -> str:
    try:
        out = subprocess.check_output(["espeak-ng", "--version"], text=True)
        # First line looks like: "eSpeak NG text-to-speech: 1.51 ..."
        return out.strip().split("\n")[0]
    except Exception as e:
        return f"unknown ({e})"


def strip_markdown_inline(text: str) -> str:
    """Strip inline markdown markup so the prose-only text is what we
    transcribe. Mirrors the runtime's strip_inline_markup logic
    closely enough for phoneme generation purposes.
    """
    # Code spans: `foo` → foo
    text = re.sub(r"`([^`]+)`", r"\1", text)
    # Bold: **foo** → foo
    text = re.sub(r"\*\*([^*]+)\*\*", r"\1", text)
    # Italic: *foo* → foo (single-star)
    text = re.sub(r"(?<!\*)\*([^*]+)\*(?!\*)", r"\1", text)
    # Links: [text](url) → text
    text = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", text)
    return text


def heading_level(line: str) -> int:
    """Return heading level (1-6) or 0 if not a heading."""
    stripped = line.lstrip()
    if not stripped.startswith("#"):
        return 0
    i = 0
    while i < len(stripped) and stripped[i] == "#":
        i += 1
    if i > 6:
        return 0
    if i < len(stripped) and stripped[i] != " ":
        return 0
    return i


def is_ul_item(line: str) -> bool:
    return line.lstrip().startswith(("- ", "* ", "+ "))


def is_ol_item(line: str) -> bool:
    return bool(re.match(r"^\s*\d+\.\s", line))


def extract_paragraphs(md: str) -> list[str]:
    """Walk a markdown source and emit the same set of paragraph-like
    blocks the runtime will wrap as selectable. Order matches runtime
    so paragraph IDs line up.

    Selectable blocks (per src/manpages.rs):
      - Headings (h1-h3): one block per heading
      - Prose paragraphs: blank-line-separated, no special prefix
      - Unordered lists: full list as one block
      - Ordered lists: full list as one block
      - Fenced code blocks: one block per fence pair
      - Tables: one block per table (not generated here; tables
        in bundled manpages are rare and would need more parsing)
    """
    paragraphs: list[str] = []
    lines = md.splitlines()
    i = 0
    in_code = False
    code_buf: list[str] = []

    while i < len(lines):
        line = lines[i]
        stripped = line.lstrip()

        # Fenced code block boundary.
        if stripped.startswith("```"):
            if not in_code:
                # Open fence.
                in_code = True
                code_buf = []
            else:
                # Close fence — emit collected code as a block.
                paragraphs.append("\n".join(code_buf))
                in_code = False
                code_buf = []
            i += 1
            continue

        if in_code:
            code_buf.append(line)
            i += 1
            continue

        # Blank lines separate prose paragraphs.
        if not stripped:
            i += 1
            continue

        # Heading (only h1-h3 selectable per qml_gen).
        lvl = heading_level(stripped)
        if lvl > 0:
            if lvl <= 3:
                text = stripped[lvl:].strip()
                paragraphs.append(text)
            i += 1
            continue

        # Unordered list: greedily gather items.
        if is_ul_item(stripped):
            items = []
            while i < len(lines) and is_ul_item(lines[i].lstrip()):
                items.append(lines[i].lstrip()[2:].strip())
                # Continuation lines (same-indent, no marker) merge in.
                j = i + 1
                while j < len(lines):
                    nxt = lines[j]
                    nxt_stripped = nxt.lstrip()
                    if not nxt_stripped:
                        break
                    if (is_ul_item(nxt_stripped) or is_ol_item(nxt_stripped)
                            or heading_level(nxt_stripped) > 0
                            or nxt_stripped.startswith("```")):
                        break
                    items[-1] += " " + nxt_stripped
                    j += 1
                i = j
            # Items joined by " · " mirrors the runtime's joined text.
            paragraphs.append(" \u00B7 ".join(items))
            continue

        # Ordered list: same shape as ul.
        if is_ol_item(stripped):
            items = []
            while i < len(lines) and is_ol_item(lines[i].lstrip()):
                items.append(re.sub(r"^\s*\d+\.\s+", "", lines[i]).strip())
                j = i + 1
                while j < len(lines):
                    nxt = lines[j]
                    nxt_stripped = nxt.lstrip()
                    if not nxt_stripped:
                        break
                    if (is_ul_item(nxt_stripped) or is_ol_item(nxt_stripped)
                            or heading_level(nxt_stripped) > 0
                            or nxt_stripped.startswith("```")):
                        break
                    items[-1] += " " + nxt_stripped
                    j += 1
                i = j
            paragraphs.append(" \u00B7 ".join(items))
            continue

        # Plain prose paragraph: gather lines until blank.
        buf = [stripped]
        j = i + 1
        while j < len(lines):
            nxt = lines[j]
            nxt_stripped = nxt.lstrip()
            if not nxt_stripped:
                break
            if (is_ul_item(nxt_stripped) or is_ol_item(nxt_stripped)
                    or heading_level(nxt_stripped) > 0
                    or nxt_stripped.startswith("```")):
                break
            buf.append(nxt_stripped)
            j += 1
        paragraphs.append(" ".join(buf))
        i = j

    return paragraphs


# Tokenizer for word splitting. Keeps simple words; preserves
# hyphens and apostrophes inside words; drops surrounding
# punctuation. Mirrors what espeak handles naturally.
WORD_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9'\-_]*")


def words_of(paragraph_text: str) -> list[str]:
    stripped = strip_markdown_inline(paragraph_text)
    return WORD_RE.findall(stripped)


def espeak_word(word: str) -> str:
    """Run espeak-ng on a single word, return IPA phoneme string.
    Strips leading/trailing whitespace from the result.
    """
    try:
        out = subprocess.check_output(
            ["espeak-ng", "-q", "-x", "--ipa", word],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        return out.strip()
    except Exception as e:
        return f"<ERR:{e}>"


def espeak_batch(words: list[str]) -> list[str]:
    """Batch espeak-ng calls by sending newline-separated words on
    stdin. One subprocess per BATCH_SIZE words. Much faster than
    one process per word.
    """
    if not words:
        return []
    # espeak preserves line boundaries in --ipa output when given
    # newline-separated stdin: each input line gets one output
    # line. Verified empirically.
    BATCH_SIZE = 200
    results: list[str] = []
    for i in range(0, len(words), BATCH_SIZE):
        batch = words[i : i + BATCH_SIZE]
        # Some words become multi-word in espeak output. To keep
        # 1:1 mapping, send each as its own line; espeak emits
        # one phoneme line per input line.
        joined = "\n".join(batch)
        try:
            proc = subprocess.run(
                ["espeak-ng", "-q", "-x", "--ipa"],
                input=joined,
                text=True,
                capture_output=True,
                check=True,
            )
            lines = proc.stdout.splitlines()
            # Pad if espeak emitted fewer lines than expected
            # (e.g., empty word lines might be skipped).
            while len(lines) < len(batch):
                lines.append("")
            for j, ph in enumerate(lines[: len(batch)]):
                results.append(ph.strip())
        except subprocess.CalledProcessError as e:
            # Fall back to per-word for this batch.
            for w in batch:
                results.append(espeak_word(w))
    return results


def checksum(text: str) -> str:
    h = hashlib.blake2b(text.encode("utf-8"), digest_size=16)
    return "blake2:" + h.hexdigest()


def process_manpage(name: str, path: Path) -> dict:
    raw = path.read_text(encoding="utf-8")
    paragraphs = extract_paragraphs(raw)
    # Filter by word count to match runtime selection bounds.
    filtered: list[tuple[int, str, list[str]]] = []  # (id, text, words)
    for i, p in enumerate(paragraphs):
        ws = words_of(p)
        if WC_MIN <= len(ws) <= WC_MAX:
            filtered.append((i, p, ws))

    # Collect all words, batch espeak, then redistribute.
    all_words: list[str] = []
    word_origin: list[tuple[int, int]] = []  # (paragraph_idx_in_filtered, word_idx)
    for fi, (_, _, ws) in enumerate(filtered):
        for wi, w in enumerate(ws):
            all_words.append(w)
            word_origin.append((fi, wi))

    print(f"  {name}: {len(filtered)} selectable paragraphs, {len(all_words)} words", file=sys.stderr)
    phonemes = espeak_batch(all_words)

    # Build per-paragraph word lists.
    out_paragraphs = []
    para_words: dict[int, list[dict]] = {}
    for idx, (fi, wi) in enumerate(word_origin):
        para_words.setdefault(fi, []).append(
            {"text": all_words[idx], "phonemes": phonemes[idx]}
        )

    for fi, (pid, text, _ws) in enumerate(filtered):
        out_paragraphs.append({
            "id": pid,
            "text": strip_markdown_inline(text),
            "words": para_words.get(fi, []),
        })

    return {
        "source_checksum": checksum(raw),
        "paragraphs": out_paragraphs,
    }


def emit_db(db: dict, out_path: Path) -> None:
    """Write the database in the line-based format. Sanitization:
    - Newlines and tabs in paragraph text or words are escaped (\\n, \\t).
      Required because the format itself uses them as separators.
    - No external escaping otherwise; the format is forgiving.
    """
    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t")

    lines = []
    # v0.1.286: stronger DO-NOT-EDIT signpost. Previous one-line
    # header was easy to miss when grep-walking the data/ tree;
    # contributors might tweak a phoneme by hand and have it
    # silently overwritten on next regen. This block names the
    # generator script explicitly so future contributors find it.
    lines.append("# ════════════════════════════════════════════════════════════════")
    lines.append("# GENERATED FILE — DO NOT EDIT BY HAND.")
    lines.append("# Regenerate with: python3 scripts/gen-phoneme-db.py")
    lines.append("# Source of truth: data/manpages/*.md")
    lines.append("# ════════════════════════════════════════════════════════════════")
    lines.append(f"# phoneme-db v{db['version']} (generator: {db['generator']})")
    lines.append(f"# ipa: {'true' if db['ipa'] else 'false'}")
    for name in sorted(db["manpages"].keys()):
        m = db["manpages"][name]
        lines.append(f"M {name} {m['source_checksum']}")
        for p in m["paragraphs"]:
            lines.append(f"P {p['id']} {esc(p['text'])}")
            for w in p["words"]:
                lines.append(f"W {esc(w['text'])}\t{esc(w['phonemes'])}")
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    if not MANPAGES_DIR.is_dir():
        print(f"ERROR: {MANPAGES_DIR} not found", file=sys.stderr)
        sys.exit(1)

    try:
        subprocess.check_output(["espeak-ng", "--version"], stderr=subprocess.DEVNULL)
    except Exception:
        print("ERROR: espeak-ng not found on $PATH.", file=sys.stderr)
        print("  Install via: apt-get install espeak-ng  (or dnf install espeak-ng)", file=sys.stderr)
        sys.exit(1)

    db = {
        "version": 1,
        "generator": espeak_version(),
        "ipa": True,
        "manpages": {},
    }

    for path in sorted(MANPAGES_DIR.glob("*.md")):
        name = path.stem
        print(f"processing {name}.md", file=sys.stderr)
        db["manpages"][name] = process_manpage(name, path)

    emit_db(db, OUTPUT_PATH)
    size_kb = OUTPUT_PATH.stat().st_size / 1024
    print(f"wrote {OUTPUT_PATH} ({size_kb:.1f} KB)", file=sys.stderr)


if __name__ == "__main__":
    main()
