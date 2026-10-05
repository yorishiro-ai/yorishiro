#!/bin/sh
# Create the mutable state directory with integrity guarantees.
#
# root:yorishiro mode 1770 (sticky + group-writable) lets the yorishiro user
# create/update state files but prevents it from unlinking or renaming
# root-owned files such as the wrapper script.
set -e
mkdir -p /var/lib/yorishiro
chown root:yorishiro /var/lib/yorishiro
chmod 1770 /var/lib/yorishiro

systemctl daemon-reload >/dev/null 2>&1 || true
