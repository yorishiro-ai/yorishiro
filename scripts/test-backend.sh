#!/usr/bin/env bash
set -u

backend=${1:?usage: scripts/test-backend.sh <postgres|sqlite> [cargo test arguments...]}
shift

case "$backend" in
  postgres|sqlite) ;;
  *) echo "ERROR: unsupported backend: $backend" >&2; exit 2 ;;
esac

marker_dir=$(mktemp -d "${TMPDIR:-/tmp}/yorishiro-backend-markers.XXXXXX")
trap 'rm -rf "$marker_dir"' EXIT
export YORISHIRO_BACKEND_MARKER_DIR=$marker_dir

set +e
cargo test --locked --workspace "$@"
test_status=$?
set -e

verification_status=0
for suite in postgres sqlite; do
  executed=$(find "$marker_dir/$suite/executed" -type f 2>/dev/null | wc -l)
  skipped=$(find "$marker_dir/$suite/skipped" -type f 2>/dev/null | wc -l)
  selected=$((executed + skipped))
  echo "backend suite $suite gates: selected=$selected executed=$executed skipped=$skipped"
  if [ "$skipped" -gt 0 ]; then
    reason=$(find "$marker_dir/$suite/skipped" -type f -print -quit | xargs -r cat)
    echo "backend suite $suite skip reason: $reason"
  fi

  if [ "$suite" = "$backend" ] && [ "$executed" -eq 0 ]; then
    echo "ERROR: $backend job executed zero $backend-specific tests" >&2
    verification_status=1
  fi
  if [ "$suite" != "$backend" ] && [ "$executed" -ne 0 ]; then
    echo "ERROR: $backend job executed $executed $suite-specific tests" >&2
    verification_status=1
  fi
done

if [ "$test_status" -ne 0 ]; then
  exit "$test_status"
fi
exit "$verification_status"
