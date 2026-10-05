#!/usr/bin/env bash
#
# Starts the service the way an operator does: `systemctl`, under systemd as PID 1.
#
# Separate from test-install.sh because this needs a privileged container, and the checks there
# must stay runnable without one. What it covers is what `systemd-analyze verify` cannot: that a
# unit whose syntax is valid actually brings the service up.
#
#   ./packaging/test-systemd.sh <exact deb package file>
#
# Needs docker with --privileged. ubuntu:24.04 and the deb only: the rpm under systemd is
# checked by hand, because putting an EOL Fedora's package repositories on the critical path of
# every pull request trades a real dependency for a marginal case.
#
# The caller supplies one exact edition-first package filename.
#
# The first phase exercises the default SQLite deployment without external dependencies.
# DATABASE_URL defaults to sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc.
# HOST defaults to http://localhost.
# The queue uses its separate packaged SQLite file.
# The smoke test sets an OpenAI-compatible embedding configuration before this phase starts.
# The provider is constructed synchronously without calling the configured endpoint.
# This avoids downloading the approximately 1 GiB local model.
# The later phase switches the database to PostgreSQL.

set -uo pipefail

PKG_FILE="${1:?usage: test-systemd.sh <exact deb package file>}"
PKG_FILE="$(cd "$(dirname "$PKG_FILE")" && pwd)/$(basename "$PKG_FILE")"
case "$(basename "$PKG_FILE")" in
  yorishiro-ce-*-amd64.deb|yorishiro-ee-*-amd64.deb|yorishiro-ce-*-arm64.deb|yorishiro-ee-*-arm64.deb) ;;
  *) echo "package filename is not edition-first: $(basename "$PKG_FILE")" >&2; exit 2 ;;
esac
PKG_DIR="$(dirname "$PKG_FILE")"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }
# The container's PID 1 is systemd, so `docker logs` never carries the service's output.
dump_logs() { docker exec "$APP" journalctl -u yorishiro -u yorishiro-worker -n 40 --no-pager >&2; }

DEB="$(basename "$PKG_FILE")"
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
note "a packaged SQLite start succeeds (default database)"
# --------------------------------------------------------------------------------------------
docker exec "$APP" bash -c "apt-get install -y -qq /pkg/$DEB >/dev/null 2>&1" || {
  echo "installing the package failed" >&2; exit 1
}

# Packaging assertions: wrapper exists, is executable, service points to it.
docker exec "$APP" test -f /var/lib/yorishiro/worker-wrapper.sh \
  && ok "wrapper script is installed at /var/lib/yorishiro/worker-wrapper.sh" \
  || bad "wrapper script not found at /var/lib/yorishiro/worker-wrapper.sh"
docker exec "$APP" test -x /var/lib/yorishiro/worker-wrapper.sh \
  && ok "wrapper script is executable" \
  || bad "wrapper script is not executable"
execstart=$(docker exec "$APP" bash -c '
  grep -m1 "^ExecStart=" /lib/systemd/system/yorishiro-worker.service' 2>/dev/null)
echo "$execstart" | grep -q "worker-wrapper" \
  && ok "yorishiro-worker.service ExecStart points to the wrapper" \
  || bad "yorishiro-worker.service ExecStart does not reference the wrapper: $execstart"

# Integrity probes: /var/lib/yorishiro is root:yorishiro mode 1770.
# Root records wrapper checksum and creates a protected backup before
# unprivileged probes.  After yorishiro's attempts, root verifies the
# wrapper is intact and atomically restores from backup.  All assertions
# happen on the host via markers returned from docker exec.
integrity=$(docker exec "$APP" bash -c '
  stat -c "%U:%G" /var/lib/yorishiro | grep -q "^root:yorishiro$" && echo "INTEG_OWNER" || echo "INTEG_OWNERFAIL"
  stat -c "%a" /var/lib/yorishiro | grep -q "^1770$" && echo "INTEG_PERMS" || echo "INTEG_PERMSFAIL"
  # Record wrapper metadata for post-probe verification.
  stat -c "%U:%G %a" /var/lib/yorishiro/worker-wrapper.sh > /tmp/wrapper-meta.bak
  md5sum /var/lib/yorishiro/worker-wrapper.sh > /tmp/wrapper-md5.bak
  # Create a root-owned backup outside the state directory.
  cp /var/lib/yorishiro/worker-wrapper.sh /root/worker-wrapper.backup
  echo "INTEG_BACKED"
')
# Unprivileged probes as the yorishiro user via docker exec --user.
unpriv=$(docker exec --user yorishiro "$APP" bash -c '
  # State-file creation must succeed (directory is group-writable).
  touch /var/lib/yorishiro/writable-as-yorishiro && echo "UNPRIV_WRITE" || echo "UNPRIV_WRITEFAIL"
  # rm test: wrapper is root-owned; yorishiro cannot delete it.
  rm -f /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null || true
  if [ -f /var/lib/yorishiro/worker-wrapper.sh ]; then
    echo "UNPRIV_UNRMED"
  else
    echo "UNPRIV_GONE"
  fi
  # Sticky-rename test: mv -f (force) as yorishiro over root-owned wrapper.
  # The sticky bit on the directory blocks the rename because yorishiro does
  # not own the target file.
  touch /var/lib/yorishiro/unpriv-tempfile
  mv -f /var/lib/yorishiro/unpriv-tempfile /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null || true
  if [ -f /var/lib/yorishiro/worker-wrapper.sh ] \
     && [ "$(stat -c %U /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null)" = "root" ]; then
    echo "UNPRIV_MV_INTEGRITY"
  else
    echo "UNPRIV_MV_CORRUPT"
  fi
  # Shell redirection overwrite attempt as yorishiro.
  echo "tampered" > /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null && {
    echo "UNPRIV_REDIRECT_OK"
  } || echo "UNPRIV_REDIRECT_FAIL"
')
# Root verifies wrapper is still intact after unprivileged probes, then
# atomically restores from backup using cp-to-temp + mv.  set -e ensures
# any restore step failure aborts the block rather than silently proceeding.
restore=$(docker exec "$APP" bash -c '
  set -e
  # Check wrapper metadata has not changed.
  meta=$(stat -c "%U:%G %a" /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null)
  orig=$(cat /tmp/wrapper-meta.bak)
  if [ "$meta" = "$orig" ]; then
    echo "INTEG_META_INTACT"
  else
    echo "INTEG_META_CHANGED"
  fi
  # Atomic restore: copy backup to a temp file in the same directory,
  # set temp ownership to root:root, then mv -f over the wrapper.
  cp /root/worker-wrapper.backup /var/lib/yorishiro/worker-wrapper.sh.tmp
  chown root:root /var/lib/yorishiro/worker-wrapper.sh.tmp
  chmod 0755 /var/lib/yorishiro/worker-wrapper.sh.tmp
  mv -f /var/lib/yorishiro/worker-wrapper.sh.tmp /var/lib/yorishiro/worker-wrapper.sh
  chown root:yorishiro /var/lib/yorishiro/worker-wrapper.sh
  # Verify restored file matches original checksum and exact metadata.
  restored_md5=$(md5sum /var/lib/yorishiro/worker-wrapper.sh | cut -d" " -f1)
  orig_md5=$(cat /tmp/wrapper-md5.bak | cut -d" " -f1)
  restored_meta=$(stat -c "%U:%G %a" /var/lib/yorishiro/worker-wrapper.sh)
  if [ "$restored_md5" = "$orig_md5" ] && [ "$restored_meta" = "root:yorishiro 755" ]; then
    echo "INTEG_RESTORED"
  else
    echo "INTEG_RESTORE_FAILED"
  fi
')
for marker in INTEG_OWNER INTEG_PERMS; do
  case "$integrity" in *"$marker"*) ok "$marker" ;; *) bad "$marker ($integrity)" ;; esac
done
for marker in UNPRIV_WRITE UNPRIV_UNRMED UNPRIV_MV_INTEGRITY UNPRIV_REDIRECT_FAIL; do
  case "$unpriv" in *"$marker"*) ok "$marker" ;; *) bad "$marker ($unpriv)" ;; esac
done
for marker in INTEG_META_INTACT INTEG_RESTORED; do
  case "$restore" in *"$marker"*) ok "$marker" ;; *) bad "$marker ($restore)" ;; esac
done

# Use an OpenAI-compatible provider configuration so both the server and the worker construct the provider at startup without downloading the ~1 GiB local model.
# `base_url` + `model` take priority over the enum in `build_embedding_provider`, so no `provider` variable is needed.
# `none` is rejected (#524) because the server needs query embeddings in this smoke test.
# The endpoint is not reachable.
# The provider constructor is synchronous and never calls it.
docker exec "$APP" systemctl set-environment \
  YORISHIRO_EMBEDDING_BASE_URL=http://127.0.0.1:5151/v1 \
  YORISHIRO_EMBEDDING_MODEL=package-smoke-test

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
  ok "the packaged SQLite service is active"
else
  bad "expected active for packaged SQLite start, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
fi
if grep -q 'ping=200' <<<"$state"; then
  ok "the packaged SQLite service answers /_ping"
else
  bad "expected 200 from packaged SQLite start, got: $(grep -o 'ping=[0-9]*' <<<"$state")"
  dump_logs
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

# The package ships /etc/yorishiro/production.yaml and its header documents the variables an
# operator sets to point it at PostgreSQL, so this sets those instead of writing a second
# configuration. A hand-written copy drifts from the shipped one: it once omitted `settings:`
# and the server exited at start with "missing field `max_tenants`".
docker exec "$APP" systemctl set-environment \
  DATABASE_URL="postgres://yorishiro:secret@$PGIP:5432/yorishiro" \
  HOST=http://127.0.0.1:5150 \
  DB_CONNECT_TIMEOUT=5000

# Use `restart` instead of `enable --now` because the packaged SQLite phase left the server running.
# Starting an active unit would not reload the new environment while the old server kept answering /_ping.
# Restart the server before the worker and wait for its ping.
# Both processes run automigrate, and starting them together races on PostgreSQL role creation.
# `After=yorishiro.service` only orders the systemd start jobs and does not wait for server readiness.
docker exec "$APP" bash -c '
  systemctl reset-failed yorishiro
  systemctl daemon-reload
  systemctl enable yorishiro yorishiro-worker
  systemctl restart yorishiro' >/dev/null 2>&1

server_ready=
# Wait for the server before starting the worker.
for _ in $(seq 1 120); do
  docker exec "$APP" curl -fsS http://127.0.0.1:5150/_ping >/dev/null 2>&1 \
    && server_ready=1
  if [ -n "$server_ready" ]; then
    break
  fi
  sleep 3
done

if [ -z "$server_ready" ]; then
  bad "server did not become ready before starting worker"
  dump_logs
  worker_skipped=1
  worker_assertion_recorded=1
fi

if [ -z "${worker_skipped:-}" ]; then
  docker exec "$APP" bash -c '
    systemctl reset-failed yorishiro-worker
    systemctl restart yorishiro-worker' >/dev/null 2>&1
fi

worker_ready=
if [ -z "${worker_skipped:-}" ]; then
  for _ in $(seq 1 120); do
    docker exec "$APP" systemctl is-active --quiet yorishiro-worker 2>/dev/null \
      && worker_ready=1
    [ -n "$worker_ready" ] && break
    sleep 3
  done
fi

if [ -z "${worker_skipped:-}" ] && [ -z "$worker_ready" ]; then
  bad "worker did not become ready"
  dump_logs
  worker_assertion_recorded=1
fi

state=$(docker exec "$APP" bash -c '
  echo "active=$(systemctl is-active yorishiro)"
  echo "worker-active=$(systemctl is-active yorishiro-worker)"
  echo "enabled=$(systemctl is-enabled yorishiro)"
  echo "worker-enabled=$(systemctl is-enabled yorishiro-worker)"
  echo "ping=$(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:5150/_ping)"' 2>&1)

grep -q 'active=active' <<<"$state" \
  && ok "the service is active" \
  || bad "expected active, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
if [ -n "${worker_skipped:-}" ]; then
  :
elif [ -n "${worker_assertion_recorded:-}" ]; then
  :
elif grep -q 'worker-active=active' <<<"$state"; then
  ok "the worker is active"
else
  bad "expected worker active, got: $(grep -o 'worker-active=[a-z-]*' <<<"$state")"
  dump_logs
fi
grep -q 'enabled=enabled' <<<"$state" \
  && ok "the service is enabled" \
  || bad "expected enabled, got: $(grep -o 'enabled=[a-z-]*' <<<"$state")"
grep -q 'worker-enabled=enabled' <<<"$state" \
  && ok "the worker is enabled" \
  || bad "expected worker enabled, got: $(grep -o 'worker-enabled=[a-z-]*' <<<"$state")"
grep -q 'ping=200' <<<"$state" \
  && ok "it answers /_ping" \
  || { bad "expected 200 from /_ping, got: $(grep -o 'ping=[0-9]*' <<<"$state")"; dump_logs; }

tables=$(docker exec "$PG" psql -U yorishiro -d yorishiro -Atc \
  "select count(*) from information_schema.tables where table_schema = 'public'" 2>/dev/null)
[ "${tables:-0}" -gt 0 ] 2>/dev/null \
  && ok "it applied its migrations to PostgreSQL" \
  || bad "PostgreSQL has no tables: the service is not running against it"

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
