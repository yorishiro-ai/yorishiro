#!/usr/bin/env bash
#
# Starts the service the way an operator does: `systemctl`, under systemd as PID 1.
#
# Separate from test-install.sh because this needs a privileged container, and the checks there
# must stay runnable without one. What it covers is what `systemd-analyze verify` cannot: that a
# unit whose syntax is valid actually brings the service up.
#
#   ./packaging/test-systemd.sh <directory holding the .deb files>
#
# Needs docker with --privileged. ubuntu:24.04 and the deb only: the rpm under systemd is
# checked by hand, because putting an EOL Fedora's package repositories on the critical path of
# every pull request trades a real dependency for a marginal case.
#
# One package, so one run. An earlier version looped over `ce` and `ee` because they were
# different binaries behind the same unit name, which made "it starts under systemd" a separate
# fact about each. There is one binary now.
#
# The unconfigured-start section exercises the zero-config default: production.yaml boots
# against a local SQLite file with no external dependencies (DATABASE_URL defaults to
# sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc, HOST defaults to http://localhost,
# queue derives from DATABASE_URL's scheme), so an unconfigured install starts successfully
# and serves a single-tenant trial instance. Embeddings are disabled to avoid fetching the
# ~1 GiB model in CI. The later phase sets DATABASE_URL/QUEUE_URL to Postgres explicitly
# to verify that path as well.

set -uo pipefail

PKG_DIR="${1:?usage: test-systemd.sh <package directory>}"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }

DEB="$(basename "$(ls "$PKG_DIR"/yorishiro_*.deb | head -1)")"
NET="ysr-sd-$$" APP="ysr-sd-app-$$" PG="ysr-sd-pg-$$"

cleanup() {
  docker rm -f "$APP" "$PG" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

docker network create "$NET" >/dev/null 2>&1
docker run -d --name "$PG" --network "$NET" \
  -e POSTGRES_USER=yorishiro -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=yorishiro \
  pgvector/pgvector:pg18 >/dev/null

# systemd has to be PID 1, which is what `exec` at the end of the command is for, and needs both
# tmpfs mounts plus a cgroup namespace it can write.
docker run -d --name "$APP" --network "$NET" --privileged --cgroupns=host \
  --tmpfs /run --tmpfs /run/lock -v "$PKG_DIR":/pkg:ro ubuntu:24.04 \
  bash -c 'apt-get update -qq >/dev/null 2>&1
           apt-get install -y -qq systemd systemd-sysv curl >/dev/null 2>&1
           exec /lib/systemd/systemd' >/dev/null

booted=
for _ in $(seq 1 60); do
  case "$(docker exec "$APP" systemctl is-system-running 2>/dev/null)" in
    running|degraded) booted=1; break ;;
  esac
  sleep 3
done
if [ -z "$booted" ]; then
  echo "systemd never finished booting in the container" >&2
  exit 1
fi

# --------------------------------------------------------------------------------------------
note "an unconfigured start succeeds (SQLite trial default)"
# --------------------------------------------------------------------------------------------
docker exec "$APP" bash -c "apt-get install -y -qq /pkg/$DEB >/dev/null 2>&1" || {
  echo "installing the package failed" >&2; exit 1
}

# Disable embeddings to avoid the ~1 GiB model fetch in CI.
# The zero-config defaults are SQLite + local provider, but fetching a gigabyte is
# impractical for a smoke test that should complete in under a minute.
docker exec "$APP" bash -c "cat > /etc/yorishiro/yorishiro.env <<EOF
YORISHIRO_EMBEDDING_PROVIDER=none
EOF"

docker exec "$APP" systemctl reset-failed yorishiro >/dev/null 2>&1
docker exec "$APP" systemctl start yorishiro >/dev/null 2>&1
# Short timeout: no model fetch means a fast boot (~2-3 seconds).
for _ in $(seq 1 20); do
  docker exec "$APP" curl -fsS http://127.0.0.1:5150/_ping >/dev/null 2>&1 && break
  sleep 3
done

state=$(docker exec "$APP" bash -c '
  echo "active=$(systemctl is-active yorishiro)"
  echo "ping=$(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:5150/_ping)"' 2>&1)

if grep -q 'active=active' <<<"$state"; then
  ok "the unconfigured service is active"
else
  bad "expected active for unconfigured boot, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
fi
if grep -q 'ping=200' <<<"$state"; then
  ok "the unconfigured service answers /_ping"
else
  bad "expected 200 from /_ping, got: $(grep -o 'ping=[0-9]*' <<<"$state")"
fi

# --------------------------------------------------------------------------------------------
note "a configured service starts and serves (Postgres)"
# --------------------------------------------------------------------------------------------
docker exec "$PG" psql -U yorishiro -d yorishiro \
  -c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm;" >/dev/null 2>&1

# By address, not by name. systemd-resolved takes over /etc/resolv.conf on boot and its stub
# cannot reach Docker's embedded DNS from inside the container, so the container's own name
# lookups stop working: an artefact of running systemd in Docker, not something an operator meets
# on a real host.
PGIP=$(docker inspect "$PG" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
docker exec "$APP" bash -c "cat > /etc/yorishiro/yorishiro.env <<EOF
DATABASE_URL=postgres://yorishiro:secret@$PGIP:5432/yorishiro
QUEUE_URL=postgres://yorishiro:secret@$PGIP:5432/yorishiro
HOST=http://127.0.0.1:5150
YORISHIRO_EMBEDDING_PROVIDER=none
EOF"

docker exec "$APP" bash -c '
  systemctl reset-failed yorishiro
  systemctl daemon-reload
  systemctl enable --now yorishiro' >/dev/null 2>&1

for _ in $(seq 1 60); do
  docker exec "$APP" curl -fsS http://127.0.0.1:5150/_ping >/dev/null 2>&1 && break
  sleep 3
done

state=$(docker exec "$APP" bash -c '
  echo "active=$(systemctl is-active yorishiro)"
  echo "enabled=$(systemctl is-enabled yorishiro)"
  echo "ping=$(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:5150/_ping)"' 2>&1)

grep -q 'active=active' <<<"$state" \
  && ok "the service is active" \
  || bad "expected active, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
grep -q 'enabled=enabled' <<<"$state" \
  && ok "the service is enabled" \
  || bad "expected enabled, got: $(grep -o 'enabled=[a-z-]*' <<<"$state")"
grep -q 'ping=200' <<<"$state" \
  && ok "it answers /_ping" \
  || bad "expected 200 from /_ping, got: $(grep -o 'ping=[0-9]*' <<<"$state")"

# It runs as the unpriviliged account the package creates, not as root: the unit says `User=`,
# and a package that installed a unit systemd silently ran as root would look identical here
# without this check.
owner=$(docker exec "$APP" systemctl show yorishiro -p MainPID --value 2>/dev/null)
if [ -n "$owner" ] && [ "$owner" != 0 ]; then
  who=$(docker exec "$APP" ps -o user= -p "$owner" 2>/dev/null | tr -d ' ')
  [ "$who" = "yorishiro" ] \
    && ok "the process runs as yorishiro" \
    || bad "expected the process to run as yorishiro, got: ${who:-unknown}"
else
  bad "the service has no main PID to inspect"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
