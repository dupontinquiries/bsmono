#!/usr/bin/env bash
# pure_test.sh <pure_module.rs> ... — compile each dependency-free module with
# rustc --test and run it (unit + property tests). The fast in-sandbox loop.
# Only valid for modules coupling_check.py says are PURE.
set -uo pipefail
fail=0
for f in "$@"; do
  n="$(basename "$f" .rs)"
  if rustc --test --edition 2021 "$f" -o "/tmp/ptest_$n" 2>"/tmp/ptest_$n.log"; then
    if "/tmp/ptest_$n" >"/tmp/ptest_$n.out" 2>&1; then
      echo "PASS  $f  ($(grep 'test result:' /tmp/ptest_$n.out | head -1))"
    else echo "FAIL  $f"; tail -8 "/tmp/ptest_$n.out"; fail=1; fi
  else echo "FAIL  $f (compile)"; tail -12 "/tmp/ptest_$n.log"; fail=1; fi
done
exit $fail
