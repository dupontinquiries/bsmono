#!/usr/bin/env bash
# force_recompile_shader.sh <stem>  — e.g. force_recompile_shader.sh chunk_mesh_octree
# Deletes a shader's compiled .spv + hash sidecar so build.rs MUST recompile it
# from source on the next build. Use after editing any shader source (see
# SESSION_HANDOFF.md s1: the stale-.spv trap).
set -euo pipefail
stem="${1:?usage: force_recompile_shader.sh <shader-stem-without-extension-or-with>}"
# accept "chunk_mesh_octree" or "chunk_mesh_octree.comp"
base="$(basename "$stem")"
shopt -s nullglob
found=0
for ext in comp frag vert; do
  for f in assets/shaders/${base%.*}.${ext}.spv assets/shaders/${base%.*}.${ext}.spv.hash; do
    if [ -e "$f" ]; then rm -v "$f"; found=1; fi
  done
done
# also try exact-name match (if stem already had an extension)
for f in assets/shaders/${base}.spv assets/shaders/${base}.spv.hash; do
  if [ -e "$f" ]; then rm -v "$f"; found=1; fi
done
[ "$found" = 1 ] && echo "→ rebuild now; build.rs will regenerate the .spv." \
                  || echo "no matching .spv found for '$stem' (already clean?)."
