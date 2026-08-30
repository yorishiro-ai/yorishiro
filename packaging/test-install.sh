#!/usr/bin/env bash
#
# Installs the package in the distributions it claims to support, and checks what an operator
# actually gets: a package that installs and then cannot start, a licence file dpkg deletes on
# install, a start that says nothing useful about what is missing.
#
# None of that is visible from the package contents: a package passes inspection and still fails
# the install. The test is the install.
#
#   ./packaging/test-install.sh <directory holding the .deb and .rpm files>
#
# Needs docker. Runs the same matrix locally as in CI, so a failure can be reproduced without
# pushing.
#
# There is one package now. An earlier version of this file tested `yorishiro-ce` alongside
# `yorishiro-ee`: it installed the community deb and rpm, asserted `Conflicts: yorishiro-ee`,
# asserted the paid licence was absent from it, scanned the installed community binary for paid
# markers (`hosted/stripe`, `api/marketplace`, `infer-fill`), and switched a machine between the
# two editions in both directions. All of that is gone because the community package is gone:
# `ee/` compiles into the single binary and the licence layer decides at runtime what serves, so
# there is no second artifact to install and no on-disk edition boundary to assert. The marker
# scan in particular could only ever fail now, since every marker it looked for is in this
# binary by design. `packaging/nfpm-yorishiro.yaml` records the same reasoning.

set -uo pipefail

PKG_DIR="${1:?usage: test-install.sh <package directory>}"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The floor the package declares. Read rather than hardcoded: this file must not be the place
# the two disagree.
# Scoped to the rpm depends line specifically (`libc.so.6(GLIBC_X.Y)(64bit)`), not the whole
# file: nfpm-yorishiro.yaml's own comments discuss other glibc versions by number (a locally
# measured floor that was rejected, historical figures), and a bare whole-file grep picks up
# whichever of those sorts highest, which is not necessarily the one actually declared below.
GLIBC_FLOOR="$(grep -oE 'libc\.so\.6\(GLIBC_[0-9]+\.[0-9]+\)' "$REPO/packaging/nfpm-yorishiro.yaml" \
  | grep -oE 'GLIBC_[0-9]+\.[0-9]+')"

# Checked up front rather than at the call site: a missing tool otherwise surfaces as the
# assertion it happens to sit in, which reads as a packaging fault.
for tool in docker jq; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }

deb() { ls "$PKG_DIR"/yorishiro_*.deb | head -1; }
rpm() { ls "$PKG_DIR"/yorishiro-[0-9]*.rpm | head -1; }

# --------------------------------------------------------------------------------------------
note "deb on ubuntu:24.04 — the supported case"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro --help >/dev/null 2>&1 && echo "RUNS"
  getent passwd yorishiro >/dev/null && echo "USER"
  [ -f /usr/share/doc/yorishiro/copyright ] && echo "COPYRIGHT"
  [ -f /etc/yorishiro/LICENSE.enterprise ] && echo "EE_LICENCE"
  [ -f /usr/share/yorishiro/config/production.yaml ] && echo "CONFIG"
  [ -f /usr/share/yorishiro/docs/configuration.md ] && echo "DOCS"
  [ "$(stat -c "%a %U:%G" /etc/yorishiro/yorishiro.env)" = "640 root:yorishiro" ] && echo "ENVPERM"
  [ "$(stat -c "%U" /var/lib/yorishiro)" = "yorishiro" ] && echo "STATEOWNER"
' 2>&1)
for want in RUNS USER COPYRIGHT EE_LICENCE CONFIG DOCS ENVPERM STATEOWNER; do
  case "$out" in
    *"$want"*) ok "$want" ;;
    *) bad "$want (install output: $(echo "$out" | tr '\n' ' '))" ;;
  esac
done

# --------------------------------------------------------------------------------------------
note "every file the package declares survives the install"
# --------------------------------------------------------------------------------------------
# The list above names files one at a time, which only ever catches what someone thought to
# add. `dpkg-deb -c` versus the installed filesystem catches the general case: dpkg's
# `path-exclude=/usr/share/doc/*` deletes at unpack, silently, so a package can contain a file
# it does not deliver.
#
# `ee/LICENSE` shipping as `/usr/share/doc/yorishiro/copyright.ee` is exactly that case: the
# `path-include` covers the name `copyright` exactly, so the package would deliver everything
# except one of the licences it is distributed under, on every minimal image.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  set -euo pipefail
  apt-get update -qq >/dev/null 2>&1
  dpkg-deb -c /pkg/'"$(basename "$(deb)")"' | awk "\$6 ~ /^\.\// && \$1 !~ /^d/ { print substr(\$6, 2) }" > /tmp/declared
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  while read -r f; do
    [ -e "$f" ] || echo "DROPPED:$f"
  done < /tmp/declared
  echo "COMPARED:$(wc -l < /tmp/declared)"' 2>&1)
if ! grep -q COMPARED: <<<"$out"; then
  bad "could not compare the package against the install: $(echo "$out" | tr '\n' ' ')"
elif grep -q DROPPED: <<<"$out"; then
  bad "files in the package are missing after install: $(grep -o 'DROPPED:[^ ]*' <<<"$out" | tr '\n' ' ')"
else
  ok "all $(grep -o 'COMPARED:[0-9]*' <<<"$out" | cut -d: -f2) declared files are present"
fi

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
note "rpm on almalinux:10 — the current RPM-family release this supports"
# --------------------------------------------------------------------------------------------
# This project supports the current LTS releases, which on the RPM side is AlmaLinux 10: glibc
# 2.39, measured, against a declared floor of 2.39. There is no GLIBCXX floor to check against:
# the candle-based embedding provider links no C++ standard library dynamically.
#
# It replaced fedora:39, which was here because its glibc was 2.38, the floor as declared at the
# time. That floor was wrong (the binary needs 2.39), so fedora:39 was a system this package
# installed on and could not start on, and it is EOL besides.
#
# This is the only block that installs an rpm at all: the others install its counterpart deb. So
# it is not a second opinion on the deb tests, it is the only evidence that the other half of
# what a release publishes can be unpacked and run.
# The image is pulled first, and its progress goes to the terminal rather than into `$out`.
# Docker writes that to stderr, and folding it in with `2>&1` below puts a paragraph of layer
# names in front of every assertion's failure message, which is what this looked like the first
# time it failed for a real reason.
docker pull -q almalinux:10 >/dev/null 2>&1 || true
# The exit code is captured into a variable and always printed, rather than being consumed by an
# `if`. When this first failed, `RUNS` was simply absent: no marker said whether the binary had
# been run at all, and an `if` with an `echo` in both branches would have been just as silent if
# the command never reached either. A line that is always emitted cannot fail to appear.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro almalinux:10 bash -c '
  dnf install -y -q /pkg/'"$(basename "$(rpm)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro --help >/dev/null 2>/tmp/help.err
  rc=$?
  echo "HELP_EXIT:$rc"
  [ "$rc" = 0 ] && echo "RUNS"
  [ -s /tmp/help.err ] && echo "HELP_STDERR: $(head -c 300 /tmp/help.err | tr "\n" " ")"
  [ -f /usr/share/doc/yorishiro/copyright ] && echo "COPYRIGHT"
  getent passwd yorishiro >/dev/null && echo "USER"
' 2>&1)
# Reported separately from the marker loop below, so a missing `RUNS` always comes with the
# reason next to it rather than only in the raw output dump.
case "$out" in
  *HELP_EXIT:0*) ;;
  *HELP_EXIT:*) bad "almalinux:10 --help exited $(sed -n 's/.*HELP_EXIT:\([0-9]*\).*/\1/p' <<<"$out")" ;;
  *) bad "almalinux:10 --help never reported an exit code: $(echo "$out" | tr '\n' ' ')" ;;
esac
for want in RUNS COPYRIGHT USER; do
  case "$out" in
    *"$want"*) ok "almalinux:10 $want" ;;
    *) bad "almalinux:10 $want (output: $(echo "$out" | tr '\n' ' '))" ;;
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
note "an unconfigured start fails on the thing that is missing"
# --------------------------------------------------------------------------------------------
# There is no exit-code assertion here. An earlier version required exit 78 (EX_CONFIG) for a
# missing DATABASE_URL and exit 1 for an unreachable one, because the unit used
# `RestartPreventExitStatus=78` to tell a permanent misconfiguration from a database that is
# merely slow to come up. This binary does not produce 78: it delegates to `loco_rs::cli::main`,
# and nothing in `src/` returns that code, so the unit no longer carries the directive either
# (see `packaging/yorishiro.service`). Asserting a number nothing emits would fail every run.
#
# What is still worth asserting is that the failure names the variable an operator has to set,
# and does not hand them a Rust panic.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  cd /var/lib/yorishiro
  su -s /bin/sh yorishiro -c "LOCO_ENV=production LOCO_CONFIG_FOLDER=/usr/share/yorishiro/config /usr/bin/yorishiro start" 2>&1
  echo "EXIT:$?"' 2>&1)
if grep -q 'EXIT:0' <<<"$out"; then
  bad "an unconfigured start exited 0: $(echo "$out" | tail -3 | tr '\n' ' ')"
else
  ok "an unconfigured start fails rather than starting"
fi
# Whichever of the three it stops on, not `DATABASE_URL` specifically. The config file is a
# Tera template rendered top to bottom, so the first unset variable is the one reported, and
# `host:` sits above `database:` in `config/production.yaml`: with nothing set at all this fails
# on HOST. Measured, after asserting DATABASE_URL here and watching it not appear.
if grep -qE 'HOST|DATABASE_URL|QUEUE_URL' <<<"$out"; then
  ok "names the variable it stopped on"
else
  bad "names none of HOST/DATABASE_URL/QUEUE_URL: $(echo "$out" | head -3 | tr '\n' ' ')"
fi
# `panicked at` only. `RUST_BACKTRACE` was in this pattern and had to come out: a config error
# prints the whole rendered template, and `config/production.yaml` mentions that variable in a
# comment about `pretty_backtrace`, so the pattern matched the configuration file rather than a
# panic and reported a failure on a run that behaved correctly.
if grep -q 'panicked at' <<<"$out"; then
  bad "still prints a Rust panic at an operator"
else
  ok "no panic"
fi

# --------------------------------------------------------------------------------------------
note "the systemd unit is valid"
# --------------------------------------------------------------------------------------------
# `systemd-analyze` resolves ExecStart, so a unit checked without its binary reports a missing
# command: a failure about the test environment rather than about the unit.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq systemd /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  systemd-analyze verify /lib/systemd/system/yorishiro.service 2>&1' 2>&1)
if [ -z "$(echo "$out" | grep -v '^$')" ]; then
  ok "systemd-analyze verify is silent on yorishiro.service"
else
  bad "systemd-analyze verify complained: $(echo "$out" | head -3 | tr '\n' ' ')"
fi

# Every absolute path, not a list of prefixes: a unit naming `/opt/...` or `/run/...` would
# otherwise be scanned and reported complete without that path having been looked at.
# `systemd-analyze` reads directives and ignores comments, so a path named only in a comment is
# unverified by it: an operator following the unit's own instructions would find nothing there
# if that path were wrong. A path in a comment is documentation the package ships, so it is
# checked like the rest of the package.
#
# Two exclusions, each because the path is not a file the package ships: systemd's own
# `WantedBy=` targets (`multi-user.target` is a unit name, and appears without a directory), and
# anything under `/proc` or `/sys`.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  set -euo pipefail
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  unit=/lib/systemd/system/yorishiro.service
  [ -f "$unit" ] || { echo "NO_UNIT"; exit 1; }
  grep -oE "(^|[[:space:]=\"])/[A-Za-z0-9._/-]+" "$unit" \
    | sed -e "s/^[[:space:]=\"]//" -e "s/[.,]$//" \
    | grep -vE "^/(proc|sys)(/|$)" \
    | sort -u | while read -r p; do
      [ -e "$p" ] || echo "MISSING:$p"
    done
  echo CHECKED' 2>&1)
if ! grep -q CHECKED <<<"$out"; then
  bad "could not check the paths the unit names: $(echo "$out" | tr '\n' ' ')"
elif grep -q MISSING: <<<"$out"; then
  bad "the unit names paths the package does not install: $(grep -o 'MISSING:[^ ]*' <<<"$out" | tr '\n' ' ')"
else
  ok "every path the unit names exists after install"
fi

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
QUEUE_URL=postgres://yorishiro:secret@pg-$$:5432/yorishiro
HOST=http://127.0.0.1:5150
YORISHIRO_EMBEDDING_BASE_URL=http://localhost:1
YORISHIRO_EMBEDDING_MODEL=unused
EOF
  " >/dev/null 2>&1
  # Started the way the unit does, since there is no systemd here: the same environment file,
  # the same LOCO_ENV and config folder.
  docker exec -d "app-$$" bash -c \
    'cd /var/lib/yorishiro && set -a && . /etc/yorishiro/yorishiro.env && set +a &&
     exec su -s /bin/sh yorishiro -c "env LOCO_ENV=production LOCO_CONFIG_FOLDER=/usr/share/yorishiro/config \
       DATABASE_URL=$DATABASE_URL QUEUE_URL=$QUEUE_URL HOST=$HOST \
       YORISHIRO_EMBEDDING_BASE_URL=$YORISHIRO_EMBEDDING_BASE_URL \
       YORISHIRO_EMBEDDING_MODEL=$YORISHIRO_EMBEDDING_MODEL /usr/bin/yorishiro start"'

  up=
  for _ in $(seq 1 60); do
    docker exec "app-$$" curl -fsS http://127.0.0.1:5150/_ping >/dev/null 2>&1 && { up=1; break; }
    sleep 1
  done

  if [ -z "$up" ]; then
    bad "configured server never answered /_ping"
  else
    ok "starts and applies its migrations"

    # The field, not the word: `grep true` would also pass on
    # `{"setup_required":false,"something_else":true}`, which is the opposite of what this
    # asserts. `jq` rather than a pattern, so adding a field to the response cannot quietly
    # turn this green.
    status=$(docker exec "app-$$" curl -s http://127.0.0.1:5150/setup/status)
    if [ "$(jq -r '.setup_required' <<<"$status" 2>/dev/null)" = "true" ]; then
      ok "the first-run wizard is offered"
    else
      bad "setup_required was not true: $status"
    fi

    code=$(docker exec "app-$$" curl -s -o /dev/null -w '%{http_code}' \
      -X POST http://127.0.0.1:5150/setup -H 'Content-Type: application/json' \
      -d '{"email":"admin@example.com","password":"correct-horse-battery"}')
    [ "$code" = "201" ] && ok "the wizard creates the deployment" || bad "wizard POST returned $code"

    code=$(docker exec "app-$$" curl -s -o /dev/null -w '%{http_code}' \
      -X POST http://127.0.0.1:5150/setup -H 'Content-Type: application/json' \
      -d '{"email":"second@example.com","password":"another-password-x"}')
    [ "$code" = "409" ] && ok "a second run is refused" || bad "second wizard POST returned $code"
  fi
fi
docker rm -f "pg-$$" "app-$$" >/dev/null 2>&1
docker network rm "$NET" >/dev/null 2>&1

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
