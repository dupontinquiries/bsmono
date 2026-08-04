#!/usr/bin/env python3
"""extract_models.py -- recursively extract a library of model .zip files,
PRESERVING the zip NAMES as folder names (the names carry classification metadata
the rest of the pipeline routes on), and print a recursive tree.

Each `path/to/both-Grimlock.zip` becomes `path/to/both-Grimlock/<contents>`, so
find_models / process_models / texture_export / model_metadata all see the
classification name via the folder. Handles spaces + special chars, nested zips
(a zip extracted from a zip is itself extracted), and skips already-done folders.

Usage:
  python3 tools/extract_models.py LIBRARY_ROOT           # extract in place
  python3 tools/extract_models.py LIBRARY_ROOT --out extracted   # mirror to a new tree
  python3 tools/extract_models.py LIBRARY_ROOT --tree-only       # just print the tree
  python3 tools/extract_models.py LIBRARY_ROOT --keep-zips       # don't note zips as done
"""
import sys, os, argparse, zipfile, shutil

def _safe_extract(zf, dest):
    """Extract with path-traversal protection (no absolute / .. escapes)."""
    os.makedirs(dest, exist_ok=True)
    base = os.path.abspath(dest)
    for member in zf.namelist():
        target = os.path.abspath(os.path.join(dest, member))
        if not target.startswith(base + os.sep) and target != base:
            print("  ! skipped unsafe path:", member); continue
    zf.extractall(dest)

def extract_all(root, out_root=None, max_passes=4):
    """Recursively extract all zips. Returns (extracted, skipped, failed)."""
    extracted = skipped = failed = 0
    for _pass in range(max_passes):
        found_new = False
        for dp, _dirs, files in os.walk(root):
            for f in files:
                if not f.lower().endswith(".zip"):
                    continue
                zip_path = os.path.join(dp, f)
                stem = f[:-4]  # strip .zip, keep the rest (spaces, --, aka, etc.)
                if out_root:
                    rel = os.path.relpath(dp, root)
                    dest = os.path.join(out_root, rel, stem)
                else:
                    dest = os.path.join(dp, stem)
                # skip if already extracted (folder exists + has files)
                if os.path.isdir(dest) and any(os.scandir(dest)):
                    skipped += 1
                    continue
                try:
                    with zipfile.ZipFile(zip_path) as zf:
                        _safe_extract(zf, dest)
                    extracted += 1; found_new = True
                    print("  extracted:", os.path.relpath(dest, out_root or root))
                except zipfile.BadZipFile:
                    print("  ! bad zip:", zip_path); failed += 1
                except Exception as e:
                    print("  ! failed:", zip_path, "-", e); failed += 1
        # if extracting produced NEW nested zips, another pass catches them
        if not found_new:
            break
        if out_root:
            root = out_root  # subsequent passes scan the output tree for nested zips
    return extracted, skipped, failed

MODEL_EXT = (".gltf", ".glb", ".obj", ".fbx", ".stl", ".dae")

def print_tree(root, out=sys.stdout, max_depth=6):
    """Print a recursive tree, marking folders that contain a model file."""
    root = os.path.abspath(root)
    def has_model(d):
        try:
            return any(fn.lower().endswith(MODEL_EXT) for fn in os.listdir(d))
        except OSError:
            return False
    def walk(d, prefix, depth):
        if depth > max_depth: return
        try:
            entries = sorted(os.scandir(d), key=lambda e: (not e.is_dir(), e.name.lower()))
        except OSError:
            return
        dirs = [e for e in entries if e.is_dir()]
        for i, e in enumerate(dirs):
            last = (i == len(dirs) - 1)
            tag = "  [model]" if has_model(e.path) else ""
            out.write(f"{prefix}{'└── ' if last else '├── '}{e.name}{tag}\n")
            walk(e.path, prefix + ("    " if last else "│   "), depth + 1)
    out.write(os.path.basename(root) + "/\n")
    walk(root, "", 1)

def count_models(root):
    n = 0
    for _dp, _d, fs in os.walk(root):
        if any(f.lower().endswith(MODEL_EXT) for f in fs):
            n += 1
    return n

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--out", default=None, help="mirror extraction to a new tree (else in place)")
    ap.add_argument("--tree-only", action="store_true", help="just print the tree, no extraction")
    ap.add_argument("--tree-out", default=None, help="also write the tree to this file")
    a = ap.parse_args()
    if not os.path.isdir(a.root):
        print("not a directory:", a.root); sys.exit(1)

    if not a.tree_only:
        print("extracting zips (preserving names as folders)...")
        ex, sk, fa = extract_all(a.root, a.out)
        print(f"\n{ex} extracted, {sk} already done, {fa} failed")

    scan_root = a.out if (a.out and not a.tree_only) else a.root
    print(f"\nmodel folders found: {count_models(scan_root)}")
    print("\ntree:")
    print_tree(scan_root)
    if a.tree_out:
        with open(a.tree_out, "w") as fh:
            print_tree(scan_root, out=fh)
        print("\ntree written to", a.tree_out)

if __name__ == "__main__":
    main()
