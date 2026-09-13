#!/usr/bin/env python3
"""train_vocab.py — build the static token vocabulary for tinypack-kit's text codec.

Pipeline (all of it is here, on purpose — the loop is meant to be re-run on a
better corpus):

  1. pre-tokenise the corpus into "words" (whitespace attached to the front of
     the following word, GPT-2 style) and count them
  2. byte-level BPE over the word dictionary: start from single bytes, merge the
     most frequent adjacent pair N times; the merged strings become tokens
  3. force in a few tokens the corpus may under-represent (accented Latin-1
     letters as UTF-8 pairs, common escapes) so the codec never degrades to
     byte-literals on French
  4. parse the corpus with the SAME parser the Rust encoder uses (greedy
     longest-match, then optimal-parse against provisional code lengths) to get
     token frequencies
  5. length-limited canonical Huffman lengths (zlib-style overflow repair)
  6. emit vocab.rs: (token bytes, code length) pairs in vocab order; the Rust
     side rebuilds canonical codes from the lengths, so the lengths are the
     whole contract

usage: train_vocab.py --corpus a.txt [--corpus b.txt --weight 30] --vocab 4096 \
                      --out vocab.rs [--heldout x.txt ...]
"""
import argparse, collections, heapq, math, re, sys, time, zlib

MAX_TOKEN_LEN = 12          # bytes; bounds the encoder's lookahead
MAX_CODE_LEN = 20           # bits; keeps the decoder's tables tiny
EOS_RATE = 40               # one EOS per ~40 tokens (typical chat message)

# UTF-8 pairs for letters a French/Spanish/German chat message will hit.
FORCED = [c.encode() for c in "éèêëàâäçùûüôöîïñÉÈÀÇ"] + [b"\xe2\x80\x99", b"\xe2\x80\x94"]

WORD_RE = re.compile(rb"\s?[^\s]+|\s+")


def pretokenise(data: bytes) -> collections.Counter:
    return collections.Counter(WORD_RE.findall(data))


def bpe(words: collections.Counter, n_merges: int):
    """Incremental BPE. words: Counter[bytes] -> freq. Returns merge list (ordered)."""
    # each word is a list of symbols (bytes); track which words contain each pair
    wsyms = {w: [bytes([b]) for b in w] for w in words}
    pair_cnt = collections.Counter()
    pair_words = collections.defaultdict(set)
    for w, syms in wsyms.items():
        f = words[w]
        for a, b in zip(syms, syms[1:]):
            pair_cnt[(a, b)] += f
            pair_words[(a, b)].add(w)
    merges = []
    t0 = time.time()
    # lazy max-heap of (-count, pair); stale entries are skipped on pop
    heap = [(-c, p) for p, c in pair_cnt.items()]
    heapq.heapify(heap)
    while len(merges) < n_merges and heap:
        negc, pair = heapq.heappop(heap)
        if pair_cnt.get(pair, 0) != -negc or -negc <= 0:
            continue                      # stale
        a, b = pair
        if len(a) + len(b) > MAX_TOKEN_LEN:
            pair_cnt[pair] = 0            # never merge; drop it
            continue
        new = a + b
        merges.append(new)
        touched = collections.Counter()
        for w in list(pair_words[pair]):
            syms = wsyms[w]
            f = words[w]
            i = 0
            out = []
            while i < len(syms):
                if i + 1 < len(syms) and syms[i] == a and syms[i + 1] == b:
                    # remove old pair contributions around position i
                    if out:
                        touched[(out[-1], a)] -= f
                        touched[(out[-1], new)] += f
                    if i + 2 < len(syms):
                        touched[(b, syms[i + 2])] -= f
                        touched[(new, syms[i + 2])] += f
                    out.append(new)
                    i += 2
                else:
                    out.append(syms[i])
                    i += 1
            wsyms[w] = out
            # rebuild membership for this word (cheap: words are short)
            for p in set(zip(syms, syms[1:])):
                pair_words[p].discard(w)
            for p in set(zip(out, out[1:])):
                pair_words[p].add(w)
        touched[pair] = -pair_cnt[pair]
        for p, d in touched.items():
            if d == 0:
                continue
            pair_cnt[p] += d
            if pair_cnt[p] > 0 and p != pair:
                heapq.heappush(heap, (-pair_cnt[p], p))
        pair_cnt[pair] = 0
        if len(merges) % 500 == 0:
            print(f"  merges {len(merges)}  last={new!r} cnt={-negc}  {time.time()-t0:.0f}s", file=sys.stderr)
    return merges


def build_lookup(tokens):
    by_len = collections.defaultdict(dict)
    for i, t in enumerate(tokens):
        by_len[len(t)][t] = i
    return by_len


def parse_greedy(data: bytes, tokens, by_len):
    out = []
    i, n = 0, len(data)
    maxl = max(by_len)
    while i < n:
        for L in range(min(maxl, n - i), 0, -1):
            j = by_len[L].get(data[i:i + L])
            if j is not None:
                out.append(j)
                i += L
                break
    return out


def parse_optimal(data: bytes, tokens, by_len, lens):
    """Min-bits parse. Forward DP with the SAME iteration order and strict
    comparison as the Rust encoder (`text::dp`), so equal-cost ties resolve
    identically and test vectors match byte for byte."""
    n = len(data)
    maxl = max(by_len)
    INF = 1 << 60
    cost = [INF] * (n + 1)
    back = [-1] * (n + 1)
    cost[0] = 0
    for k in range(1, n + 1):
        best, bt = INF, -1
        lo = max(0, k - maxl)
        for j in range(k - 1, lo - 1, -1):          # shorter tokens first
            t = by_len[k - j].get(data[j:k])
            if t is None:
                continue
            c = cost[j] + lens[t]
            if c < best:
                best, bt = c, t
        cost[k] = best
        back[k] = bt
    out = []
    k = n
    while k > 0:
        t = back[k]
        out.append(t)
        k -= len(tokens[t])
    out.reverse()
    return out


def canonical_codes(lens):
    """RFC 1951 §3.2.2 canonical assignment; mirrors huffman::Code::from_lengths."""
    maxl = max(lens)
    count = [0] * (maxl + 1)
    for l in lens:
        if l: count[l] += 1
    count[0] = 0
    code, nxt = 0, [0] * (maxl + 2)
    for bits in range(1, maxl + 1):
        code = (code + count[bits - 1]) << 1
        nxt[bits] = code
    codes = [None] * len(lens)
    for i, l in enumerate(lens):
        if l:
            codes[i] = (nxt[l], l); nxt[l] += 1
    return codes


def encode_bytes(data: bytes, tokens, by_len, lens, codes, eos) -> bytes:
    acc, n, out = 0, 0, bytearray()
    for t in parse_optimal(data, tokens, by_len, lens) + [eos]:
        c, l = codes[t]
        acc = (acc << l) | c; n += l
        while n >= 8:
            n -= 8; out.append((acc >> n) & 0xff)
        acc &= (1 << n) - 1
    if n:
        out.append((acc << (8 - n)) & 0xff)
    return bytes(out)


def huffman_lengths(freqs, max_len):
    """Canonical Huffman code lengths, limited to max_len (zlib-style repair)."""
    n = len(freqs)
    heap = [(f if f > 0 else 1, i, None) for i, f in enumerate(freqs)]
    # build tree
    nodes = [(f, i) for f, i, _ in heap]
    heapq.heapify(nodes)
    parent = {}
    nxt = n
    while len(nodes) > 1:
        f1, a = heapq.heappop(nodes)
        f2, b = heapq.heappop(nodes)
        parent[a] = nxt
        parent[b] = nxt
        heapq.heappush(nodes, (f1 + f2, nxt))
        nxt += 1
    lens = [0] * n
    for i in range(n):
        d, x = 0, i
        while x in parent:
            x = parent[x]
            d += 1
        lens[i] = max(d, 1)
    # overflow repair: clamp, then push shallow codes deeper until Kraft <= 1
    if max(lens) > max_len:
        for i in range(n):
            lens[i] = min(lens[i], max_len)
        kraft = sum(2.0 ** -l for l in lens)
        order = sorted(range(n), key=lambda i: (-lens[i], freqs[i]))  # deepest first, then rarest
        k = 0
        while kraft > 1.0 + 1e-12:
            i = order[k % n]
            if lens[i] < max_len:
                kraft -= 2.0 ** -lens[i]
                lens[i] += 1
                kraft += 2.0 ** -lens[i]
            k += 1
        # sanity: still a complete-or-under prefix code
    assert sum(2.0 ** -l for l in lens) <= 1.0 + 1e-12
    return lens


def bits_of(parse, lens, eos):
    return sum(lens[j] for j in parse) + lens[eos]


def rs_bytes_literal(b: bytes) -> str:
    out = []
    for c in b:
        if c == 0x22: out.append('\\"')
        elif c == 0x5c: out.append("\\\\")
        elif 0x20 <= c < 0x7f: out.append(chr(c))
        elif c == 0x0a: out.append("\\n")
        elif c == 0x09: out.append("\\t")
        else: out.append(f"\\x{c:02x}")
    return 'b"' + "".join(out) + '"'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", action="append", required=True)
    ap.add_argument("--weight", action="append", type=int, help="repeat count for the corpus at the same index")
    ap.add_argument("--vocab", type=int, default=4096)
    ap.add_argument("--train-bytes", type=int, default=6_000_000, help="BPE training slice")
    ap.add_argument("--freq-bytes", type=int, default=3_000_000, help="frequency-estimation slice")
    ap.add_argument("--out", required=True)
    ap.add_argument("--heldout", action="append", default=[])
    ap.add_argument("--dump", help="write the raw token list here (one per line, repr)")
    ap.add_argument("--vectors", help="write cross-check vectors (hex in, hex out) for the Rust tests")
    args = ap.parse_args()

    parts = []
    for i, p in enumerate(args.corpus):
        d = open(p, "rb").read()
        w = (args.weight[i] if args.weight and i < len(args.weight) else 1)
        parts.append(d * w)
    # small weighted seeds go FIRST so a byte-limited slice never drops them
    parts.sort(key=len)
    corpus = b"\n".join(parts)
    train = corpus[: args.train_bytes]
    freqd = corpus[: args.freq_bytes]
    print(f"corpus {len(corpus)} B, BPE slice {len(train)} B, freq slice {len(freqd)} B", file=sys.stderr)

    words = pretokenise(train)
    print(f"{len(words)} unique words", file=sys.stderr)
    n_merges = args.vocab - 256 - 1 - len(FORCED)
    merges = bpe(words, n_merges)

    tokens = [bytes([i]) for i in range(256)]
    seen = set(tokens)
    for m in merges + FORCED:
        if m not in seen and len(m) <= MAX_TOKEN_LEN:
            tokens.append(m); seen.add(m)
    EOS = len(tokens)
    tokens.append(b"")            # EOS: empty token, index = last
    tokens_nz = tokens[:-1]
    by_len = build_lookup(tokens_nz)
    print(f"{len(tokens)} tokens incl. EOS; max len {max(len(t) for t in tokens)}", file=sys.stderr)

    # provisional lengths from greedy parse, then refine with optimal parse
    t0 = time.time()
    parse = parse_greedy(freqd, tokens_nz, by_len)
    print(f"greedy parse: {len(parse)} tokens {time.time()-t0:.0f}s", file=sys.stderr)
    for rnd in range(2):
        freqs = [0] * len(tokens)
        for j in parse: freqs[j] += 1
        freqs[EOS] = max(1, len(parse) // EOS_RATE)
        lens = huffman_lengths(freqs, MAX_CODE_LEN)
        t0 = time.time()
        parse = parse_optimal(freqd, tokens_nz, by_len, lens)
        print(f"optimal parse round {rnd}: {len(parse)} tokens, "
              f"{bits_of(parse, lens, EOS)/len(freqd):.3f} bits/byte  {time.time()-t0:.0f}s", file=sys.stderr)
    freqs = [0] * len(tokens)
    for j in parse: freqs[j] += 1
    freqs[EOS] = max(1, len(parse) // EOS_RATE)
    lens = huffman_lengths(freqs, MAX_CODE_LEN)
    print(f"code lengths: min {min(lens)} max {max(lens)}; EOS {lens[EOS]} bits", file=sys.stderr)

    # held-out report: per line, greedy vs optimal vs raw deflate-9
    for h in args.heldout:
        lines = [l for l in open(h, "rb").read().split(b"\n") if l.strip()]
        tot_raw = tot_g = tot_o = tot_z = 0
        for l in lines:
            g = bits_of(parse_greedy(l, tokens_nz, by_len), lens, EOS)
            o = bits_of(parse_optimal(l, tokens_nz, by_len, lens), lens, EOS)
            co = zlib.compressobj(9, zlib.DEFLATED, -15)
            z = len(co.compress(l) + co.flush()) * 8
            tot_raw += len(l) * 8; tot_g += g; tot_o += o; tot_z += z
        print(f"heldout {h}: {len(lines)} lines, {tot_raw//8} B raw | "
              f"greedy {math.ceil(tot_g/8)} B ({tot_g/(tot_raw/8):.2f} b/B) | "
              f"optimal {math.ceil(tot_o/8)} B ({tot_o/(tot_raw/8):.2f} b/B) | "
              f"deflate-9 {tot_z//8} B ({tot_z/(tot_raw/8):.2f} b/B)", file=sys.stderr)

    if args.vectors:
        codes = canonical_codes(lens)
        samples = [b"", b"a", b" ", bytes(range(256)), "\U0001F600\U0001F600 emoji only".encode(),
                   b"https://example.com/a/very/long/path?with=querystring&and=more",
                   b"the the the the the the the the the the the the"]
        for h in args.heldout:
            samples += [l for l in open(h, "rb").read().split(b"\n") if l.strip()]
        with open(args.vectors, "w") as f:
            f.write("# input_hex encoded_hex — produced by train_vocab.py (optimal parse); '-' = empty\n")
            for smp in samples:
                enc = encode_bytes(smp, tokens_nz, by_len, lens, codes, EOS)
                f.write(f"{smp.hex() or '-'} {enc.hex()}\n")
        print(f"wrote {len(samples)} vectors to {args.vectors}", file=sys.stderr)

    with open(args.out, "w", encoding="utf-8") as f:
        f.write("// GENERATED by shared/tinypack-kit/tools/train_vocab.py — do not edit by hand.\n")
        f.write(f"// vocab {len(tokens)} tokens (incl. EOS), max token {MAX_TOKEN_LEN} B, "
                f"max code {MAX_CODE_LEN} bits, corpus {len(corpus)} B.\n")
        f.write("// Entry = (token bytes, canonical Huffman code length). Codes are rebuilt\n")
        f.write("// from the lengths at startup; order matters (canonical assignment).\n")
        f.write(f"pub const N_TOKENS: usize = {len(tokens)};\n")
        f.write(f"pub const EOS: u16 = {EOS};\n")
        f.write(f"pub const MAX_TOKEN_LEN: usize = {MAX_TOKEN_LEN};\n")
        f.write(f"pub const MAX_CODE_LEN: u8 = {MAX_CODE_LEN};\n")
        f.write("pub static VOCAB: [(&[u8], u8); N_TOKENS] = [\n")
        for t, l in zip(tokens, lens):
            f.write(f"    ({rs_bytes_literal(t)}, {l}),\n")
        f.write("];\n")
    if args.dump:
        with open(args.dump, "w") as f:
            for t, l, fr in zip(tokens, lens, freqs):
                f.write(f"{l:2d} {fr:8d} {t!r}\n")
    print(f"wrote {args.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
