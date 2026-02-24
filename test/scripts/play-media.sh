#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <media-file>" >&2
  exit 2
fi

media="$1"
if [[ ! -f "$media" ]]; then
  echo "media file not found: $media" >&2
  exit 1
fi

if command -v mpv >/dev/null 2>&1; then
  exec mpv \
    --no-config \
    --really-quiet \
    --no-audio \
    --loop-file=inf \
    --image-display-duration=inf \
    --fullscreen \
    "$media"
fi

if command -v imv >/dev/null 2>&1; then
  exec imv -f "$media"
fi

if command -v feh >/dev/null 2>&1; then
  exec feh -F -Z -Y -x -q -R 0.1 "$media"
fi

echo "no media viewer found (tried mpv, imv, feh)" >&2
exec sleep infinity
