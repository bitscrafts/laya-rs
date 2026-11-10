# Supervising the decision head

How the Laya sidecar stays up across crashes and reboots, and why it is supervised
**on the host** rather than inside this container.

Unit installed at: `~/.config/systemd/user/laya-sidecar.service` on the host.
This is the doc referenced by the unit's `Documentation=` field.

---

## What was measured before choosing this design

Four facts, each verified, each ruling out an option that looks reasonable on paper.

| Fact | Command that showed it | Consequence |
|---|---|---|
| No init system in this container | `/proc/1/comm` → `podman-init`; `systemctl` **not found** | nothing inside this container can supervise a daemon |
| No container runtime inside | `podman` **absent** | cannot manage containers from here |
| No cron inside | `crontab` **absent**; no user crontab | cannot schedule a restart from here |
| `/run` is container-local tmpfs | `findmnt /run` → not a separate mount | the socket does **not** survive a container restart |
| The venv cannot run on the host | host exec of `.venv-laya/bin/python` → **Permission denied** (it is a symlink into the container's `~/.local/share/uv/...`) | the head can **only** run inside the container |
| The host can attach to the container | `podman exec hermes /bin/bash -c 'echo EXEC-OK'` → `EXEC-OK` | supervision goes through `podman exec` |
| The host runs user services at boot | `loginctl show-user mvcorrea --property=Linger` → `Linger=yes` | a user unit is enough; no root unit needed |

Host services that already exist and follow this pattern: `llamacpp-laya.service`,
the host's own units.

---

## The unit

`~/.config/systemd/user/laya-sidecar.service` on the host:

```ini
[Unit]
Description=laya-sidecar — the Laya decision head (inside the `hermes` container)
Documentation=https://github.com/bitscrafts/laya-rs/blob/master/_docs/05-operations/SUPERVISION.md
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/podman exec hermes ~/.local/bin/laya-sidecar
Restart=always
RestartSec=5
TimeoutStartSec=180
TimeoutStopSec=45

[Install]
WantedBy=default.target
```

Each line is load-bearing:

* **`podman exec hermes ...`** — attach to the *running* container. The interpreter
  exists nowhere else, so this is not a preference.
* **`Restart=always`** — the entire point. The failure being fixed is *a dead head
  nobody noticed*, which also went undetected because `doctor` did not check the head.
* **`RestartSec=5`** — fast enough to matter, slow enough not to hot-loop.
* **`TimeoutStartSec=180`** — the head loads in ~24–29 s. A shorter timeout makes
  systemd declare success a failure and kill it mid-load, which looks like a crash loop.
* **`TimeoutStopSec=45`** — give torch time to release cleanly rather than SIGKILL.

### Two approaches that do NOT work (recorded so nobody retries them)

* **A `.container` quadlet.** Quadlets *start* a container from an image; they cannot
  attach to an existing one. Attempting it produced a unit that would not enable:
  `Unit /run/user/1000/systemd/generator/laya-sidecar.service is transient or generated`.
  Use a plain `.service` when you need `enable` + `Restart=`.
* **Starting the sidecar from the image's entrypoint** (`/usr/local/bin/entrypoint.sh`).
  That file belongs to the image, so an image update reverts it. A host unit lives
  outside the image and is upgrade-proof.

---

## Install

```bash
# from inside the container, with the script in the repo:
scp scripts/install-host-service2.sh user@host:/tmp/
ssh user@host 'bash /tmp/install-laya-sidecar2.sh'
```

Or by hand on the host:

```bash
systemctl --user daemon-reload
systemctl --user enable --now laya-sidecar.service
```

Expect **~30 s** before the head is usable — that is the load, not a fault.

---

## Operate

```bash
# is it up, and is the head actually usable?  (the whole path, not just the unit)
ssh user@host 'podman exec hermes ~/.local/bin/laya doctor'

systemctl --user status  laya-sidecar.service        # unit state
systemctl --user restart laya-sidecar.service        # restart (waits ~30 s)
systemctl --user stop    laya-sidecar.service
journalctl --user -u laya-sidecar.service -n 40      # why did it exit
```

From **inside** the container, the same check without ssh:

```bash
laya doctor
```

---

## Verified: it actually recovers

The unit existing is not evidence that supervision works. This was tested by killing
the head and watching:

```
killed:      podman exec hermes pkill -f 'scripts/laya-sidecar.py'
observed:    RESTORED automatically after ~30 s (load ~24 s + restart secs)
result:      `laya doctor` -> READY, with no human action
```

`Restart=always` did the work. The near-miss that must **not** be mistaken for success:
`systemctl status` showing `active (running)` is not proof the *head* works — the unit
can be healthy while the sidecar inside it is failing to load. Poll `laya doctor`, which
checks the head's tensor count, not the unit's state.

---

## Boot order

`laya-sidecar.service` has `Wants=network-online.target` but does **not** `Requires=`
the `hermes` container. Reason: the sidecar is useless without the container, but a
`Requires=` on a user container unit makes boot order brittle and can leave the service
in a failed state that `Restart=always` will not clear, because the dependency itself
failed.

`Restart=always` resolves the ordering on its own: if the container is not up yet,
`podman exec` fails, and systemd retries every 5 s until it is. That is a *converging*
loop by construction, and it is why the unit has no container dependency.

**If a container restart ever leaves the head down**, check the socket first: `/run` is
tmpfs *inside* the container, so a container restart empties `/run/laya` and the unit
must bind a fresh socket. `Restart=always` handles this on the container's next stop,
but a container that is merely *recreated* (image update) may need
`systemctl --user restart laya-sidecar`.

---

## Failure modes and what they look like

| State | `laya doctor` says | Meaning |
|---|---|---|
| All good | `READY`, `head : ok (sidecar at ...)` | nothing to do |
| Head down, backbone up | `NOT READY` · `the backbone is UP but the DECISION HEAD is not usable` | restart the sidecar — **do not** restart the backbone |
| Backbone down | `connection failed` + `podman start llamacpp-laya` | the head may be fine; the backbone is not |
| Both down | backbone failure reported first | fix the backbone, then re-check |
| Unit healthy, head broken | `NOT READY` with a tensor-count reason | the unit is not the signal; `doctor` is |

---

## What is still NOT supervised (honest gaps)

* **The backbone** is supervised on the host (`llamacpp-laya.service`) — done separately.
* **The MCP server itself** needs no supervision: `laya-mcp` is spawned per client
  session and exits when its stdin closes. Nothing to restart.
* **Nothing restarts the client.** If an agent session holds a stale `laya-mcp` binary
  from before a rebuild, it keeps answering with the old behaviour until the session
  restarts. MCP clients do not hot-reload; seen live during development when a tool
  answered `laya-rs: ... not implemented` long after `laya-rs` had been renamed.
