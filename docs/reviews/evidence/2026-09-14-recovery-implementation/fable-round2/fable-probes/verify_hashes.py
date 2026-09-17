#!/usr/bin/env python3
"""Re-hash every file in the snapshot manifest against this checkout."""
import hashlib, json, os, sys

E = "docs/reviews/evidence/2026-09-14-recovery-implementation"
manifest = json.load(open(os.path.join(E, "source-sha256.json")))
if isinstance(manifest, dict) and "files" in manifest:
    manifest = manifest["files"]
if isinstance(manifest, dict):
    items = list(manifest.items())
else:
    items = [(x.get("path"), x.get("sha256")) for x in manifest]
bad = 0
for path, want in items:
    if isinstance(want, dict):
        want = want.get("sha256")
    if not os.path.exists(path):
        print("MISSING", path); bad += 1; continue
    got = hashlib.sha256(open(path, "rb").read()).hexdigest()
    if got != want:
        print("MISMATCH", path, "manifest", want[:12], "checkout", got[:12]); bad += 1
print(f"entries={len(items)} mismatches={bad}")
sys.exit(1 if bad else 0)
