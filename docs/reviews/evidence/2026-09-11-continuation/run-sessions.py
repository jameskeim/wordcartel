#!/usr/bin/env python3
"""Reproduce two LIVE normal-library processes overwriting a named swap."""
from pathlib import Path
import os
import selectors
import subprocess
import tempfile

here = Path(__file__).resolve().parent
root = here.parents[3]
examples = root / 'wordcartel/examples'
examples.mkdir(exist_ok=True)
source = examples / 'review_swap_session.rs'
assert not source.exists()
payload = (here / 'session-probe.rs').read_bytes()
processes = []
try:
    source.write_bytes(payload)
    with (here / 'sessions.log').open('w') as log:
        cmd = ['cargo', 'build', '-p', 'wordcartel', '--example', 'review_swap_session']
        subprocess.run(cmd, cwd=root, stdout=log, stderr=subprocess.STDOUT, check=True)
        with tempfile.TemporaryDirectory(prefix='wordcartel-sessions-') as tmp:
            tmp = Path(tmp)
            env = os.environ.copy()
            for key, suffix in [('XDG_STATE_HOME', 'state'), ('XDG_CONFIG_HOME', 'config'),
                                ('XDG_CACHE_HOME', 'cache')]:
                (tmp / suffix).mkdir()
                env[key] = str(tmp / suffix)
            doc = tmp / 'shared.md'
            doc.write_text('on disk')
            carriers = []
            for body in ['session A unsaved', 'session B unsaved']:
                child = subprocess.Popen([str(root / 'target/debug/examples/review_swap_session'),
                    str(doc), body], env=env, text=True, stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE, stderr=log)
                processes.append(child)
                with selectors.DefaultSelector() as sel:
                    sel.register(child.stdout, selectors.EVENT_READ)
                    assert sel.select(timeout=10), 'helper did not produce its checkpoint'
                carriers.append(Path(child.stdout.readline().strip()))
                assert carriers[-1].read_text().endswith(body)
            assert all(child.poll() is None for child in processes), 'both sessions must be live'
            assert processes[0].pid != processes[1].pid
            assert carriers[0] == carriers[1], 'expected the observed shared-path defect'
            assert carriers[0].read_text().endswith('session B unsaved')
            assert 'session A unsaved' not in carriers[0].read_text()
            log.write('\nCONFIRMED: two distinct live processes used the same named swap; B replaced A.\n')
            for child in processes:
                child.stdin.write('done\n')
                child.stdin.flush()
                child.wait(timeout=10)
            log.flush()
finally:
    for child in processes:
        if child.poll() is None:
            child.kill()
            child.wait()
    assert source.read_bytes() == payload
    source.unlink()
    if not any(examples.iterdir()): examples.rmdir()
print('Cross-process probe complete; temporary source removed; see sessions.log')
