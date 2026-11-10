#!/usr/bin/env python3
"""Measure Laya concurrency properly, then PROVE the redb cross-process behaviour.

Two questions the P1 architecture depends on:
  1. Does the Laya server serialise concurrent calls? (decides whether a priority
     queue in front of it has real value)
  2. Does redb refuse a second process? (blocker B4 — prove it, don't assume)
"""
import json
import os
import subprocess
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor

URL = "http://10.10.10.217:8083"
BODY = json.dumps({"content": ["is this reversible"]}).encode()


def one_call(_i: int) -> float:
    t0 = time.perf_counter()
    req = urllib.request.Request(
        f"{URL}/embedding", data=BODY, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    assert r.status == 200, r.status
    assert len(data) > 1000, f"suspiciously small: {len(data)}"
    return (time.perf_counter() - t0) * 1000


# ---- 1. sequential baseline -------------------------------------------------
seq = []
for i in range(3):
    seq.append(one_call(i))
print("1. SEQUENTIAL (3 calls)")
print("   " + "  ".join(f"{v:.0f}ms" for v in seq))
print(f"   median {sorted(seq)[1]:.0f} ms")
print()

# ---- 2. concurrent ----------------------------------------------------------
for n in (4, 8):
    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=n) as ex:
        times = list(ex.map(one_call, range(n)))
    wall = (time.perf_counter() - t0) * 1000
    print(f"2. CONCURRENT x{n}")
    print("   " + "  ".join(f"{v:.0f}ms" for v in times))
    print(f"   wall {wall:.0f} ms  |  sum-of-individual {sum(times):.0f} ms")
    ratio = sum(times) / wall if wall else 0
    verdict = (
        "SERIALISED (wall ~= sum)" if ratio < 1.5
        else f"PARALLELISED (wall ~= sum/{ratio:.1f})"
    )
    print(f"   -> {verdict}")
    print()

# ---- 3. the redb cross-process proof ----------------------------------------
print("3. REDB CROSS-PROCESS (blocker B4)")
print("   a tiny redb writer, run twice against the SAME file")
src = r'''
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use redb::{Database, TableDefinition};
    const T: TableDefinition<&str, &str> = TableDefinition::new("t");
    let path = std::env::args().nth(1).unwrap();
    let hold = std::env::args().nth(2).is_some();
    let db = Database::create(&path)?;
    let w = db.begin_write()?;
    { let mut t = w.open_table(T)?; t.insert("k", "v")?; }
    w.commit()?;
    println!("   writer: OPENED AND COMMITTED ok");
    if hold {
        std::thread::sleep(std::time::Duration::from_secs(6));
        println!("   writer: released");
    }
    Ok(())
}
'''
probe = "/projects/laya-rs/_tmp/redb-probe"
os.makedirs(probe, exist_ok=True)
with open(f"{probe}/Cargo.toml", "w") as f:
    f.write(
        '[package]\nname = "redb-probe"\nversion = "0.1.0"\nedition = "2021"\n'
        '[dependencies]\nredb = "4.2"\n'
    )
os.makedirs(f"{probe}/src", exist_ok=True)
with open(f"{probe}/src/main.rs", "w") as f:
    f.write(src)

env = dict(os.environ)
env["PATH"] = "/usr/local/cargo/bin:" + env.get("PATH", "")
env["CARGO_HOME"] = "/usr/local/cargo"
env["RUSTUP_HOME"] = "/usr/local/rustup"

print("   building the probe...")
b = subprocess.run(
    ["cargo", "build", "-q", "--release"], cwd=probe, env=env,
    capture_output=True, text=True, timeout=600,
)
if b.returncode != 0:
    print("   BUILD FAILED:", b.stderr[-600:])
else:
    binp = f"{probe}/target/release/redb-probe"
    dbp = "/tmp/laya-probe.redb"
    for suffix in ("", "-wal", "-lock"):
        try:
            os.remove(dbp + suffix)
        except FileNotFoundError:
            pass

    # writer A holds the DB open for 6s; writer B tries to open in that window
    a = subprocess.Popen([binp, dbp, "hold"], env=env,
                         stdout=subprocess.PIPE, text=True)
    time.sleep(1.5)
    bres = subprocess.run([binp, dbp], env=env, capture_output=True,
                          text=True, timeout=60)
    print(f"   A (first, holding): {'running' if a.poll() is None else 'exited'}")
    print(f"   B (second) exit    : {bres.returncode}")
    out = (bres.stdout + bres.stderr).strip().splitlines()
    for line in out[:4]:
        print(f"     {line}")
    verdict = (
        "REFUSED — B4 CONFIRMED: one writer at a time, single-process model"
        if bres.returncode != 0
        else "ADMITTED — B4 would be WRONG; concurrent writers are allowed"
    )
    print(f"   -> {verdict}")
    a.wait(timeout=30)
    print(f"   A final exit: {a.returncode} (0 = it committed before releasing)")
