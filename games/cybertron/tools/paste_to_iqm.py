#!/usr/bin/env python3
"""Turn base64 copied out of the Spine Grower / Part Editor (mobile export
fallback) back into a real file. Usage:
    python3 paste_to_iqm.py mypart.iqm   < paste.txt
  or paste when prompted:
    python3 paste_to_iqm.py mypart.iqm
Then move the .iqm into engine_src/data/ and point a part template at it."""
import base64, sys
out = sys.argv[1] if len(sys.argv) > 1 else "spine_part.iqm"
data = sys.stdin.read().strip() if not sys.stdin.isatty() else input("paste base64: ").strip()
open(out, "wb").write(base64.b64decode(data))
print("wrote", out, "(", len(base64.b64decode(data)), "bytes )")
