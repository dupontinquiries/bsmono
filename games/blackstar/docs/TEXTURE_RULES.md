# Texture rules

Written after 262 orphan textures cost 51 MB on disk and **74 MB in git
history** — where they stayed long after being deleted, because a blob on any
commit is never unreachable.

## The rules

**1. A texture must be named by something.** An `.iqm` material, a `.c` string,
or a part list. The check is mechanical and lives in `tools/texture_audit.py`;
run it before adding a directory of images, not after.

**2. Neither engine loads textures today.** The C engine has no `stbi`, no
`fopen` on a `.png`, and the string `"textures/"` appears in no `.c` file. The
Rust port is SDF and vertex-colour. So a texture added now is *speculative* —
which is fine, but it should be a deliberate choice rather than a side effect of
copying an asset folder.

**3. Source resolution is not shipping resolution.** `stuff.png` was 9.9 MB.
Nothing sampled it. If a texture is kept for future work, keep it at the size
that work will use, and keep the master somewhere that is not this repo.

**4. Deleting is not enough — a big binary must never be committed twice.** This
is the expensive one. Git stores a full blob per version and none of them ever
becomes unreachable, so `gc` cannot help. The fleet's packs were written by five
successive commits and cost 385 MB reachable against 261 MB on disk; the deleted
textures cost 74 MB after deletion. For generated or vendored binaries:

* squash them into one commit rather than iterating in place, and
* if a rewrite is needed, `filter-repo` the whole path and re-add the current
  set as a single entry.

**5. Back up before stripping history.** An over-broad `filter-repo` in this
session removed the fold geometry of 42 chassis, recovered only because an
unrelated old bundle still had it. Copy the live files out, strip, re-add,
verify the count.

## The audit

```
python3 tools/texture_audit.py engine_src/data
```

Cross-references every `.png` against material names embedded in `.iqm`, `.png`
strings in C source, bare stems a `"textures/%s.png"` pattern could form, and
the `imported_*_parts.txt` part lists. Four sources rather than one grep,
because the first version found 38 references and the four together found 32
genuinely used files out of 298 — a single check would have deleted files that
were in use, or kept 260 that were not.
