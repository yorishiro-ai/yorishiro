#!/usr/bin/env bash
#
# Starts the service the way an operator does: `systemctl`, under systemd as PID 1.
#
# Separate from test-install.sh because this needs a privileged container, and the 30 checks
# there must stay runnable without one. What it covers is what `systemd-analyze verify` cannot:
# that a unit whose syntax is valid actually brings the service up.
#
# It found both of the defects it now guards. The units named a `config.example.yml` at a path
# dpkg deletes -- valid syntax, wrong address, and `systemd-analyze` reads directives and
# ignores comments. And an unconfigured start crashlooped every five seconds forever while
# `systemctl is-failed` answered `activating`, so a permanently broken service never reported
# failed to anything watching unit state.
#
#   ./packaging/test-systemd.sh <directory holding the .deb files>
#
# Needs docker with --privileged. ubuntu:24.04 and the deb only: the rpm under systemd is
# checked by hand, because putting an EOL Fedora's package repositories on the critical path of
# every pull request trades a real dependency for a marginal case.

set -uo pipefail

PKG_DIR="${1:?usage: test-systemd.sh <package directory>}"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }

# Both editions, because they are different binaries behind the same unit: the community
# build is headless and composes none of `ee/`, so "it starts under systemd" is a separate
# fact about each. Only the enterprise one was covered until this ran the pair.
run_edition() {
  EDITION="$1"
  DEB="$(basename "$(ls "$PKG_DIR"/yorishiro-"$EDITION"_*.deb | head -1)")"
  NET="ysr-sd-$EDITION-$$" APP="ysr-sd-app-$EDITION-$$" PG="ysr-sd-pg-$EDITION-$$"
  printf "\n######## %s ########\n" "yorishiro-$EDITION"

  # Torn down at the end of this call rather than on EXIT: the trap would fire once, after both
  # editions have run, leaving the first one's containers up for the length of the second.
  cleanup() {
    docker rm -f "$APP" "$PG" >/dev/null 2>&1
    docker network rm "$NET" >/dev/null 2>&1
  }
  trap cleanup EXIT INT TERM

  docker network create "$NET" >/dev/null 2>&1
  docker run -d --name "$PG" --network "$NET" \
    -e POSTGRES_USER=yorishiro -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=yorishiro \
    pgvector/pgvector:pg18 >/dev/null

  # systemd has to be PID 1, which is what `exec` at the end of the command is for, and needs
  # both tmpfs mounts plus a cgroup namespace it can write.
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
  note "an unconfigured start stops instead of retrying forever"
  # --------------------------------------------------------------------------------------------
  docker exec "$APP" bash -c "apt-get install -y -qq /pkg/$DEB >/dev/null 2>&1" || {
    echo "installing the package failed" >&2; exit 1
  }
  docker exec "$APP" systemctl start yorishiro >/dev/null 2>&1
  # Long enough that a five-second restart loop would have gone round twice.
  sleep 15

  state=$(docker exec "$APP" bash -c '
    echo "failed=$(systemctl is-failed yorishiro)"
    echo "status=$(systemctl show yorishiro -p ExecMainStatus --value)"
    echo "restarts=$(systemctl show yorishiro -p NRestarts --value)"' 2>&1)

  grep -q 'failed=failed' <<<"$state" \
    && ok "systemd reports it failed" \
    || bad "expected is-failed=failed, got: $(grep -o 'failed=[a-z-]*' <<<"$state")"
  # The number that makes the difference: 78 is what RestartPreventExitStatus matches on.
  grep -q 'status=78' <<<"$state" \
    && ok "the unit stopped on 78/CONFIG" \
    || bad "expected ExecMainStatus=78, got: $(grep -o 'status=[0-9]*' <<<"$state")"
  grep -q 'restarts=0' <<<"$state" \
    && ok "it did not retry" \
    || bad "expected 0 restarts, got: $(grep -o 'restarts=[0-9]*' <<<"$state")"

  journal=$(docker exec "$APP" journalctl -u yorishiro --no-pager -n 40 2>&1)
  grep -q '/etc/yorishiro/config.yml' <<<"$journal" \
    && ok "the journal names the file to edit" \
    || bad "the journal does not name the env file"

  # --------------------------------------------------------------------------------------------
  note "a configured service starts and serves"
  # --------------------------------------------------------------------------------------------
  docker exec "$PG" psql -U yorishiro -d yorishiro \
    -c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm;" >/dev/null 2>&1

  # By address, not by name. systemd-resolved takes over /etc/resolv.conf on boot and its stub
  # cannot reach Docker's embedded DNS from inside the container, so the container's own name
  # lookups stop working -- an artefact of running systemd in Docker, not something an operator
  # meets on a real host. /etc/hosts would work equally well; the address is simpler.
  PGIP=$(docker inspect "$PG" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
  # Written without leading whitespace: YAML gives indentation meaning, so the heredoc body
  # cannot be indented to match the surrounding shell the way the env file could be.
  docker exec "$APP" bash -c "cat >> /etc/yorishiro/config.yml <<EOF
database_url: postgres://yorishiro:secret@$PGIP:5432/yorishiro
bind: 0.0.0.0:8081
embedding:
  provider: openai
  base_url: http://localhost:1
  model: unused
EOF"

  docker exec "$APP" bash -c '
    systemctl reset-failed yorishiro
    systemctl daemon-reload
    systemctl enable --now yorishiro' >/dev/null 2>&1

  for _ in $(seq 1 60); do
    docker exec "$APP" curl -fsS http://127.0.0.1:8081/up >/dev/null 2>&1 && break
    sleep 3
  done

  state=$(docker exec "$APP" bash -c '
    echo "active=$(systemctl is-active yorishiro)"
    echo "enabled=$(systemctl is-enabled yorishiro)"
    echo "up=$(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:8081/up)"' 2>&1)

  grep -q 'active=active' <<<"$state" \
    && ok "systemctl reports it active" \
    || bad "expected is-active=active, got: $(grep -o 'active=[a-z-]*' <<<"$state")"
  grep -q 'up=200' <<<"$state" \
    && ok "it answers /up through the unit" \
    || bad "expected 200 from /up, got: $(grep -o 'up=[0-9]*' <<<"$state")"
  # `enable` is what makes it come back after a reboot, and is a separate claim from having
  # started once.
  grep -q 'enabled=enabled' <<<"$state" \
    && ok "it is enabled for the next boot" \
    || bad "expected is-enabled=enabled, got: $(grep -o 'enabled=[a-z-]*' <<<"$state")"

  # --------------------------------------------------------------------------------------------
  note "it comes back on reboot"
  # --------------------------------------------------------------------------------------------
  # `is-enabled` only proves the symlink exists. This proves multi-user.target actually pulls the
  # service in, which is the property an operator relies on and the one a wrong [Install] breaks.
  docker restart "$APP" >/dev/null 2>&1
  booted=
  for _ in $(seq 1 60); do
    case "$(docker exec "$APP" systemctl is-system-running 2>/dev/null)" in
      running|degraded) booted=1; break ;;
    esac
    sleep 3
  done
  if [ -z "$booted" ]; then
    bad "systemd did not come back after the restart"
  else
    for _ in $(seq 1 60); do
      docker exec "$APP" curl -fsS http://127.0.0.1:8081/up >/dev/null 2>&1 && break
      sleep 3
    done
    code=$(docker exec "$APP" curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8081/up 2>&1)
    [ "$code" = "200" ] \
      && ok "the service is serving again after a reboot, unattended" \
      || bad "after reboot /up returned $code"
  fi

  cleanup
}

run_edition ee
run_edition ce

# --------------------------------------------------------------------------------------------
# Switching editions keeps the database
# --------------------------------------------------------------------------------------------
# `run_edition` gives each edition its own Postgres, so it says nothing about the case an
# operator actually meets: the same database, before and after a package swap. That works only
# because one `migrations/` directory serves both binaries. The paid tables are in the initial
# migration, and the licence key rather than the schema is what gates the paid half. If that ever
# stopped being true, a swap would strand a deployment's data behind a migration the other
# edition does not have, and nothing here would have said so.
#
# Both directions, because they fail differently: ce to ee could apply something extra, and
# ee to ce could find something it cannot read.
swap_editions() {
  NET="ysr-swap-$$" APP="ysr-swap-app-$$" PG="ysr-swap-pg-$$"
  printf "\n######## edition swap, one database ########\n"

  cleanup_swap() {
    docker rm -f "$APP" "$PG" >/dev/null 2>&1
    docker network rm "$NET" >/dev/null 2>&1
  }
  trap cleanup_swap EXIT INT TERM

  docker network create "$NET" >/dev/null 2>&1
  docker run -d --name "$PG" --network "$NET" \
    -e POSTGRES_USER=yorishiro -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=yorishiro \
    pgvector/pgvector:pg18 >/dev/null
  docker run -d --name "$APP" --network "$NET" --privileged --cgroupns=host \
    --tmpfs /run --tmpfs /run/lock -v "$PKG_DIR":/pkg:ro ubuntu:24.04 \
    bash -c 'apt-get update -qq >/dev/null 2>&1
             apt-get install -y -qq systemd systemd-sysv curl >/dev/null 2>&1
             exec /lib/systemd/systemd' >/dev/null

  for _ in $(seq 1 60); do
    case "$(docker exec "$APP" systemctl is-system-running 2>/dev/null)" in
      running|degraded) break ;;
    esac
    sleep 3
  done
  for _ in $(seq 1 30); do
    docker exec "$PG" pg_isready -U yorishiro >/dev/null 2>&1 && break
    sleep 2
  done
  docker exec "$PG" psql -U yorishiro -d yorishiro \
    -c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm;" >/dev/null 2>&1

  PGIP=$(docker inspect "$PG" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')

  # The fingerprint that has to survive a swap: which migrations ran, and their checksums. A
  # count alone would miss an edition rewriting one in place.
  migration_state() {
    docker exec "$PG" psql -U yorishiro -d yorishiro -tAc \
      "SELECT count(*) || ':' || coalesce(md5(string_agg(version::text || encode(checksum, 'hex'), ',' ORDER BY version)), 'none') FROM _sqlx_migrations" 2>/dev/null | tr -d ' \r'
  }

  install_edition() {
    deb=$(basename "$(ls "$PKG_DIR"/yorishiro-"$1"_*.deb | head -1)")
    docker exec "$APP" bash -c "
      systemctl stop yorishiro >/dev/null 2>&1
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --allow-downgrades /pkg/$deb >/dev/null 2>&1" || return 1
    # Appended after the install so the first edition writes it and the second inherits it.
    # The same file surviving the swap is half of what is being tested.
    docker exec "$APP" bash -c "grep -q '^database_url:' /etc/yorishiro/config.yml || cat >> /etc/yorishiro/config.yml <<EOF
database_url: postgres://yorishiro:secret@$PGIP:5432/yorishiro
bind: 0.0.0.0:8081
embedding:
  provider: openai
  base_url: http://localhost:1
  model: unused
EOF"
    docker exec "$APP" bash -c '
      systemctl reset-failed yorishiro >/dev/null 2>&1
      systemctl daemon-reload
      systemctl enable --now yorishiro' >/dev/null 2>&1
    for _ in $(seq 1 60); do
      docker exec "$APP" curl -fsS http://127.0.0.1:8081/up >/dev/null 2>&1 && return 0
      sleep 3
    done
    return 1
  }

  install_edition ce \
    && ok "swap: ce starts on a fresh database" \
    || bad "swap: ce did not come up on a fresh database"
  first=$(migration_state)
  [ "${first%%:*}" -gt 0 ] 2>/dev/null \
    && ok "swap: ce applied its migrations ($first)" \
    || bad "swap: no migrations recorded after ce started ($first)"

  # A row only this deployment could have written, so "the data is still there" is a claim about
  # content rather than about the tables existing.
  docker exec "$PG" psql -U yorishiro -d yorishiro \
    -c "CREATE TABLE IF NOT EXISTS swap_marker (note text); INSERT INTO swap_marker VALUES ('written-under-ce');" >/dev/null 2>&1

  install_edition ee \
    && ok "swap: ee starts on the database ce created" \
    || bad "swap: ee did not come up on ce's database"
  after_ee=$(migration_state)
  [ "$after_ee" = "$first" ] \
    && ok "swap: ce -> ee left the migration set untouched" \
    || bad "swap: ce -> ee changed the migration set ($first -> $after_ee)"
  [ "$(docker exec "$PG" psql -U yorishiro -d yorishiro -tAc \
        "SELECT note FROM swap_marker" 2>/dev/null | tr -d ' \r')" = "written-under-ce" ] \
    && ok "swap: the data written under ce survived" \
    || bad "swap: the row written under ce is gone after switching to ee"

  docker exec "$PG" psql -U yorishiro -d yorishiro \
    -c "INSERT INTO swap_marker VALUES ('written-under-ee');" >/dev/null 2>&1

  install_edition ce \
    && ok "swap: ce starts again on the database ee wrote to" \
    || bad "swap: ce did not come back up after ee had run"
  after_ce=$(migration_state)
  [ "$after_ce" = "$first" ] \
    && ok "swap: ee -> ce left the migration set untouched" \
    || bad "swap: ee -> ce changed the migration set ($first -> $after_ce)"
  [ "$(docker exec "$PG" psql -U yorishiro -d yorishiro -tAc \
        "SELECT count(*) FROM swap_marker" 2>/dev/null | tr -d ' \r')" = "2" ] \
    && ok "swap: both editions' writes are readable from ce" \
    || bad "swap: ce cannot see both rows after the round trip"

  cleanup_swap
}

swap_editions

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
