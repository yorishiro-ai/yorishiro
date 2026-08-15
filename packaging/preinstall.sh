#!/bin/sh
# Creates the service account the unit runs as. Idempotent: an upgrade re-runs this.
set -e
if ! getent group yorishiro >/dev/null; then
    groupadd --system yorishiro
fi
if ! getent passwd yorishiro >/dev/null; then
    useradd --system --gid yorishiro --home-dir /var/lib/yorishiro \
            --shell /usr/sbin/nologin --comment "Yorishiro server" yorishiro
fi
