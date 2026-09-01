#!/bin/sh
# The state directory and the environment file are created here rather than shipped as package
# contents: a settings file the package owns would be overwritten or prompt on every upgrade.
set -e
mkdir -p /var/lib/yorishiro
chown yorishiro:yorishiro /var/lib/yorishiro

ENVFILE=/etc/yorishiro/yorishiro.env

mkdir -p /etc/yorishiro

if [ ! -e "$ENVFILE" ]; then
    cat > "$ENVFILE" <<'ENVEOF'
# Yorishiro settings, read by the systemd unit as the process environment.
#
# This is where configuration goes. The YAML files under
# /usr/share/yorishiro/config/ belong to the package and read their values from
# the variables set here, so editing those directly is undone by the next upgrade.
#
# Every variable and its default: /usr/share/yorishiro/docs/configuration.md
#
# production.yaml ships defaults for every variable (SQLite database, queue deriving
# from DATABASE_URL's scheme, HOST=http://localhost), so the environment file below
# is only needed when you override those defaults: point DATABASE_URL at PostgreSQL,
# set HOST to a reachable address for external callers, or set YORISHIRO_QUEUE_KIND
# to Redis for a separate queue backend.

# DATABASE_URL=postgres://USER:PASSWORD@localhost:5432/yorishiro
# HOST=http://localhost:5150
ENVEOF
    chmod 0640 "$ENVFILE"
    chown root:yorishiro "$ENVFILE"
fi

systemctl daemon-reload >/dev/null 2>&1 || true
