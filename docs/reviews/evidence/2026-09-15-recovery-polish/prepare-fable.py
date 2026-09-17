#!/usr/bin/env python3
"""Prepare an isolated snapshot for the explicitly requested Fable review."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

out = Path(__file__).resolve().parent
root = out.parents[3]
hashes = json.loads((out / 'source-sha256.json').read_text())
for name, expected in hashes.items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
base = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
assert base == '9e409c0f09d9683c40e84dbd2499c9f9e6e32ded'
work = Path(tempfile.mkdtemp(prefix='wordcartel-fable-'))
checkout = work / 'checkout'
with (out / 'fable-prepare.log').open('w') as log:
    subprocess.run(['git', 'clone', '--shared', '--no-hardlinks', str(root), str(checkout)],
                   stdout=log, stderr=subprocess.STDOUT, check=True)
    subprocess.run(['git', 'apply', str(out / 'source.patch')], cwd=checkout,
                   stdout=log, stderr=subprocess.STDOUT, check=True)
for name, expected in hashes.items():
    assert hashlib.sha256((checkout / name).read_bytes()).hexdigest() == expected, name
new_sources = subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard',
    '--', 'wordcartel/src'], cwd=checkout, text=True).splitlines()
if new_sources:
    subprocess.run(['git', 'add', '-N', '--', *new_sources], cwd=checkout, check=True)
# Preserve Wordcartel's relative local dependency without pointing compilation at
# another live working tree. repar is a standalone package, not a workspace member.
dependency = root.parent / 'par-command/repar'
dependency_copy = work / 'par-command/repar'
dependency_copy.mkdir(parents=True)
for name in ['src', 'Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE', 'build.rs']:
    source = dependency / name
    if source.is_dir(): shutil.copytree(source, dependency_copy / name)
    elif source.is_file(): shutil.copyfile(source, dependency_copy / name)
for pattern in ['docs/superpowers/specs/2026-09-14-recovery*.md',
                'docs/superpowers/plans/2026-09-14-recovery*.md',
                'docs/reviews/2026-09-14-recovery*.md',
                'docs/reviews/2026-09-15-recovery*.md']:
    for source in root.glob(pattern):
        target = checkout / source.relative_to(root)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
target = checkout / out.relative_to(root)
target.mkdir(parents=True, exist_ok=True)
for source in out.iterdir():
    if source.is_file(): shutil.copyfile(source, target / source.name)
    elif source.name in {'fable-round1', 'fable-probes', 'previous-final-review'}:
        shutil.copytree(source, target / source.name, dirs_exist_ok=True)
shutil.copyfile(out / 'fable-brief.md', checkout / 'REVIEW_BRIEF.md')
for name in ['state', 'config', 'cache', 'target']:
    (work / name).mkdir()
manifest = {'base': base, 'work': str(work), 'checkout': str(checkout),
            'source_hashes': hashes, 'model': 'fable'}
(out / 'fable-run.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'checkout': str(checkout), 'verified_source_files': len(hashes)}))
