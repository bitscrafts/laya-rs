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
echo "################ 3b. spec 21 via the DEPLOYED binary: decide timeout + allow_failure ################"
python3 - <<'PY'
import json
def flow(extra=None):
    d = {"flow":"dt","version":1,"nodes":[
      {"id":"d","kind":"decide","question":"Is the sky blue?","prompt":"the sky is blue",
       "options":["yes","no"],"ports":{"yes":"y","no":"n","_":"n"}},
      {"id":"y","kind":"shell","run":"echo y"},
      {"id":"n","kind":"shell","run":"echo n"},
      {"id":"sibling","kind":"shell","run":"echo sibling"}]}
    if extra: d["nodes"][0].update(extra)
    return d
open('/tmp/smoke-dt.flow.json','w').write(json.dumps(flow({"timeout": 0})))
open('/tmp/smoke-af.flow.json','w').write(json.dumps(flow({"prompt":"","allow_failure":True})))
open('/tmp/smoke-dt-none.flow.json','w').write(json.dumps(flow()))
PY
echo "  the timeout refusal must be LIFTED: $(laya graph validate /tmp/smoke-dt.flow.json 2>&1 | head -1 | cut -c1-60)"
printf "  decide with timeout: 0  -> "
laya graph run /tmp/smoke-dt.flow.json 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        d=json.loads(l)
        errs=[str(r.get('error')) for r in d['records'] if r['node_id']=='d']
        print('timed out =', bool(errs and 'timed out' in (errs[0] or '')), '| stopped_early =', d['stopped_early'])
        break
"
printf "  CONTROL: same decide, no timeout -> "
laya graph run /tmp/smoke-dt-none.flow.json 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        d=json.loads(l)
        print('completes =', not d['stopped_early'])
        break
"
printf "  allow_failure on a decide -> the run must NOT stop: "
laya graph run /tmp/smoke-af.flow.json 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        d=json.loads(l)
        print('stopped_early =', d['stopped_early'], '| sibling ran =', 'sibling' in [r['node_id'] for r in d['records']])
        break
"

echo
echo "################ 3c. spec 22 via the DEPLOYED binary: finds its own sibling ################"
echo "  the installed binary, invoked by ABSOLUTE path, with a STRIPPED PATH for its children"
echo "  (this is the only way the defect appears -- the shell resolves the parent, not the child):"
env PATH=/usr/bin:/bin /root/.local/bin/laya graph run _docs/02-guides/wc-classify-measured.flow.json 2>/dev/null | python3 -c "
import sys, json
d = json.loads(next(l for l in sys.stdin if l.startswith('{')))
bad = [r['node_id'] for r in d['records'] if not r['success']]
print('    stripped PATH ->', ('FAILED: ' + str(bad)) if bad else 'ALL NODES SUCCEEDED')
"
echo "  and which server it resolved:"
/root/.local/bin/laya doctor 2>&1 | grep -i 'laya-mcp' | sed 's/^/    /'

echo
echo "################ 3d. spec 25 through the INSTALLED binary: decide from an exit status ################"
cat > /tmp/smoke-check.flow.json <<'J'
{"flow":"smoke-check","version":1,"nodes":[
 {"id":"count","kind":"check","run":"[ \"$(grep -h -c '^- \\[ \\]' specs/*.md | awk '{s+=$1} END{print s+0}')\" = \"0\" ]","ports":{"pass":"release","fail":"work_left","_":"esc"}},
 {"id":"release","kind":"shell","run":"echo 'RELEASED: nothing unfinished'"},
 {"id":"work_left","kind":"shell","run":"echo 'BLOCKED: criteria still open'"},
 {"id":"esc","kind":"shell","run":"echo 'ESCALATED'"}]}
J
/root/.local/bin/laya graph run /tmp/smoke-check.flow.json 2>/dev/null | python3 -c "
import sys, json
d = json.loads(next(l for l in sys.stdin if l.startswith('{')))
c = [r for r in d['records'] if r['node_id'] == 'count'][0]
print('    check exit_code=%s (non-zero is an ANSWER, not a failure: success=%s)' % (c.get('exit_code'), c['success']))
print('    took:', [r['node_id'] for r in d['records'] if r['node_id'] != 'count'])
print('    stopped_early:', d.get('stopped_early'))
"

echo
echo "################ 4. the gate still answers ################"
laya-gate "git status" >/dev/null 2>&1; echo "  git status        -> exit $?"
laya-gate "rm -rf /"   >/dev/null 2>&1; echo "  rm -rf /          -> exit $?  (1 = ask, correct)"

echo
echo "################ 5. the head is still up ################"
laya doctor 2>&1 | head -4 | sed 's/^/  /'
