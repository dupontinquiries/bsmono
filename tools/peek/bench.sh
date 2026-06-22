#!/bin/sh
# bench.sh <source_file> — compares peek v1, peek v2, and sed on a source file and
# a 2M-line synthetic file. Times with date(ns). Run from a repo root.
P1=${PEEK1:-./target/release/peek}; P2=${PEEK2:-./target/release/peek2}
SRC=${1:-Cargo.toml}
ms() { echo "scale=1; ($2-$1)/1000000" | bc; }
run() { l="$1"; shift; s=$(date +%s%N); "$@" >/dev/null 2>&1; e=$(date +%s%N); printf "%-5s %7s ms\n" "$l" "$(ms $s $e)"; }
[ -f /tmp/big.txt ] || seq 1 2000000 | awk '{print "line " $1 " content"}' > /tmp/big.txt
echo "source ($SRC): 5 ranges/call"
run v1 $P1 "$SRC:5xx" "$SRC:10xx" "$SRC:15xx" "$SRC:20xx" "$SRC:25xx"
run v2 $P2 "$SRC:5xx" "$SRC:10xx" "$SRC:15xx" "$SRC:20xx" "$SRC:25xx"
echo "big (2M lines): 5 spread ranges/call"
run v1 $P1 /tmp/big.txt:5xx /tmp/big.txt:5000xx /tmp/big.txt:19999xx
run v2 $P2 /tmp/big.txt:5xx /tmp/big.txt:5000xx /tmp/big.txt:19999xx
