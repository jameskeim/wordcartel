#!/usr/bin/env python3
"""Re-hash every file in the two evidence manifests against this checkout."""
import hashlib, json, subprocess
from pathlib import Path
root = Path('/tmp/wordcartel-fable-us9b2pys/checkout')
ev = root / 'docs/reviews/evidence/2026-09-15-recovery-polish'
new = json.loads((ev / 'source-sha256.json').read_text())
old = json.loads((ev / 'previous-final-review/source-sha256.json').read_text())
def h(p): return hashlib.sha256((root / p).read_bytes()).hexdigest()
mism = [p for p, v in new.items() if h(p) != v]
print(f'new manifest: {len(new)} files, {len(mism)} mismatches', mism)
diff = sorted(p for p in set(new) | set(old) if new.get(p) != old.get(p))
print(f'files differing old->new: {len(diff)}')
for p in diff: print('  ', p)
manifest_sha = hashlib.sha256((ev / 'source-sha256.json').read_bytes()).hexdigest()
val = json.loads((ev / 'validation.json').read_text())
print('validation.source_manifest_sha256 match:', manifest_sha == val['source_manifest_sha256'])
for g in val['gates']:
    ok = hashlib.sha256((ev / g['log']).read_bytes()).hexdigest() == g['log_sha256']
    print(f"  {g['command']}: log sha match={ok} exit={g['exit_code']}")
tracked = subprocess.check_output(['git','diff','--name-only','HEAD','--','Cargo.lock','wordcartel','scripts/smoke'],cwd=root).decode().split()
untracked = subprocess.check_output(['git','ls-files','--others','--exclude-standard','--','wordcartel/src'],cwd=root).decode().split()
live = set(tracked) | set(untracked)
print('live changed set == manifest set:', live == set(new),
      'extra live:', sorted(live - set(new)), 'missing live:', sorted(set(new) - live))
