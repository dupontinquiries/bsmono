#!/usr/bin/env python3
"""Diagnose the manifest vs. valid-tracks discrepancy against an EXISTING cache.
Read-only: does not touch or rebuild anything. Run from the pipeline/ directory:

    python3 diagnose_cache.py --cache cache
"""
import argparse, json, os, sqlite3, glob

p = argparse.ArgumentParser()
p.add_argument("--cache", default="cache")
p.add_argument("--shard-size", type=int, default=250)
args = p.parse_args()

manifest = json.load(open(os.path.join(args.cache, "manifest.json")))
print(f"manifest.json: {len(manifest)} entries")

shard_files = sorted(glob.glob(os.path.join(args.cache, "db-*.sqlite")))
print(f"shard files on disk: {len(shard_files)}")

total_rows = 0
total_matched = 0
per_shard = []
for shard_id, path in enumerate(shard_files):
    start = shard_id * args.shard_size
    end = start + args.shard_size
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    cur = conn.cursor()
    cur.execute("SELECT COUNT(*) FROM mels")
    row_count = cur.fetchone()[0]
    cur.execute("SELECT filepath FROM mels")
    db_files = set(r[0] for r in cur.fetchall())
    conn.close()

    expected_slice = manifest[start:min(end, len(manifest))]
    matched = sum(1 for f in expected_slice if f in db_files)
    total_rows += row_count
    total_matched += matched
    per_shard.append((os.path.basename(path), row_count, len(expected_slice), matched))

print(f"\n{'shard':<28}{'rows in db':<12}{'manifest slice':<16}{'matched':<10}")
for name, rows, slice_len, matched in per_shard:
    flag = "" if rows == matched else "  <-- MISMATCH"
    print(f"{name:<28}{rows:<12}{slice_len:<16}{matched:<10}{flag}")

print(f"\nTOTAL rows across all shard DBs:  {total_rows}")
print(f"TOTAL matched against manifest:   {total_matched}")
print(f"(this second number is what _setup_valid_indices should produce)")

if total_rows != total_matched:
    print("\n=> Rows exist in the DBs that do NOT match the manifest's expected")
    print("   filepath at that index/shard. Likely cause: filepath strings differ")
    print("   between what was inserted and what's in manifest.json (e.g. relative")
    print("   vs absolute paths, or the file's manifest index shifted after the")
    print("   insert). Sample mismatches below:")
    shown = 0
    for shard_id, path in enumerate(shard_files):
        if shown >= 5:
            break
        start = shard_id * args.shard_size
        end = start + args.shard_size
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        cur = conn.cursor()
        cur.execute("SELECT filepath FROM mels LIMIT 3")
        sample_db = [r[0] for r in cur.fetchall()]
        conn.close()
        sample_manifest = manifest[start:start + 3]
        if sample_db != sample_manifest:
            print(f"\n  shard {os.path.basename(path)}:")
            print(f"    manifest[{start}:{start+3}] = {sample_manifest}")
            print(f"    db first 3 filepaths        = {sample_db}")
            shown += 1
else:
    print("\n=> Rows match the manifest exactly. If training still reports far fewer")
    print("   valid tracks than this, the bug is in _setup_valid_indices' shard_id")
    print("   enumeration itself, not the data -- rerun with the exact same")
    print("   --shard-size value your Config uses (default 250) to be sure.")
