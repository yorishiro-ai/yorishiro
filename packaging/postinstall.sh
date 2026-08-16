#!/bin/sh
# The state directory and the config file are created here rather than shipped as package
# contents: a config file the package owns would be overwritten or prompt on every upgrade.
set -e
mkdir -p /var/lib/yorishiro
chown yorishiro:yorishiro /var/lib/yorishiro

CONFIG=/etc/yorishiro/config.yml
LEGACY=/etc/yorishiro/yorishiro.env

mkdir -p /etc/yorishiro

if [ ! -e "$CONFIG" ]; then
    cat > "$CONFIG" <<'YAMLEOF'
# Yorishiro configuration.
# Every setting, its default and what it does: /etc/yorishiro/config.example.yml
#
# Only `database_url` has no working default. It is deliberately left commented out rather
# than filled in: a predictable user and password here would start the service against
# whatever local database happened to accept them, and an installation that fails loudly is
# better than one that silently connects to the wrong place with credentials everyone knows.
#
# Until it is set, the service exits 78 (EX_CONFIG) and systemd stops rather than retrying.

# database_url: postgres://USER:PASSWORD@localhost:5432/yorishiro
YAMLEOF
    chmod 0640 "$CONFIG"
    chown root:yorishiro "$CONFIG"
fi

# A machine carrying settings in the environment file needs to be told, in the file it will be
# sent to, that the service does not read them -- otherwise it comes back up unconfigured and
# stops at 78 with nothing pointing at the cause.
#
# Not migrated automatically: the env file is shell syntax with no schema, an aliased or
# hand-added variable has no reliable mapping into YAML, and a half-translated config that
# starts is worse than one that does not.
if [ -e "$LEGACY" ] && grep -qE '^[[:space:]]*[A-Z_]+=' "$LEGACY" 2>/dev/null; then
    if ! grep -q 'yorishiro.env' "$CONFIG" 2>/dev/null; then
        cat >> "$CONFIG" <<'NOTEEOF'

# ---------------------------------------------------------------------------------------
# /etc/yorishiro/yorishiro.env holds settings, and the service does not read that file.
#
# Move what it holds into this one. The names map directly -- lowercased, with the
# YORISHIRO_ prefix dropped:
#
#   DATABASE_URL=postgres://...        ->  database_url: postgres://...
#   YORISHIRO_BIND=0.0.0.0:8080        ->  bind: 0.0.0.0:8080
#   YORISHIRO_EMBEDDING_PROVIDER=local ->  embedding:
#                                            provider: local
#
# config.example.yml beside this file shows the nesting for every setting. Delete
# yorishiro.env and this note once the move is done.
# ---------------------------------------------------------------------------------------
NOTEEOF
        echo "yorishiro: settings in /etc/yorishiro/yorishiro.env are not read; see the note at the end of $CONFIG" >&2
    fi
fi

systemctl daemon-reload >/dev/null 2>&1 || true
