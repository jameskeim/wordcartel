#!/usr/bin/env python3
"""Inventory and lexical reference index, not a Rust call graph or coverage metric."""
from pathlib import Path
import csv
import re

here = Path(__file__).resolve().parent
root = here.parents[3]
paths = sorted(p for base in ['wordcartel/src', 'wordcartel-core/src', 'wordcartel-nlp/src']
               for p in (root / base).rglob('*.rs'))
groups = {
    1: 'save swap recovery file fsx pathx editor workspace scratch state session_restore startup file_browser_commit',
    2: 'jobs jobs_apply timers app panicx term',
    3: 'transact edit_apply commands blocks_marked marks search_ui transform ventilate',
    4: 'derive reconcile fold nav lines compose block_paint',
    5: 'diag_provider diagnostics_run diag_overlay lsp_rpc lsp_client harper_ls ltex_ls clipboard filter export nlp lenses',
}
core_pass = {name: 3 for name in 'buffer change selection history textobj register search'.split()}
core_pass.update({name: 4 for name in 'block_tree md_parse outline layout count style'.split()})
core_pass.update(diagnostics=5, theme=6, lib=0, test_support=0, proptest_strategies=3)
scopes = {
    'fsx': 'Atomic replace, destination resolution, reads and fault seam',
    'file': 'Bounded open and atomic-save wrappers',
    'pathx': 'Tilde expansion and platform directories',
    'workspace': 'Open/new/close/switch and scratch routing',
    'scratch': 'Copy/move to scratch',
    'state': 'Load/save session and size caps',
    'session_restore': 'Resume, scratch restore, open replacement, persistence/migrations',
    'startup': 'Configuration seeding and resume enablement',
    'save': 'Fingerprint, save dispatch/merge, Save As, reload/recovery',
    'swap': 'Naming, cadence, dispatch, cleanup, assessment and orphan discovery',
    'editor': 'Buffer construction/identity/dirty state and lifecycle methods only',
    'app': 'Launch recovery, reduce timing, pre-render advance, shutdown/persistence only',
    'prompts': 'Recovery, Save As, close/quit, cleanup actions only',
    'timers': 'Swap deadlines/ticks and save timeout only',
    'jobs': 'FIFO executor and result-class routing',
    'jobs_apply': 'Save completion and quit-drain actions only',
    'file_browser_commit': 'Destination classification and save routing only',
}
texts = {}
for p in paths:
    # Strip trailing ordinary test module, then comments, for a conservative reference index.
    text = re.split(r'#\[cfg\(test\)\]\s*mod tests\s*\{', p.read_text())[0]
    texts[p] = '\n'.join(line for line in text.splitlines() if not line.lstrip().startswith('//'))
rows = []
for p in paths:
    rel = p.relative_to(root)
    crate = rel.parts[0]
    stem = p.stem
    if crate == 'wordcartel-core':
        review_pass = core_pass.get(stem, 4)
    elif crate == 'wordcartel-nlp':
        review_pass = 5
    elif 'plugin' in rel.parts:
        review_pass = 5
    elif 'commands' in rel.parts:
        review_pass = 3
    elif stem in ['lib', 'main', 'e2e', 'test_support', 'limits']:
        review_pass = 0
    else:
        review_pass = next((n for n, names in groups.items() if stem in names.split()), 6)
    module = '::'.join(rel.with_suffix('').parts[2:])
    if module.endswith('::mod'): module = module[:-5]
    references = []
    for caller, text in texts.items():
        if caller == p: continue
        prefix = 'crate' if caller.relative_to(root).parts[0] == crate else crate.replace('-', '_')
        if re.search(r'\b' + re.escape(prefix + '::' + module) + r'\b', text):
            references.append(str(caller.relative_to(root)))
    scope = scopes.get(stem) if crate == 'wordcartel' and len(rel.parts) == 3 else None
    exercised = scope and stem in ['save', 'swap', 'fsx', 'file', 'workspace', 'editor',
                                  'app', 'prompts', 'timers', 'jobs', 'jobs_apply']
    rows.append([str(rel), review_pass, 'exercised (scope-limited)' if exercised else
                 ('inspected (scope-limited)' if scope else 'mapped only'),
                 scope or 'Deferred to assigned pass; no semantic review claimed', '; '.join(references)])
with (here / 'module-coverage.csv').open('w', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(['module', 'primary_pass', 'depth', 'scope_or_gap', 'lexical_reference_candidates'])
    writer.writerows(rows)
print(f'Indexed {len(rows)} modules; reference candidates omit aliases, reexports and dynamic dispatch.')
