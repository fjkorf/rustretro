#!/usr/bin/env bash
#
# Pre-build hook: ensure the APFS build volume is mounted before cargo runs.
#
# Build artifacts live on an APFS sparse-bundle disk image on the external
# Samsung T5 (see .cargo/config.toml -> target-dir). The image must be mounted
# or cargo's target-dir is unreachable and the build fails. This script is
# idempotent: it mounts the image only if it isn't already mounted.
#
# Usage:
#   ./scripts/mount-build-volume.sh   # mount if needed
#   ./scripts/mount-build-volume.sh && cargo build
#
# Exit codes: 0 = volume is mounted (was already, or we just mounted it);
#             1 = could not mount (e.g. the T5 isn't plugged in).

set -euo pipefail

MOUNT_POINT="/Volumes/rustretro-build"
IMAGE="/Volumes/Samsung_T5/rustretro-build.sparsebundle"

# Already mounted? Nothing to do.
if [ -d "$MOUNT_POINT" ] && mount | grep -q " on $MOUNT_POINT "; then
  exit 0
fi

# The image lives on the T5 — if that isn't plugged in, we can't proceed.
if [ ! -e "$IMAGE" ]; then
  echo "error: build image not found at $IMAGE" >&2
  echo "       Is the Samsung T5 plugged in and mounted?" >&2
  exit 1
fi

echo "Mounting build volume: $IMAGE -> $MOUNT_POINT"
if hdiutil attach "$IMAGE" >/dev/null 2>&1; then
  # Confirm it actually came up at the expected mount point.
  if [ -d "$MOUNT_POINT" ] && mount | grep -q " on $MOUNT_POINT "; then
    exit 0
  fi
  echo "error: attached the image but $MOUNT_POINT is not mounted" >&2
  exit 1
else
  echo "error: failed to attach $IMAGE" >&2
  exit 1
fi
