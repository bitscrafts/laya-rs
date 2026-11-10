#!/usr/bin/env bash
# PROVE the current binary + sidecar work end to end, on the DEFAULT socket,
# using the EXACT command line Hermes uses. This isolates one question:
# "is the failure my session's stale process, or the wiring?"
set -uo pipefail
MCP=/root/.local/bin/laya-mcp
SOCKET=/run/laya/sidecar.sock

echo "=== preconditions ==="
printf '  binary      : %s (%s)\n' "$(stat -c '%y' $MCP | cut -d. -f1)" "$(basename $MCP)"
printf '  sidecar sock: %s\n' "$(ls -la $SOCKET 2>/dev/null | awk '{print $1}')"
printf '  prefix check: %s occurrences of "laya-mcp:" in the binary\n' "$(strings $MCP | grep -c '^laya-mcp:' || echo 0)"

echo
echo "=== the exact Hermes command line, driven with a real MCP session ==="
python3 - <<'PY'
import json, subprocess, sys, time
MCP = "/root/.local/bin/laya-mcp"

p = subprocess.Popen([MCP], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.PIPE, text=True,
                     env={**__import__("os").environ, "HOME": "/root"})
def rpc(o):
    p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()
def rd():
    return json.loads(p.stdout.readline())

rpc({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
    "protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"hermes-check","version":"1"}}})
init = rd()
print("  initialize:", init.get("result",{}).get("serverInfo"))
rpc({"jsonrpc":"2.0","method":"notifications/initialized"})

t0 = time.time()
rpc({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
    "name":"gate","arguments":{"action":"rm -rf node_modules && npm install"}}})
r = rd()
wall = (time.time()-t0)*1000
if "error" in r:
    print(f"  gate -> ERROR: {r['error'].get('message','')[:200]}")
    sys.exit(1)
g = json.loads(r["result"]["content"][0]["text"])
print(f"  gate -> decision={g.get('decision')!r} band={g.get('band')!r} "
      f"confidence={g.get('confidence')} ({wall:.0f} ms)")

# the reference design's exact case: irreversible at 0.35 confidence must NOT auto-allow
rpc({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
    "name":"gate","arguments":{"action":"rm -rf /var/lib/postgresql/data",
                               "irreversible":True}}})
r = rd()
if "error" not in r:
    g2 = json.loads(r["result"]["content"][0]["text"])
    print(f"  gate(irreversible) -> decision={g2.get('decision')!r} "
          f"band={g2.get('band')!r} confidence={g2.get('confidence')}")

p.stdin.close(); p.wait(timeout=20)
print(f"  clean exit rc={p.returncode}")
PY
