#!/usr/bin/env bash
#
# Installs the packages in the distributions they claim to support, and checks what an operator
# actually gets. Every case here is a bug that shipped: a package that installed and then could
# not start, a licence file dpkg deleted on install, an unconfigured start that printed a Rust
# panic naming a source file.
#
# None of that is visible from the package contents -- all three passed inspection. The test is
# the install.
#
#   ./packaging/test-install.sh <directory holding the .deb and .rpm files>
#
# Needs docker. Runs the same matrix locally as in CI, so a failure can be reproduced without
# pushing.

set -uo pipefail

PKG_DIR="${1:?usage: test-install.sh <package directory>}"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The floor the packages declare. Read rather than hardcoded: this file must not be the place
# the two disagree.
GLIBC_FLOOR="$(grep -oE 'GLIBC_[0-9]+\.[0-9]+' "$REPO/packaging/nfpm-yorishiro.yaml" | sort -uV | tail -1)"

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }

deb() { ls "$PKG_DIR"/yorishiro_*.deb | head -1; }
deb_ce() { ls "$PKG_DIR"/yorishiro-ce_*.deb | head -1; }
rpm() { ls "$PKG_DIR"/yorishiro-[0-9]*.rpm | head -1; }

# --------------------------------------------------------------------------------------------
note "deb on ubuntu:24.04 — the supported case"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-server --help >/dev/null 2>&1 && echo "RUNS"
  getent passwd yorishiro >/dev/null && echo "USER"
  [ -f /usr/share/doc/yorishiro/copyright ] && echo "COPYRIGHT"
  [ -f /etc/yorishiro/config.example.yml ] && echo "EXAMPLE"
  [ "$(stat -c "%a %U:%G" /etc/yorishiro/yorishiro.env)" = "640 root:yorishiro" ] && echo "ENVPERM"
  [ "$(stat -c "%U" /var/lib/yorishiro)" = "yorishiro" ] && echo "STATEOWNER"
' 2>&1)
for want in RUNS USER COPYRIGHT EXAMPLE ENVPERM STATEOWNER; do
  case "$out" in
    *"$want"*) ok "$want" ;;
    *) bad "$want (install output: $(echo "$out" | tr '\n' ' '))" ;;
  esac
done

# --------------------------------------------------------------------------------------------
note "deb refused on ubuntu:22.04 — below the glibc floor"
# --------------------------------------------------------------------------------------------
# Asserting the reason, not merely a nonzero exit: a network failure also exits nonzero, and
# would otherwise read as the refusal working.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:22.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y /pkg/'"$(basename "$(deb)")"' 2>&1' 2>&1)
if grep -qiE 'depends|not installable|unmet' <<<"$out"; then
  ok "apt refuses, naming the dependency"
else
  bad "apt did not refuse for a dependency reason: $(echo "$out" | tail -3 | tr '\n' ' ')"
fi

# --------------------------------------------------------------------------------------------
note "rpm on fedora:39 — glibc $GLIBC_FLOOR exactly, the tightest supported case"
# --------------------------------------------------------------------------------------------
# The rpm's *positive* path had no coverage at all: it was only ever seen refusing on Rocky 9.
#
# fedora:39 specifically, because its glibc is 2.38 -- the floor exactly, so this is the
# tightest system the packages claim to support. Replacing it needs a distro at whatever the
# floor then is, not merely a newer Fedora, or the boundary stops being tested. The image is
# EOL and its repositories may be gone; installing a local rpm whose dependencies are already
# satisfied does not need them, which is why there is no `dnf update` here.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro fedora:39 bash -c '
  dnf install -y -q /pkg/'"$(basename "$(rpm)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-server --help >/dev/null 2>&1 && echo "RUNS"
  [ -f /usr/share/doc/yorishiro/copyright ] && echo "COPYRIGHT"
  getent passwd yorishiro >/dev/null && echo "USER"
' 2>&1)
for want in RUNS COPYRIGHT USER; do
  case "$out" in
    *"$want"*) ok "fedora:39 $want" ;;
    *) bad "fedora:39 $want (output: $(echo "$out" | tr '\n' ' '))" ;;
  esac
done

# --------------------------------------------------------------------------------------------
note "rpm refused on rockylinux:9 — below the floor"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro rockylinux:9 bash -c '
  dnf install -y /pkg/'"$(basename "$(rpm)")"' 2>&1' 2>&1)
if grep -q "nothing provides libc.so.6($GLIBC_FLOOR)" <<<"$out"; then
  ok "dnf refuses, naming the missing glibc symbol"
else
  bad "dnf did not refuse for the glibc reason: $(echo "$out" | tail -3 | tr '\n' ' ')"
fi

# --------------------------------------------------------------------------------------------
note "an unconfigured start says what to do"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  su -s /bin/sh yorishiro -c "/usr/bin/yorishiro-server" 2>&1' 2>&1)
if grep -q '/etc/yorishiro/yorishiro.env' <<<"$out"; then
  ok "names the file to edit"
else
  bad "does not name the env file: $(echo "$out" | head -2 | tr '\n' ' ')"
fi
if grep -qE 'panicked at|RUST_BACKTRACE' <<<"$out"; then
  bad "still prints a Rust panic at an operator"
else
  ok "no panic, no source path"
fi

# --------------------------------------------------------------------------------------------
note "the community package is the community package"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb_ce)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-ce-server --help >/dev/null 2>&1 && echo "RUNS"
  [ -f /usr/share/doc/yorishiro-ce/copyright ] && echo "COPYRIGHT"
  [ -f /usr/share/doc/yorishiro-ce/copyright.ee ] && echo "HAS_EE_LICENCE"
  dpkg -s yorishiro-ce 2>/dev/null | grep -qi "^Conflicts: yorishiro" && echo "CONFLICTS"
' 2>&1)
for want in RUNS COPYRIGHT CONFLICTS; do
  case "$out" in
    *"$want"*) ok "ce $want" ;;
    *) bad "ce $want (output: $(echo "$out" | tr '\n' ' '))" ;;
  esac
done
# The edition boundary at the package layer: shipping the paid licence here would mean paid
# material is on a machine that installed this package precisely to avoid it.
case "$out" in
  *HAS_EE_LICENCE*) bad "ce ships the paid licence" ;;
  *) ok "ce does not ship the paid licence" ;;
esac

# --------------------------------------------------------------------------------------------
note "install → configure → first-run wizard, against a real database"
# --------------------------------------------------------------------------------------------
NET=yorishiro-pkgtest-$$
docker network create "$NET" >/dev/null 2>&1
docker rm -f "pg-$$" "app-$$" >/dev/null 2>&1
# A private network and no published ports: this runs beside CI's own service containers.
docker run -d --name "pg-$$" --network "$NET" \
  -e POSTGRES_USER=yorishiro -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=yorishiro \
  pgvector/pgvector:pg18 >/dev/null

ready=
for _ in $(seq 1 60); do
  # `pg_isready` answers before initdb has created the database, so ask for the database.
  docker exec "pg-$$" psql -U yorishiro -d yorishiro -c 'select 1' >/dev/null 2>&1 && { ready=1; break; }
  sleep 1
done
if [ -z "$ready" ]; then
  bad "postgres never became ready"
else
  docker exec "pg-$$" psql -U yorishiro -d yorishiro \
    -c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm;" >/dev/null 2>&1

  docker run -d --name "app-$$" --network "$NET" -v "$PKG_DIR":/pkg:ro ubuntu:24.04 sleep infinity >/dev/null
  docker exec "app-$$" bash -c "
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y -qq /pkg/$(basename "$(deb)") curl >/dev/null 2>&1
    cat >> /etc/yorishiro/yorishiro.env <<EOF
DATABASE_URL=postgres://yorishiro:secret@pg-$$:5432/yorishiro
YORISHIRO_BIND=0.0.0.0:8081
YORISHIRO_EMBEDDING_PROVIDER=openai
YORISHIRO_EMBEDDING_BASE_URL=http://localhost:1
YORISHIRO_EMBEDDING_MODEL=unused
EOF
  " >/dev/null 2>&1
  docker exec -d "app-$$" bash -c \
    'set -a; . /etc/yorishiro/yorishiro.env; set +a; exec su -s /bin/sh yorishiro -c /usr/bin/yorishiro-server'

  up=
  for _ in $(seq 1 60); do
    docker exec "app-$$" curl -fsS http://127.0.0.1:8081/up >/dev/null 2>&1 && { up=1; break; }
    sleep 1
  done

  if [ -z "$up" ]; then
    bad "configured server never answered /up"
  else
    ok "starts and applies its migrations"
    grep -q 'true' <<<"$(docker exec "app-$$" curl -s http://127.0.0.1:8081/setup/status)" \
      && ok "the first-run wizard is offered" || bad "setup_required was not true"

    code=$(docker exec "app-$$" curl -s -o /dev/null -w '%{http_code}' \
      -X POST http://127.0.0.1:8081/setup -H 'Content-Type: application/json' \
      -d '{"email":"admin@example.com","password":"correct-horse-battery"}')
    [ "$code" = "201" ] && ok "the wizard creates the deployment" || bad "wizard POST returned $code"

    code=$(docker exec "app-$$" curl -s -o /dev/null -w '%{http_code}' \
      -X POST http://127.0.0.1:8081/setup -H 'Content-Type: application/json' \
      -d '{"email":"second@example.com","password":"another-password-x"}')
    [ "$code" = "409" ] && ok "a second run is refused" || bad "second wizard POST returned $code"
  fi
fi
docker rm -f "pg-$$" "app-$$" >/dev/null 2>&1
docker network rm "$NET" >/dev/null 2>&1

# --------------------------------------------------------------------------------------------
note "the systemd units are valid"
# --------------------------------------------------------------------------------------------
# Each unit is verified inside its own package's container. `systemd-analyze` resolves
# ExecStart, so a unit checked without its binary reports a missing command -- a failure about
# the test environment rather than about the unit. The two packages conflict, so this is also
# the only way to have each binary present for its own unit.
verify_unit() {
  pkg="$1" unit="$2"
  out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y -qq systemd /pkg/'"$pkg"' >/dev/null 2>&1
    systemd-analyze verify /lib/systemd/system/'"$unit"' 2>&1' 2>&1)
  if [ -z "$(echo "$out" | grep -v '^$')" ]; then
    ok "systemd-analyze verify is silent on $unit"
  else
    bad "systemd-analyze verify complained about $unit: $(echo "$out" | head -3 | tr '\n' ' ')"
  fi
}
verify_unit "$(basename "$(deb)")" yorishiro.service
verify_unit "$(basename "$(deb_ce)")" yorishiro-ce.service

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
