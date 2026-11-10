#!/usr/bin/env bash
# Correct supervision: a plain systemd USER SERVICE that `podman exec`s into the
# container. The previous attempt used a `.container` quadlet, which was wrong:
# quadlets START a container from an image, they cannot attach to an existing one.
#
# Also: `systemctl --user enable` refuses a generated quadlet unit ("transient or
# generated"), so a hand-written .service under ~/.config/systemd/user/ is the right
# shape for something that must be enabled and restarted.
set -uo pipefail

UNIT_DIR="$HOME/.config/systemd/user"
UNIT="$UNIT_DIR/laya-sidecar.service"
QD="$HOME/.config/containers/systemd"
STAMP="$(date +%Y%m%dT%H%M%S)"
PODMAN="$(command -v podman)"

echo "=== 0. is the hand-started sidecar still alive? ==="
podman exec hermes /bin/bash -c 'pgrep -af "laya-sidecar.py" || echo "  NOT RUNNING (the pkill from the previous attempt stopped it)"' 2>&1 | sed 's/^/  /'

echo
echo "=== 1. retire the WRONG quadlet (move aside, never delete) ==="
for f in "$QD/laya-sidecar.container" "$QD/laya-sidecar.service"; do
  if [ -e "$f" ]; then mv "$f" "$f.bak-$STAMP"; echo "  moved aside: $f"; fi
done

mkdir -p "$UNIT_DIR"
if [ -e "$UNIT" ]; then mv "$UNIT" "$UNIT.bak-$STAMP"; echo "  moved aside: $UNIT"; fi

echo
echo "=== 2. write the user service (podman path: $PODMAN) ==="
cat > "$UNIT" <<EOF
[Unit]
Description=laya-sidecar — the Laya decision head (inside the \`hermes\` container)
Documentation=https://github.com/bitscrafts/laya-rs/blob/master/_docs/05-operations/SUPERVISION.md
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# Attach to the RUNNING container. The venv's python is a symlink into the container's
# /root/.local/share/uv/..., so the interpreter does not exist on the host — this can
# only be run from inside. Verified: host-side exec gives "Permission denied".
ExecStart=$PODMAN exec --name hermes /root/.local/bin/laya-sidecar
# The whole point of the unit: a head that dies must come back by itself. The failure
# this replaces was a dead head nobody noticed, with \`doctor\` still reporting READY.
Restart=always
RestartSec=5
# The head loads in ~24-29 s. systemd must not treat that as a failure, and must not
# interrupt the load to "restart" it.
TimeoutStartSec=180
TimeoutStopSec=45

[Install]
WantedBy=default.target
EOF
echo "  wrote: $UNIT"

echo
echo "=== 3. reload, enable, start ==="
systemctl --user daemon-reload
systemctl --user enable laya-sidecar.service 2>&1 | sed 's/^/  /'
systemctl --user restart laya-sidecar.service 2>&1 | sed 's/^/  /'

echo
echo "=== 4. wait for the head (loads ~24-29 s), polling the REAL check ==="
ok=no
for i in $(seq 1 30); do
  out="$(podman exec hermes /root/.local/bin/laya doctor 2>&1)"
  if printf '%s' "$out" | grep -q '^laya: READY'; then
    echo "  READY after ~$((i * 5)) s"
    printf '%s\n' "$out" | sed 's/^/    /'
    ok=yes
    break
  fi
  sleep 5
done
[ "$ok" = yes ] || echo "  NOT ready yet after 150 s — inspect: systemctl --user status laya-sidecar"

echo
echo "=== 5. unit state ==="
systemctl --user status laya-sidecar.service --no-pager 2>&1 | head -12 | sed 's/^/  /'
echo
echo "=== 6. enabled at boot? ==="
systemctl --user is-enabled laya-sidecar.service 2>&1 | sed 's/^/  /'
