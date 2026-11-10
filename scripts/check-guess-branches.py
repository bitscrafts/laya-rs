#!/usr/bin/env python3
"""The control for spec 24 R3: the two guess flows must reach DIFFERENT advice nodes.

Runs both flows through the CLI and compares which advice branch each one took. A control that
cannot distinguish the two is not a control, so this exits non-zero if the branches are the same,
or if either flow did not reach one.

The binary is `target/debug/laya` under the repository root, or `$LAYA_CLI_BIN`.
"""
import json
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("LAYA_CLI_BIN", os.path.join(REPO, "target", "debug", "laya"))
ADVICE = {'advise_lower', 'advise_higher'}


def run(rel):
    try:
        p = subprocess.run([BIN, 'graph', 'run', rel], cwd=REPO, capture_output=True, text=True)
    except OSError as e:
        return None, f'cannot run {BIN}: {e}'
    if p.returncode != 0:
        return None, (p.stdout + p.stderr).strip()[-300:]
    try:
        return json.loads(next(l for l in p.stdout.splitlines() if l.startswith('{'))), None
    except StopIteration:
        return None, 'no JSON report on stdout'


def reached(rep):
    return sorted(r['node_id'] for r in rep['records'] if r['node_id'] in ADVICE)


main, e1 = run('_docs/02-guides/guess-the-number.flow.json')
ctrl, e2 = run('_docs/02-guides/guess-the-number.control.flow.json')

for label, err in (('flow', e1), ('control', e2)):
    if err:
        print(f'{label}: ERROR -- {err}')
        sys.exit(1)

a, b = reached(main), reached(ctrl)
print('flow    reached:', a, ' skipped:', main.get('skipped'))
print('control reached:', b, ' skipped:', ctrl.get('skipped'))

if not a or not b:
    print('one of the flows reached no advice node at all -- nothing to compare')
    sys.exit(1)
if a != b:
    print('BRANCHES DIFFER')
    sys.exit(0)
print('BRANCHES IDENTICAL -- the control does not control anything')
sys.exit(1)
