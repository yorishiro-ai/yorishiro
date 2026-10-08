#!/usr/bin/env bash
#
# Installs the package in the distributions it claims to support, and checks what an operator
# actually gets: a package that installs and then cannot start, a licence file dpkg deletes on
# install, a start that says nothing useful about what is missing.
#
# None of that is visible from the package contents: a package passes inspection and still fails
# the install. The test is the install.
#
#   ./packaging/test-install.sh <exact package file>
#
# Needs docker. Runs the same matrix locally as in CI, so a failure can be reproduced without
# pushing.
#
# CE and EE packages are built separately; this script checks the CE package path.

set -uo pipefail

PKG_FILE="${1:?usage: test-install.sh <exact package file>}"
PKG_FILE="$(cd "$(dirname "$PKG_FILE")" && pwd)/$(basename "$PKG_FILE")"
PKG_DIR="$(dirname "$PKG_FILE")"
case "$(basename "$PKG_FILE")" in
  yorishiro-ce-*-amd64.deb|yorishiro-ee-*-amd64.deb|yorishiro-ce-*-arm64.deb|yorishiro-ee-*-arm64.deb) ;;
  *) echo "package filename is not edition-first: $(basename "$PKG_FILE")" >&2; exit 2 ;;
esac
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The floor the package declares. Read rather than hardcoded: this file must not be the place
# the two disagree.
# Scoped to the rpm depends line specifically (`libc.so.6(GLIBC_X.Y)(64bit)`), not the whole
# file: the nfpm package config's own comments discuss other glibc versions by number (a locally
# measured floor that was rejected, historical figures), and a bare whole-file grep picks up
# whichever of those sorts highest, which is not necessarily the one actually declared below.
GLIBC_FLOOR="$(grep -oE 'libc\.so\.6\(GLIBC_[0-9]+\.[0-9]+\)' "$REPO/packaging/nfpm-yorishiro-ce.yaml" \
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

deb() {
  case "$PKG_FILE" in
    *.deb) printf '%s\n' "$PKG_FILE" ;;
    *.rpm) printf '%s/%s.deb\n' "$PKG_DIR" "$(basename "${PKG_FILE%.rpm}")" ;;
  esac
}
rpm() {
  case "$PKG_FILE" in
    *.rpm) printf '%s\n' "$PKG_FILE" ;;
    *.deb) printf '%s/%s.rpm\n' "$PKG_DIR" "$(basename "${PKG_FILE%.deb}")" ;;
  esac
}
edition() { basename "$PKG_FILE" | grep -q '^yorishiro-ee-' && echo ee || echo ce; }

# --------------------------------------------------------------------------------------------
note "deb on ubuntu:24.04 — the supported case"
# --------------------------------------------------------------------------------------------
# Single container: install, root backup/metadata, unprivileged probes as yorishiro,
# root verify/atomic restore, then emit all markers to the host for assertion.
probe=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  set -euo pipefail
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  # Root-owned property checks.
  /usr/bin/yorishiro --help >/dev/null 2>&1 && echo "RUNS"
  getent passwd yorishiro >/dev/null && echo "USER"
  [ -f /usr/share/doc/yorishiro/copyright ] && echo "COPYRIGHT"
  [ -f /etc/yorishiro/LICENSE.enterprise ] && echo "EE_LICENCE"
  [ -f /etc/yorishiro/production.yaml ] && echo "CONFIG"
  [ "$(stat -c "%a %U:%G" /etc/yorishiro/production.yaml)" = "640 root:yorishiro" ] && echo "CONFIGPERM"
  [ "$(stat -c "%U" /var/lib/yorishiro)" = "root" ] && echo "STATEOWNER"
  [ "$(stat -c "%a" /var/lib/yorishiro)" = "1770" ] && echo "STATEPERMS"
  [ "$(stat -c "%G" /var/lib/yorishiro)" = "yorishiro" ] && echo "STATEGRP"
  [ "$(stat -c "%U" /var/lib/yorishiro/worker-wrapper.sh)" = "root" ] && echo "WRAPPEROWNER"
  [ "$(stat -c "%a" /var/lib/yorishiro/worker-wrapper.sh)" = "755" ] && echo "WRAPPERPERMS"
  # Root records wrapper metadata and creates a protected backup.
  stat -c "%U:%G %a" /var/lib/yorishiro/worker-wrapper.sh > /tmp/wrapper-meta.bak
  md5sum /var/lib/yorishiro/worker-wrapper.sh > /tmp/wrapper-md5.bak
  cp /var/lib/yorishiro/worker-wrapper.sh /root/worker-wrapper.backup
  # Unprivileged probes as the yorishiro user.
  su -s /bin/sh yorishiro -c '"'"'
    # State-file creation must succeed (directory is group-writable).
    touch /var/lib/yorishiro/writable-as-yorishiro && echo "STATEWRITE" || echo "STATEWRITEFAIL"
    # rm test: wrapper is root-owned; yorishiro cannot delete it.
    rm -f /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null || true
    if [ -f /var/lib/yorishiro/worker-wrapper.sh ]; then
      echo "WRAPPER_UNRMED_BY_YORISHIRO"
    else
      echo "WRAPPER_UNEXPECTEDLY_GONE"
    fi
    # Sticky-rename test: mv -f (force) as yorishiro over root-owned wrapper.
    # The sticky bit on the directory blocks the rename because yorishiro does
    # not own the target file.
    touch /var/lib/yorishiro/unpriv-tempfile
    mv -f /var/lib/yorishiro/unpriv-tempfile /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null || true
    if [ -f /var/lib/yorishiro/worker-wrapper.sh ] && [ "$(stat -c %U /var/lib/yorishiro/worker-wrapper.sh)" = "root" ]; then
      echo "WRAPPER_INTACT_AFTER_MV"
    else
      echo "WRAPPER_CORRUPTED_AFTER_MV"
    fi
    # Shell redirection overwrite attempt as yorishiro.
    echo "tampered" > /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null && {
      echo "REDIRECTOVERWRITE_SUCCEEDED"
    } || echo "REDIRECTOVERWRITE_FAILED"
  '"'"'
  # Root verifies wrapper integrity and atomically restores.
  meta=$(stat -c "%U:%G %a" /var/lib/yorishiro/worker-wrapper.sh 2>/dev/null)
  orig=$(cat /tmp/wrapper-meta.bak)
  if [ "$meta" = "$orig" ]; then
    echo "POST_META_INTACT"
  else
    echo "POST_META_CHANGED"
  fi
  # Atomic restore: copy to temp, set temp ownership to root:root, then mv -f over wrapper.
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
    echo "POST_RESTORED"
  else
    echo "POST_RESTORE_FAILED"
  fi
' 2>&1)
ALL_WANTS="RUNS USER COPYRIGHT CONFIG CONFIGPERM STATEOWNER STATEPERMS STATEGRP WRAPPEROWNER WRAPPERPERMS STATEWRITE WRAPPER_UNRMED_BY_YORISHIRO WRAPPER_INTACT_AFTER_MV REDIRECTOVERWRITE_FAILED POST_META_INTACT POST_RESTORED"
if [ "$(edition)" = ee ]; then ALL_WANTS="$ALL_WANTS EE_LICENCE"; fi
for want in $ALL_WANTS; do
  case "$probe" in
    *"$want"*) ok "$want" ;;
    *) bad "$want (probe output: $(echo "$probe" | tr '\n' ' '))" ;;
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
note "the systemd unit is valid"
# --------------------------------------------------------------------------------------------
# `systemd-analyze` resolves ExecStart, so a unit checked without its binary reports a missing
# command: a failure about the test environment rather than about the unit.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq systemd /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  systemd-analyze verify /lib/systemd/system/yorishiro.service /lib/systemd/system/yorishiro-worker.service /lib/systemd/system/yorishiro-scheduler.service 2>&1' 2>&1)
if [ -z "$(echo "$out" | grep -v '^$')" ]; then
  ok "systemd-analyze verify is silent on server and worker units"
else
  bad "systemd-analyze verify complained: $(echo "$out" | head -3 | tr '\n' ' ')"
fi

out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  grep -m1 "^ExecStart=" /lib/systemd/system/yorishiro-worker.service
' 2>&1)
if grep -q "^ExecStart=/var/lib/yorishiro/worker-wrapper.sh" <<<"$out"; then
  ok "worker unit uses programmatic wrapper for tag discovery"
else
  bad "worker unit ExecStart does not reference the wrapper: $out"
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
  units="/lib/systemd/system/yorishiro.service /lib/systemd/system/yorishiro-worker.service /lib/systemd/system/yorishiro-scheduler.service"
  for unit in $units; do
    [ -f "$unit" ] || { echo "NO_UNIT:$unit"; exit 1; }
  done
  grep -hoE "(^|[[:space:]=\"])/[A-Za-z0-9._/-]+" $units \
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
  " >/dev/null 2>&1
  # Started the way the unit does, since there is no systemd here: the same Loco environment
  # selects the packaged production configuration.
  #
  # The package ships /etc/yorishiro/production.yaml and its header documents the variables an
  # operator sets to point it at PostgreSQL, so this sets those instead of writing a second
  # configuration. A hand-written copy drifts from the shipped one: it once omitted `settings:`
  # and the server exited at start with "missing field `max_tenants`".
  #
  # The output goes to a file because `docker exec -d` discards it and PID 1 here is `sleep`,
  # so `docker logs` never shows the server.
  docker exec -d \
    -e DATABASE_URL="postgres://yorishiro:secret@pg-$$:5432/yorishiro" \
    -e HOST=http://127.0.0.1:5150 \
    -e DB_CONNECT_TIMEOUT=5000 \
    "app-$$" bash -c \
    'cd /var/lib/yorishiro && exec su -s /bin/sh yorishiro -c \
      "env LOCO_ENV=production LOCO_CONFIG_FOLDER=/etc/yorishiro \
      YORISHIRO_EMBEDDING_PROVIDER=none /usr/bin/yorishiro start" >/tmp/yorishiro.log 2>&1'

  up=
  for _ in $(seq 1 120); do
    docker exec "app-$$" curl -fsS http://127.0.0.1:5150/_ping >/dev/null 2>&1 && { up=1; break; }
    sleep 1
  done

  if [ -z "$up" ]; then
    bad "configured server never answered /_ping"
    docker exec "app-$$" tail -n 30 /tmp/yorishiro.log >&2
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
