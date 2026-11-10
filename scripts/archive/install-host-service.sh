#!/usr/bin/env bash
# Install `laya-sidecar` as a SUPERVISED service on the host.
#
# WHY THE HOST: inside the container there is no systemd (PID 1 is podman-init), no
# podman, and no crontab — verified. Nothing inside can supervise a daemon, so the
# sidecar dies with the container and stays dead.
#
# WHY `podman exec` AND NOT A DIRECT EXEC: the venv's `python` is a symlink into the
# container's /root/.local/share/uv/..., so the interpreter does not exist on the host
# at all. Verified: running it from the host gives "Permission denied". The sidecar can
# ONLY run inside the container, so the host unit drives it there.
#
# WHY A UNIT AND NOT THE IMAGE'S ENTRYPOINT: /usr/local/bin/entrypoint.sh belongs to the
# image, so an edit there is reverted by the next image update. A host unit lives
# outside the image — upgrade-proof — and follows the sibling pattern already in place
# here (the host runs its own units for the model server and anything else it needs).
set -uo pipefail

UNIT_DIR="$HOME/.config/containers/systemd"
UNIT="$UNIT_DIR/laya-sidecar.container"
STAMP="$(date +%Y%m%dT%H%M%S)"

mkdir -p "$UNIT_DIR"

# Never overwrite silently: move any existing unit aside first.
for f in "$UNIT" "$UNIT_DIR/laya-sidecar.service"; do
  if [ -e "$f" ]; then
    mv "$f" "$f.bak-$STAMP"
    echo "  moved aside: $f -> $f.bak-$STAMP"
  fi
done

cat > "$UNIT" <<'EOF'
[Unit]
Description=laya-sidecar — the Laya decision head (runs inside the `hermes` container)
Documentation=file:/podman/assistants/hermes/projects/laya-rs/_docs/05-operations/SUPERVISION.md
After=network-online.target hermes.service llamacpp-laya.service
Requires=hermes.service
# A restart of the container kills the exec'd sidecar; systemd brings it back via
# Restart=always, so there is no need to chain on the container's stop.

[Container]
# `podman exec` INTO the running hermes container. The image is a placeholder: this
# unit never starts a container, it attaches to one, so the image reference is only
# used to locate podman's runtime for the exec.
Image=localhost/pidag-runner:latest
ContainerName=hermes
Exec=/root/.local/bin/laya-sidecar
User=root
Environment=LAYA_HEAD=/projects/_tmp/laya-gguf/laya-head.safetensors
Environment=LAYA_SIDECAR_SOCKET=/run/laya/sidecar.sock

[Service]
# `always` is the point of this unit: the failure this replaces is a dead head that
# nobody noticed. It must come back on its own.
Restart=always
RestartSec=5
# The head loads in ~24-29 s. systemd must not conclude it failed, and must not
# interrupt the load to "restart" it.
TimeoutStartSec=180
TimeoutStopSec=45

[Install]
WantedBy=default.target
EOF

echo "  wrote: $UNIT"
echo

# The sidecar refuses to start if another one holds the socket (by design). The
# hand-started one from this session must go first, or the service would loop.
echo "=== stopping the hand-started sidecar (started by nohup earlier this session) ==="
podman exec hermes /bin/bash -c 'pkill -f "scripts/laya-sidecar.py" 2>/dev/null; sleep 1; pgrep -af "laya-sidecar.py" || echo "  none running"' 2>&1 | sed 's/^/  /'

echo
echo "=== reload + enable + start ==="
systemctl --user daemon-reload
systemctl --user enable --now laya-sidecar.service 2>&1 | sed 's/^/  /' || true

# The head needs ~30 s to load; poll for readiness rather than guessing a sleep.
echo
echo "=== waiting for the head to come up (loads ~24-29 s) ==="
for i in $(seq 1 24); do
  out="$(podman exec hermes /root/.local/bin/laya doctor 2>&1)"
  if printf '%s' "$out" | grep -q '^laya: READY'; then
    echo "  ready after ~$((i * 5)) s"
    printf '%s\n' "$out" | sed 's/^/    /'
    break
  fi
  sleep 5
done

echo
echo "=== unit state ==="
systemctl --user status laya-sidecar.service --no-pager 2>&1 | head -14 | sed 's/^/  /'
