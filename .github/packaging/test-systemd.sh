#!/usr/bin/env bash
# Verify installed services under systemd as PID 1 in a privileged Ubuntu container.
#   ./.github/packaging/test-systemd.sh <exact deb package file>

set -uo pipefail

PKG_FILE="${1:?usage: test-systemd.sh <exact deb package file>}"
PKG_FILE="$(cd "$(dirname "$PKG_FILE")" && pwd)/$(basename "$PKG_FILE")"
case "$(basename "$PKG_FILE")" in
  yorishiro-ce-*-amd64.deb|yorishiro-ee-*-amd64.deb|yorishiro-ce-*-arm64.deb|yorishiro-ee-*-arm64.deb) ;;
  *) echo "package filename is not edition-first: $(basename "$PKG_FILE")" >&2; exit 2 ;;
esac
PKG_DIR="$(dirname "$PKG_FILE")"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }
# The container's PID 1 is systemd, so `docker logs` never carries the service's output.
dump_logs() { docker exec "$APP" journalctl -u yorishiro -u yorishiro-worker -u yorishiro-jobs -u yorishiro-scheduler -n 40 --no-pager >&2; }

DEB="$(basename "$PKG_FILE")"
NET="ysr-sd-$$" APP="ysr-sd-app-$$" PG="ysr-sd-pg-$$"
SYSTEMD_IMAGE="yorishiro-systemd-test:$$"

cleanup() {
  docker rm -f "$APP" "$PG" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker image rm "$SYSTEMD_IMAGE" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

# Keep network-dependent package preparation outside the systemd boot deadline.
docker build -t "$SYSTEMD_IMAGE" - <<'DOCKERFILE' || exit 1
FROM ubuntu:26.04
RUN apt-get -o Acquire::Retries=5 update -qq \
    && apt-get -o Acquire::Retries=5 install -y -qq systemd systemd-sysv curl \
    && rm -rf /var/lib/apt/lists/*
CMD ["/lib/systemd/systemd"]
DOCKERFILE

docker network create "$NET" >/dev/null 2>&1
docker run -d --name "$PG" --network "$NET" \
  -e POSTGRES_USER=yorishiro -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=yorishiro \
  "$(docker compose -f "$REPO/docker-compose.test.yaml" config --format json | python3 -c 'import json,sys; print(json.load(sys.stdin)["services"]["testdb"]["image"])')" >/dev/null

# systemd needs writable runtime mounts and access to the host cgroup namespace.
docker run -d --name "$APP" --network "$NET" --privileged --cgroupns=host \
  --tmpfs /run --tmpfs /run/lock -v "$PKG_DIR":/pkg:ro "$SYSTEMD_IMAGE" >/dev/null || exit 1

booted=
for _ in $(seq 1 60); do
  case "$(docker exec "$APP" systemctl is-system-running 2>/dev/null)" in
    running|degraded) booted=1; break ;;
  esac
  sleep 3
done
if [ -z "$booted" ]; then
  docker logs "$APP" >&2 || true
  docker exec "$APP" systemctl list-jobs --no-pager >&2 || true
  docker exec "$APP" journalctl -b -n 80 --no-pager >&2 || true
  echo "systemd never finished booting in the container" >&2
  exit 1
fi

# --------------------------------------------------------------------------------------------
note "a packaged SQLite start succeeds (default database)"
# --------------------------------------------------------------------------------------------
docker exec "$APP" bash -c "apt-get -o Acquire::Retries=5 install -y -qq /pkg/$DEB >/dev/null 2>&1" || {
  echo "installing the package failed" >&2; exit 1
}

# Packaging assertions: the worker unit serves the shared target, and no wrapper is installed.
execstart=$(docker exec "$APP" bash -c '
  grep -m1 "^ExecStart=" /lib/systemd/system/yorishiro-worker.service' 2>/dev/null)
[ "$execstart" = "ExecStart=/usr/bin/yorishiro start --worker=worker-target:shared" ] \
  && ok "yorishiro-worker.service serves the shared target" \
  || bad "yorishiro-worker.service has an unexpected ExecStart: $execstart"
jobs_execstart=$(docker exec "$APP" grep -m1 "^ExecStart=" /lib/systemd/system/yorishiro-jobs.service 2>/dev/null)
[ "$jobs_execstart" = "ExecStart=/usr/bin/yorishiro start --worker=job-target:shared" ] \
  && ok "application jobs use a separate consumer" \
  || bad "unexpected application job consumer: $jobs_execstart"
docker exec "$APP" test ! -e /var/lib/yorishiro/worker-wrapper.sh \
  && ok "no worker wrapper is installed" \
  || bad "a worker wrapper is still installed at /var/lib/yorishiro/worker-wrapper.sh"
scheduler_execstart=$(docker exec "$APP" bash -c '
  grep -m1 "^ExecStart=" /lib/systemd/system/yorishiro-scheduler.service' 2>/dev/null)
echo "$scheduler_execstart" | grep -q '^ExecStart=/usr/bin/yorishiro scheduler$' \
  && ok "yorishiro-scheduler.service starts the Loco scheduler" \
  || bad "yorishiro-scheduler.service has an unexpected ExecStart: $scheduler_execstart"

# Verify the sticky state directory protects root-owned files from the service account.
integrity=$(docker exec "$APP" bash -c '
  stat -c "%U:%G" /var/lib/yorishiro | grep -q "^root:yorishiro$" && echo "INTEG_OWNER" || echo "INTEG_OWNERFAIL"
  stat -c "%a" /var/lib/yorishiro | grep -q "^1770$" && echo "INTEG_PERMS" || echo "INTEG_PERMSFAIL"
  # The package ships no root-owned file in the state directory, so the probes use one the test creates.
  install -m 0755 -o root -g yorishiro /dev/null /var/lib/yorishiro/root-owned-probe
  stat -c "%U:%G %a" /var/lib/yorishiro/root-owned-probe > /tmp/probe-meta.bak
  md5sum /var/lib/yorishiro/root-owned-probe > /tmp/probe-md5.bak
  cp /var/lib/yorishiro/root-owned-probe /root/root-owned-probe.backup
  echo "INTEG_BACKED"
')
unpriv=$(docker exec --user yorishiro "$APP" bash -c '
  touch /var/lib/yorishiro/writable-as-yorishiro && echo "UNPRIV_WRITE" || echo "UNPRIV_WRITEFAIL"
  rm -f /var/lib/yorishiro/root-owned-probe 2>/dev/null || true
  if [ -f /var/lib/yorishiro/root-owned-probe ]; then
    echo "UNPRIV_UNRMED"
  else
    echo "UNPRIV_GONE"
  fi
  # The sticky bit must also block replacing a root-owned target by rename.
  touch /var/lib/yorishiro/unpriv-tempfile
  mv -f /var/lib/yorishiro/unpriv-tempfile /var/lib/yorishiro/root-owned-probe 2>/dev/null || true
  if [ -f /var/lib/yorishiro/root-owned-probe ] \
     && [ "$(stat -c %U /var/lib/yorishiro/root-owned-probe 2>/dev/null)" = "root" ]; then
    echo "UNPRIV_MV_INTEGRITY"
  else
    echo "UNPRIV_MV_CORRUPT"
  fi
  echo "tampered" > /var/lib/yorishiro/root-owned-probe 2>/dev/null && {
    echo "UNPRIV_REDIRECT_OK"
  } || echo "UNPRIV_REDIRECT_FAIL"
')
# Fail closed if integrity verification or atomic restoration fails.
restore=$(docker exec "$APP" bash -c '
  set -e
  meta=$(stat -c "%U:%G %a" /var/lib/yorishiro/root-owned-probe 2>/dev/null)
  orig=$(cat /tmp/probe-meta.bak)
  if [ "$meta" = "$orig" ]; then
    echo "INTEG_META_INTACT"
  else
    echo "INTEG_META_CHANGED"
  fi
  cp /root/root-owned-probe.backup /var/lib/yorishiro/root-owned-probe.tmp
  chown root:root /var/lib/yorishiro/root-owned-probe.tmp
  chmod 0755 /var/lib/yorishiro/root-owned-probe.tmp
  mv -f /var/lib/yorishiro/root-owned-probe.tmp /var/lib/yorishiro/root-owned-probe
  chown root:yorishiro /var/lib/yorishiro/root-owned-probe
  restored_md5=$(md5sum /var/lib/yorishiro/root-owned-probe | cut -d" " -f1)
  orig_md5=$(cat /tmp/probe-md5.bak | cut -d" " -f1)
  restored_meta=$(stat -c "%U:%G %a" /var/lib/yorishiro/root-owned-probe)
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

# Construct an OpenAI-compatible provider without downloading a local model or contacting the endpoint.
docker exec "$APP" systemctl set-environment \
  YORISHIRO_EMBEDDING_BASE_URL=http://127.0.0.1:5151/v1 \
  YORISHIRO_EMBEDDING_MODEL=package-smoke-test

docker exec "$APP" systemctl reset-failed yorishiro >/dev/null 2>&1
docker exec "$APP" systemctl start yorishiro >/dev/null 2>&1
# Short timeout: no model fetch means a fast boot (~2-3 seconds).
for _ in $(seq 1 20); do
  docker exec "$APP" curl --noproxy localhost -fsS http://localhost:5150/_ping >/dev/null 2>&1 && break
  sleep 3
done

state=$(docker exec "$APP" bash -c '
  echo "active=$(systemctl is-active yorishiro)"
  echo "ping=$(curl --noproxy localhost -s -o /dev/null -w %{http_code} http://localhost:5150/_ping)"' 2>&1)

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
  -c "CREATE EXTENSION IF NOT EXISTS vector;" >/dev/null 2>&1

# Use an address because systemd-resolved cannot reach Docker's embedded DNS here.
PGIP=$(docker inspect "$PG" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')

docker exec "$APP" systemctl set-environment \
  DATABASE_URL="postgres://yorishiro:secret@$PGIP:5432/yorishiro" \
  HOST=http://127.0.0.1:5150 \
  DB_CONNECT_TIMEOUT=5000

# Restart to apply PostgreSQL settings, then wait for readiness before starting workers to avoid racing automigrations.
docker exec "$APP" bash -c '
  systemctl reset-failed yorishiro
  systemctl daemon-reload
  systemctl enable yorishiro yorishiro-worker yorishiro-jobs yorishiro-scheduler
  systemctl restart yorishiro' >/dev/null 2>&1

server_ready=
for _ in $(seq 1 120); do
  docker exec "$APP" curl --noproxy localhost -fsS http://localhost:5150/_ping >/dev/null 2>&1 \
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

if [ -z "${worker_skipped:-}" ]; then
  docker exec "$APP" bash -c '
    systemctl reset-failed yorishiro-jobs
    systemctl restart yorishiro-jobs' >/dev/null 2>&1
fi

if [ -z "${worker_skipped:-}" ]; then
  docker exec "$APP" bash -c '
    systemctl reset-failed yorishiro-scheduler
    systemctl restart yorishiro-scheduler' >/dev/null 2>&1
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
  echo "jobs-active=$(systemctl is-active yorishiro-jobs)"
  echo "scheduler-active=$(systemctl is-active yorishiro-scheduler)"
  echo "enabled=$(systemctl is-enabled yorishiro)"
  echo "worker-enabled=$(systemctl is-enabled yorishiro-worker)"
  echo "jobs-enabled=$(systemctl is-enabled yorishiro-jobs)"
  echo "scheduler-enabled=$(systemctl is-enabled yorishiro-scheduler)"
  echo "ping=$(curl --noproxy localhost -s -o /dev/null -w %{http_code} http://localhost:5150/_ping)"' 2>&1)

grep -qx 'active=active' <<<"$state" \
  && ok "the service is active" \
  || bad "expected active, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
if [ -n "${worker_skipped:-}" ]; then
  :
elif [ -n "${worker_assertion_recorded:-}" ]; then
  :
elif grep -qx 'worker-active=active' <<<"$state"; then
  ok "the worker is active"
else
  bad "expected worker active, got: $(grep -o 'worker-active=[a-z-]*' <<<"$state")"
  dump_logs
fi
if [ -z "${worker_skipped:-}" ]; then
  if grep -qx 'jobs-active=active' <<<"$state"; then
    ok "the application jobs consumer is active"
  else
    bad "expected application jobs consumer active, got: $(grep -o 'jobs-active=[a-z-]*' <<<"$state")"
    dump_logs
  fi
fi
if [ -z "${worker_skipped:-}" ] && grep -qx 'scheduler-active=active' <<<"$state"; then
  ok "the scheduler is active"
else
  [ -n "${worker_skipped:-}" ] || bad "expected scheduler active, got: $(grep -o 'scheduler-active=[a-z-]*' <<<"$state")"
fi
grep -qx 'enabled=enabled' <<<"$state" \
  && ok "the service is enabled" \
  || bad "expected enabled, got: $(grep -o 'enabled=[a-z-]*' <<<"$state")"
grep -qx 'worker-enabled=enabled' <<<"$state" \
  && ok "the worker is enabled" \
  || bad "expected worker enabled, got: $(grep -o 'worker-enabled=[a-z-]*' <<<"$state")"
grep -qx 'scheduler-enabled=enabled' <<<"$state" \
  && ok "the scheduler is enabled" \
  || bad "expected scheduler enabled, got: $(grep -o 'scheduler-enabled=[a-z-]*' <<<"$state")"
grep -qx 'jobs-enabled=enabled' <<<"$state" \
  && ok "the application jobs consumer is enabled" \
  || bad "expected application jobs consumer enabled, got: $(grep -o 'jobs-enabled=[a-z-]*' <<<"$state")"
grep -q 'ping=200' <<<"$state" \
  && ok "it answers /_ping" \
  || { bad "expected 200 from /_ping, got: $(grep -o 'ping=[0-9]*' <<<"$state")"; dump_logs; }

tables=$(docker exec "$PG" psql -U yorishiro -d yorishiro -Atc \
  "select count(*) from information_schema.tables where table_schema = 'public'" 2>/dev/null)
[ "${tables:-0}" -gt 0 ] 2>/dev/null \
  && ok "it applied its migrations to PostgreSQL" \
  || bad "PostgreSQL has no tables: the service is not running against it"

# Check the running process, not just the unit's User setting.
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
