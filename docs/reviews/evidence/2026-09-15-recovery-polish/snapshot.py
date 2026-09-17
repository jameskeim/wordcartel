#!/usr/bin/env python3
"""Capture the complete changed source for independent review; never stage the live tree."""
import hashlib
import json
from pathlib import Path
import subprocess

out = Path(__file__).resolve().parent
root = out.parents[3]
def git(*args):
    return subprocess.check_output(['git', *args], cwd=root)
tracked = git('diff', '--name-only', 'HEAD', '--', 'Cargo.lock', 'wordcartel', 'scripts/smoke').decode().splitlines()
new = git('ls-files', '--others', '--exclude-standard', '--', 'wordcartel/src').decode().splitlines()
paths = sorted(set(tracked + new))
patch = git('diff', '--binary', 'HEAD', '--', *tracked)
for name in new:
    result = subprocess.run(['git', 'diff', '--binary', '--no-index', '--', '/dev/null', name],
                            cwd=root, stdout=subprocess.PIPE, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(f'diff failed: {name}')
    patch += result.stdout
(out / 'source.patch').write_bytes(patch)
hashes = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in paths}
(out / 'source-sha256.json').write_text(json.dumps(hashes, indent=2) + '\n')
print(f'Captured {len(paths)} source files, {len(patch)} patch bytes')
