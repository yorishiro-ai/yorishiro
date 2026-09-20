#!/bin/sh
# The mutable state directory is created at install time.
# The editable canonical YAML template is installed by nfpm as config|noreplace.
set -e
mkdir -p /var/lib/yorishiro
chown yorishiro:yorishiro /var/lib/yorishiro

systemctl daemon-reload >/dev/null 2>&1 || true
