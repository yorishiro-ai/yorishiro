#!/bin/sh
# The state directory and the env file are created here rather than shipped as package contents:
# a config file the package owns would be overwritten or prompt on every upgrade.
set -e
mkdir -p /var/lib/yorishiro
chown yorishiro:yorishiro /var/lib/yorishiro

if [ ! -e /etc/yorishiro/yorishiro.env ]; then
    mkdir -p /etc/yorishiro
    cat > /etc/yorishiro/yorishiro.env <<'ENVEOF'
# Required. See /usr/share/doc/yorishiro/config.example.yml for everything else.
DATABASE_URL=postgres://yorishiro:yorishiro@localhost:5432/yorishiro
ENVEOF
    chmod 0640 /etc/yorishiro/yorishiro.env
    chown root:yorishiro /etc/yorishiro/yorishiro.env
fi

systemctl daemon-reload >/dev/null 2>&1 || true
