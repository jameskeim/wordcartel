#!/usr/bin/env python3
"""Launch Fable in the prepared isolated checkout, retaining report and event log."""
import json
import os
from pathlib import Path
import shutil
import subprocess

out = Path(__file__).resolve().parent
manifest_path = out / 'fable-run.json'
manifest = json.loads(manifest_path.read_text())
work, checkout = Path(manifest['work']), Path(manifest['checkout'])
env = os.environ.copy()
for key, directory in [('XDG_STATE_HOME', 'state'), ('XDG_CONFIG_HOME', 'config'),
                       ('XDG_CACHE_HOME', 'cache'), ('CARGO_TARGET_DIR', 'target')]:
    env[key] = str(work / directory)
allowed = ['Read', 'Grep', 'Glob', 'Edit', 'Write'] + [f'Bash({cmd} *)' for cmd in
    ['cargo test', 'cargo check', 'cargo build', 'cargo clippy', 'git diff', 'git status', 'git show',
     'git rev-parse', 'rg', 'sed', 'ls', 'mkdir', 'python3', 'rustc', 'cat', 'sha256sum']]
allowed.append('Bash(cargo --version)')
cmd = ['claude', '-p', '--model', 'fable', '--output-format', 'stream-json', '--verbose',
       '--no-session-persistence', '--permission-mode', 'dontAsk',
       '--tools', 'Read,Grep,Glob,Bash,Write,Edit', '--allowedTools', ','.join(allowed)]
manifest['command'] = cmd
manifest['status'] = 'starting'
manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
with (out / 'fable-events.jsonl').open('w') as stdout, (out / 'fable-stderr.log').open('w') as stderr:
    child = subprocess.Popen(cmd, cwd=checkout, env=env, stdin=subprocess.PIPE,
                             stdout=stdout, stderr=stderr, text=True)
    manifest.update(status='running', pid=child.pid)
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    child.communicate('Read REVIEW_BRIEF.md and conduct the requested independent implementation review.\n')
manifest.update(status='finished', exit_code=child.returncode)
manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
report = checkout / 'FABLE_REVIEW.md'
if report.exists(): shutil.copyfile(report, out / 'fable-review.md')
probes = checkout / 'review-probes'
if probes.exists(): shutil.copytree(probes, out / 'fable-probes', dirs_exist_ok=True)
for line in (out / 'fable-events.jsonl').read_text().splitlines():
    try: event = json.loads(line)
    except json.JSONDecodeError: continue
    if event.get('type') == 'result':
        (out / 'fable-result.json').write_text(json.dumps(event, indent=2) + '\n')
print(json.dumps({'exit_code': child.returncode, 'report_saved': report.exists(),
                  'events': str(out / 'fable-events.jsonl')}))
raise SystemExit(child.returncode)
