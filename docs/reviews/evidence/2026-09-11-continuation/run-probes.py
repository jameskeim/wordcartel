#!/usr/bin/env python3
"""Temporarily append review-only unit probes, run them, restore exact source bytes.

Run from any directory: python3 path/to/run-probes.py [log-name]
The probes assert observed defects as well as controls; PASS does not mean fixed.
"""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile

evidence = Path(__file__).resolve().parent
root = evidence.parents[3]
source = root / 'wordcartel/src/swap.rs'
original = source.read_bytes()
probe = (evidence / 'probes.rs').read_bytes()
assert b'mod review_probes {' not in original, 'probes already installed; inspect before running'
log = evidence / (sys.argv[1] if len(sys.argv) > 1 else 'probes.log')
cmd = ['cargo', 'test', '-p', 'wordcartel', '--lib', 'review_probes', '--', '--test-threads=1']
with tempfile.TemporaryDirectory(prefix='wordcartel-review-') as isolated:
    env = os.environ.copy()
    for key, directory in [('XDG_STATE_HOME', 'state'), ('XDG_CONFIG_HOME', 'config'),
                           ('XDG_CACHE_HOME', 'cache')]:
        path = Path(isolated) / directory
        path.mkdir()
        env[key] = str(path)
    try:
        source.write_bytes(original + b'\n' + probe)
        with log.open('w') as out:
            out.write('Command: ' + ' '.join(cmd) + '\n')
            out.write('Source SHA256: ' + hashlib.sha256(original).hexdigest() + '\n')
            out.flush()
            result = subprocess.run(cmd, cwd=root, env=env, stdout=out, stderr=subprocess.STDOUT)
    finally:
        assert source.read_bytes() == original + b'\n' + probe, 'concurrent edit: refusing to overwrite'
        source.write_bytes(original)
assert source.read_bytes() == original
print(f'Probes exit={result.returncode}; source restored; log={log.relative_to(root)}')
sys.exit(result.returncode)
