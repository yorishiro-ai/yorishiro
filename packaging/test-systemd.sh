#!/usr/bin/env bash
#
# Starts the service the way an operator does: `systemctl`, under systemd as PID 1.
#
# Separate from test-install.sh because this needs a privileged container, and the 30 checks
# there must stay runnable without one. What it covers is what `systemd-analyze verify` cannot:
# that a unit whose syntax is valid actually brings the service up.
#
# It guards two defects. A unit naming `config.example.yml` at a path dpkg deletes has valid
# syntax and a wrong address, which `systemd-analyze` cannot see: it reads directives and
# ignores comments. And an unconfigured start can crashloop every five seconds forever while
# `systemctl is-failed` answers `activating`, so a permanently broken service never reports
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
  # lookups stop working: an artefact of running systemd in Docker, not something an operator
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

  # Entities through the running service's own REST API, not rows through psql. A marker row
  # written and read by psql proves the database survived the swap; it does not prove either
  # edition can read what the other wrote, which is the claim being made. Going through the API
  # exercises the migrator, the schema, the row's JSONB shape and the deserializer in the binary
  # that is actually installed.
  api() {
    docker exec "$APP" curl -s -o /dev/null -w '%{http_code}' \
      -H "Authorization: Bearer $KEY" "$@" 2>/dev/null
  }
  write_entity() {
    docker exec "$APP" curl -s -o /dev/null -w '%{http_code}' \
      -X POST http://127.0.0.1:8081/api/entities \
      -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
      -d "{\"schema_name\":\"task-management\",\"entity_type\":\"task\",\"data\":{\"title\":\"$1\",\"done\":false,\"priority\":\"medium\"}}" 2>/dev/null
  }
  entity_titles() {
    docker exec "$APP" curl -s -H "Authorization: Bearer $KEY" \
      http://127.0.0.1:8081/api/entities 2>/dev/null \
      | grep -o '"title":"[^"]*"' | sed 's/"title":"//;s/"$//' | sort | tr '\n' ' '
  }

  if install_edition ce; then
    ok "swap: ce starts on a fresh database"
  else
    bad "swap: ce did not come up on a fresh database"
  fi
  first=$(migration_state)
  if [ "${first%%:*}" -gt 0 ] 2>/dev/null; then
    ok "swap: ce applied its migrations ($first)"
  else
    bad "swap: no migrations recorded after ce started ($first)"
  fi

  # Provisioned through the community binary's own admin CLI, which is also the only way in on
  # ce: it is headless, so there is no setup wizard to click through.
  #
  # The workspace id comes out of `create-tenant` itself, from the two lines it prints under
  # "default workspace created". Reading it back from `list-tenants` instead returns the TENANT
  # id, which `create-api-key` rejects, and the failure surfaces three assertions later as an
  # empty entity list rather than as a bad id.
  # YORISHIRO_CONFIG_PATH is passed explicitly: the unit exports it, but `docker exec` starts a
  # shell that has none of the unit's environment, so the CLI would otherwise look for a
  # config.yml in the working directory and report the database as unconfigured. Naming the file
  # is what an operator does too, and it keeps this test independent of the fallback.
  admin() {
    docker exec -e YORISHIRO_CONFIG_PATH=/etc/yorishiro/config.yml "$APP" \
      /usr/bin/yorishiro-server admin "$@" 2>&1
  }
  provision=$(admin create-tenant swap --template task-management)
  WS=$(sed -n '/default workspace created/,$p' <<<"$provision" \
    | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1)
  if [ -n "$WS" ]; then
    ok "swap: ce provisioned a tenant and workspace"
  else
    bad "swap: no workspace id in create-tenant's output: $(tr '\n' ' ' <<<"$provision" | tail -c 200)"
  fi
  KEY=$(admin create-api-key "$WS" schema | grep -oE 'ysr_[A-Za-z0-9_]+' | head -1)
  if [ -n "$KEY" ]; then
    ok "swap: provisioned a workspace and key through ce's admin CLI"
  else
    bad "swap: could not issue an API key from ce (workspace='$WS')"
  fi

  if [ "$(write_entity written-under-ce)" = "201" ]; then
    ok "swap: ce wrote an entity through its own API"
  else
    bad "swap: ce could not write an entity"
  fi

  if install_edition ee; then
    ok "swap: ee starts on the database ce created"
  else
    bad "swap: ee did not come up on ce's database"
  fi
  after_ee=$(migration_state)
  if [ "$after_ee" = "$first" ]; then
    ok "swap: ce to ee left the migration set untouched"
  else
    bad "swap: ce to ee changed the migration set ($first -> $after_ee)"
  fi
  if [ "$(entity_titles)" = "written-under-ce " ]; then
    ok "swap: ee reads the entity ce wrote"
  else
    bad "swap: ee does not read ce's entity (got '$(entity_titles)')"
  fi
  if [ "$(write_entity written-under-ee)" = "201" ]; then
    ok "swap: ee wrote an entity of its own"
  else
    bad "swap: ee could not write an entity"
  fi

  if install_edition ce; then
    ok "swap: ce starts again on the database ee wrote to"
  else
    bad "swap: ce did not come back up after ee had run"
  fi
  after_ce=$(migration_state)
  if [ "$after_ce" = "$first" ]; then
    ok "swap: ee to ce left the migration set untouched"
  else
    bad "swap: ee to ce changed the migration set ($first -> $after_ce)"
  fi
  if [ "$(entity_titles)" = "written-under-ce written-under-ee " ]; then
    ok "swap: ce reads both editions' entities"
  else
    bad "swap: ce does not read both entities (got '$(entity_titles)')"
  fi
  # The key ce issued still authenticates after two swaps, so the swap does not invalidate
  # credentials an operator already handed out.
  if [ "$(api http://127.0.0.1:8081/whoami)" = "200" ]; then
    ok "swap: the API key ce issued still works after the round trip"
  else
    bad "swap: the API key stopped working across the swap"
  fi

  cleanup_swap
}

swap_editions

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
