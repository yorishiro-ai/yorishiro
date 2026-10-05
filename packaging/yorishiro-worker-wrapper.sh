#!/bin/sh
# Start the background worker with all registered tags.
#
# This wrapper exists because systemd's ExecStart does not perform shell
# substitution, so a bare start --worker=... would need a hand-maintained tag
# list that drifts when new worker classes or job types are added.  The wrapper
# invokes `yorishiro worker-tags` to discover every registered tag and passes
# them as a comma-separated list to `start --worker=`.
#
# The yorishiro binary is located via `command -v` so the same script works
# in both contexts:
#   - package deployment: binary at /usr/bin/yorishiro
#   - container / Docker image: binary at /usr/local/bin/yorishiro (or on PATH
#     via ENTRYPOINT)

YORISHIRO_BIN="$(command -v yorishiro)"
if [ -z "$YORISHIRO_BIN" ]; then
  echo "error: yorishiro binary not found on PATH" >&2
  exit 1
fi

exec "$YORISHIRO_BIN" start --worker="$("$YORISHIRO_BIN" worker-tags)"
