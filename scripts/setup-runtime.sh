#!/usr/bin/env bash
# Create the sidecar's OWN pinned venv.
#
# Why a new venv rather than reusing an existing one: `import laya` is not available
# from system python (verified), and reaching into ANOTHER PROJECT'S venv
# (/projects/digital-marketing/.venv) is the fragile coupling that the design review
# flagged and that the Rust head port exists to remove. The sidecar must not break
# when an unrelated project reinstalls its dependencies.
#
# Pinned: laya==0.3.5 is the version the measurements were taken against.
set -euo pipefail

VENV=/projects/laya-rs/.venv-laya
UV=/root/.local/bin/uv
LAYA_VERSION="${LAYA_VERSION:-0.3.5}"

echo "=== creating $VENV ==="
"$UV" venv "$VENV" --python 3.12 2>&1 | tail -3

echo "=== installing laya==$LAYA_VERSION (this pulls torch; ~5 GB) ==="
"$UV" pip install --python "$VENV/bin/python" "laya==$LAYA_VERSION" 2>&1 | tail -6

echo "=== verifying the import + version ==="
"$VENV/bin/python" -c "
from importlib.metadata import version
import laya, torch
print('laya', version('laya'))
print('torch', torch.__version__, 'cuda_available:', torch.cuda.is_available())
print('laya module:', laya.__file__)
"
echo "=== venv size ==="
du -sh "$VENV" 2>/dev/null
