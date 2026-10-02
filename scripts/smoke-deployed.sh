#!/usr/bin/env bash
# Smoke-test the DEPLOYED binaries: does the live path now have every fix?
set -uo pipefail
cd /projects/laya-rs
export PATH=/root/.local/bin:$PATH

echo "################ 1. spec 20 via the DEPLOYED binary: a typo is refused ################"
python3 -c "
import json; open('/tmp/typo.json','w').write(json.dumps({'flow':'t','version':1,'nodes':[{'id':'a','kind':'shell','run':'echo a','depend_on':['b']},{'id':'b','kind':'shell','run':'echo b'}]}))"
echo "  $(laya graph validate /tmp/typo.json 2>&1 | head -1 | cut -c1-110)"

echo
echo "################ 2. spec 18 via the DEPLOYED binary: a wc drives the branch ################"
F=_docs/02-guides/wc-classify-measured.flow.json
for doc in short-note long-note; do
  printf "  %-11s " "$doc"
  WC_FILE=/projects/_tmp/wc-example/$doc.txt laya graph run "$F" 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        d=json.loads(l)
        br=[r['node_id'] for r in d['records'] if r['node_id'].startswith('class_')]
        print('branch:', br[0] if br else 'none')
        break
    elif l and 'error' in l.lower(): print(l[:80])
"
done

echo
echo "################ 3. spec 16 via the DEPLOYED binary: a flow can ask a noul ################"
echo "  $(laya graph validate /tmp/noul.flow.json 2>&1 | head -1)"
laya graph run /tmp/noul.flow.json 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        d=json.loads(l)
        for r in d['records']:
            if r['node_id']=='d':
                o=r.get('output') or ''
                import re
                m=re.search(r'\"band\":\"(\w+)\"', str(o))
                print('    noul band via the deployed binary:', m.group(1) if m else '?')
        break
"

echo
echo "################ 4. the gate still answers ################"
laya-gate "git status" >/dev/null 2>&1; echo "  git status        -> exit $?"
laya-gate "rm -rf /"   >/dev/null 2>&1; echo "  rm -rf /          -> exit $?  (1 = ask, correct)"

echo
echo "################ 5. the head is still up ################"
laya doctor 2>&1 | head -4 | sed 's/^/  /'
