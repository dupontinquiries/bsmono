# Break-test helper that cannot report a false clean.
# Compilation is checked SEPARATELY from the test count: a break that does not
# compile produces no "test result" line, and `grep -c FAILED` reads that as
# zero failures — which is indistinguishable from a break nothing catches.
# Bitten twice today (the JPEG channel swap and the .socket suffix).
brk() { # <file> <old> <new> <label> <pkg> <filter>
  cp "$BRK_BACKUP" "$1"
  python3 -c "
import sys
s=open(sys.argv[1]).read()
assert sys.argv[2] in s, 'ANCHOR MISS'
open(sys.argv[1],'w').write(s.replace(sys.argv[2],sys.argv[3],1))" "$1" "$2" "$3" || { echo "  [$4] ANCHOR MISS — nothing was changed"; return; }
  out=$(cargo test -q -p "$5" "$6" 2>&1)
  cp "$BRK_BACKUP" "$1"
  # `error: test failed, to rerun pass ...` is cargo reporting a TEST failure,
  # not a compile error — matching a bare `^error:` classified a genuine catch
  # as inconclusive. Match only the forms rustc/cargo use for a build failure.
  if echo "$out" | grep -qE "^error\[E[0-9]+\]|^error: could not compile"; then
    echo "  [$4] DID NOT COMPILE — inconclusive, not a pass"
  elif echo "$out" | grep -q "^test result: FAILED"; then
    echo "  [$4] caught: $(echo "$out" | grep -oE '[0-9]+ failed' | head -1)"
  else
    echo "  [$4] NOT CAUGHT — nothing tests this"
  fi
}
