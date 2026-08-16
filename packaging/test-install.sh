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
GLIBC_FLOOR="$(grep -oE 'GLIBC_[0-9]+\.[0-9]+' "$REPO/packaging/nfpm-yorishiro-ee.yaml" | sort -uV | tail -1)"

# Checked up front rather than at the call site: a missing tool otherwise surfaces as the
# assertion it happens to sit in, which reads as a packaging fault.
for tool in docker jq; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

pass=0 fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '\n== %s ==\n' "$1"; }

deb() { ls "$PKG_DIR"/yorishiro-ee_*.deb | head -1; }
deb_ce() { ls "$PKG_DIR"/yorishiro-ce_*.deb | head -1; }
rpm() { ls "$PKG_DIR"/yorishiro-ee-[0-9]*.rpm | head -1; }
rpm_ce() { ls "$PKG_DIR"/yorishiro-ce-[0-9]*.rpm | head -1; }

# --------------------------------------------------------------------------------------------
note "deb on ubuntu:24.04 — the supported case"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-server --help >/dev/null 2>&1 && echo "RUNS"
  getent passwd yorishiro >/dev/null && echo "USER"
  [ -f /usr/share/doc/yorishiro-ee/copyright ] && echo "COPYRIGHT"
  [ -f /etc/yorishiro/config.example.yml ] && echo "EXAMPLE"
  [ "$(stat -c "%a %U:%G" /etc/yorishiro/config.yml)" = "640 root:yorishiro" ] && echo "ENVPERM"
  [ "$(stat -c "%U" /var/lib/yorishiro)" = "yorishiro" ] && echo "STATEOWNER"
' 2>&1)
for want in RUNS USER COPYRIGHT EXAMPLE ENVPERM STATEOWNER; do
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
# It found one. `ee/LICENSE` shipped as `/usr/share/doc/yorishiro-ee/copyright.ee`, and the
# `path-include` covers the name `copyright` exactly -- so the enterprise edition delivered
# everything except the licence it is distributed under, on every minimal image.
check_declared_files() {
  pkg="$1" label="$2"
  out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
    set -euo pipefail
    apt-get update -qq >/dev/null 2>&1
    dpkg-deb -c /pkg/'"$pkg"' | awk "\$6 ~ /^\.\// && \$1 !~ /^d/ { print substr(\$6, 2) }" > /tmp/declared
    apt-get install -y -qq /pkg/'"$pkg"' >/dev/null 2>&1
    while read -r f; do
      [ -e "$f" ] || echo "DROPPED:$f"
    done < /tmp/declared
    echo "COMPARED:$(wc -l < /tmp/declared)"' 2>&1)
  if ! grep -q COMPARED: <<<"$out"; then
    bad "$label: could not compare the package against the install: $(echo "$out" | tr '\n' ' ')"
  elif grep -q DROPPED: <<<"$out"; then
    bad "$label: files in the package are missing after install: $(grep -o 'DROPPED:[^ ]*' <<<"$out" | tr '\n' ' ')"
  else
    ok "$label: all $(grep -o 'COMPARED:[0-9]*' <<<"$out" | cut -d: -f2) declared files are present"
  fi
}
check_declared_files "$(basename "$(deb)")" "yorishiro-ee"
check_declared_files "$(basename "$(deb_ce)")" "yorishiro-ce"

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
  [ -f /usr/share/doc/yorishiro-ee/copyright ] && echo "COPYRIGHT"
  getent passwd yorishiro >/dev/null && echo "USER"
' 2>&1)
for want in RUNS COPYRIGHT USER; do
  case "$out" in
    *"$want"*) ok "fedora:39 $want" ;;
    *) bad "fedora:39 $want (output: $(echo "$out" | tr '\n' ' '))" ;;
  esac
done

# The community rpm too. Four packages are built and it would otherwise be the only one never
# installed anywhere -- the same gap the paid rpm had before this file existed. A separate
# container because the two packages conflict.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro fedora:39 bash -c '
  dnf install -y -q /pkg/'"$(basename "$(rpm_ce)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-server --help >/dev/null 2>&1 && echo "RUNS"
  [ -f /usr/share/doc/yorishiro-ce/copyright ] && echo "COPYRIGHT"
' 2>&1)
for want in RUNS COPYRIGHT; do
  case "$out" in
    *"$want"*) ok "fedora:39 ce $want" ;;
    *) bad "fedora:39 ce $want (output: $(echo "$out" | tr '\n' ' '))" ;;
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
  su -s /bin/sh yorishiro -c "/usr/bin/yorishiro-server" 2>&1
  echo "EXIT:$?"' 2>&1)

# 78 (EX_CONFIG), not 1. The units set `RestartPreventExitStatus=78`, so this number is what
# stops an unconfigured `enable --now` from restarting every five seconds forever while
# `systemctl is-failed` answers `activating` -- measured at 15 restarts in 45 seconds before it
# existed. A database that is merely not up yet must keep exiting 1 and keep being retried, so
# the two cases have to stay distinguishable; `a missing database still retries` below is the
# other half of that pair.
if grep -q '^EXIT:78$' <<<"$out"; then
  ok "an absent DATABASE_URL exits 78 (EX_CONFIG)"
else
  bad "expected exit 78 for missing config, got: $(grep -o 'EXIT:[0-9]*' <<<"$out")"
fi
if grep -q '/etc/yorishiro/config.yml' <<<"$out"; then
  ok "names the file to edit"
else
  bad "does not name the env file: $(echo "$out" | head -2 | tr '\n' ' ')"
fi
if grep -qE 'panicked at|RUST_BACKTRACE' <<<"$out"; then
  bad "still prints a Rust panic at an operator"
else
  ok "no panic, no source path"
fi

# The other half of the pair above. If an unreachable database also exited 78 the units would
# stop retrying it, turning a boot-order race with a same-host postgres -- which resolves itself
# in seconds -- into a permanently failed unit. Verified live: with the database arriving late,
# the service self-heals on its sixth restart.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1
  # Set inside the -c string: `su` does not carry the caller environment across.
  su -s /bin/sh yorishiro -c \
    "DATABASE_URL=postgres://nobody:nope@127.0.0.1:59999/nodb /usr/bin/yorishiro-server" \
    >/dev/null 2>&1
  echo "EXIT:$?"' 2>&1)
if grep -q '^EXIT:1$' <<<"$out"; then
  ok "an unreachable database still exits 1, so it keeps being retried"
else
  bad "expected exit 1 for an unreachable database, got: $(grep -o 'EXIT:[0-9]*' <<<"$out")"
fi

# --------------------------------------------------------------------------------------------
note "the community package is the community package"
# --------------------------------------------------------------------------------------------
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq /pkg/'"$(basename "$(deb_ce)")"' >/dev/null 2>&1 || { echo "INSTALL_FAILED"; exit 1; }
  /usr/bin/yorishiro-server --help >/dev/null 2>&1 && echo "RUNS"
  [ -f /usr/share/doc/yorishiro-ce/copyright ] && echo "COPYRIGHT"
  [ -f /etc/yorishiro/LICENSE.enterprise ] && echo "HAS_EE_LICENCE"
  # Anchored at both ends: `Conflicts: yorishiro` also matches `yorishiro-ee` as a prefix, so
  # the loose form kept passing after the package was renamed while asserting nothing.
  dpkg -s yorishiro-ce 2>/dev/null | grep -qiE "^Conflicts: yorishiro-ee *$" && echo "CONFLICTS"
  # The community binary has to answer for itself, not merely respond to --help: it is a
  # different executable at the same path, headless, with none of `ee/` composed in.
  su -s /bin/sh yorishiro -c "/usr/bin/yorishiro-server" >/dev/null 2>&1
  [ "$?" = 78 ] && echo "EXITS_78"
' 2>&1)
for want in RUNS COPYRIGHT CONFLICTS EXITS_78; do
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

# The same markers `check` scans in the build tree, re-checked on the installed binary. CI
# proves the artifact it built is clean; this proves the artifact a user receives is, which is
# a different claim once packaging sits between them.
#
# `binutils` is installed for `strings` and its absence is fatal rather than silent: without
# it every marker reports clean and a check that scanned nothing is indistinguishable from one
# that found nothing.
out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq binutils /pkg/'"$(basename "$(deb_ce)")"' >/dev/null 2>&1
  command -v strings >/dev/null || { echo "NO_STRINGS"; exit 1; }
  # And the binary itself: a failed install leaves `strings` erroring to stderr while every
  # grep finds nothing, which is the same false clean the missing tool produces.
  [ -x /usr/bin/yorishiro-server ] || { echo "NO_BINARY"; exit 1; }
  for m in hosted/stripe yorishiro_hosted api/marketplace LICENSE_KEY infer-fill; do
    strings -a /usr/bin/yorishiro-server | grep -q -- "$m" && echo "LEAK:$m"
  done
  echo "SCANNED"' 2>&1)
# `SCANNED` is the receipt that the loop actually ran; an install failure or a missing
# `strings` both land here as its absence.
if ! grep -q SCANNED <<<"$out"; then
  bad "the installed-binary marker scan did not run: $(echo "$out" | tr '\n' ' ')"
elif grep -q LEAK: <<<"$out"; then
  bad "paid markers in the installed ce binary: $(grep -o 'LEAK:[^ ]*' <<<"$out" | tr '\n' ' ')"
else
  ok "the installed ce binary carries no paid marker"
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
    cat >> /etc/yorishiro/config.yml <<EOF
database_url: postgres://yorishiro:secret@pg-$$:5432/yorishiro
bind: 0.0.0.0:8081
embedding:
  provider: openai
  base_url: http://localhost:1
  model: unused
EOF
  " >/dev/null 2>&1
  docker exec -d "app-$$" bash -c \
    'exec su -s /bin/sh yorishiro -c "YORISHIRO_CONFIG_PATH=/etc/yorishiro/config.yml /usr/bin/yorishiro-server"'

  up=
  for _ in $(seq 1 60); do
    docker exec "app-$$" curl -fsS http://127.0.0.1:8081/up >/dev/null 2>&1 && { up=1; break; }
    sleep 1
  done

  if [ -z "$up" ]; then
    bad "configured server never answered /up"
  else
    ok "starts and applies its migrations"

    # The field, not the word: `grep true` would also pass on
    # `{"setup_required":false,"something_else":true}`, which is the opposite of what this
    # asserts. `jq` rather than a pattern, so adding a field to the response cannot quietly
    # turn this green.
    status=$(docker exec "app-$$" curl -s http://127.0.0.1:8081/setup/status)
    if [ "$(jq -r '.setup_required' <<<"$status" 2>/dev/null)" = "true" ]; then
      ok "the first-run wizard is offered"
    else
      bad "setup_required was not true: $status"
    fi

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
#
# The same container also checks that every absolute path the unit *names* exists, comments
# included. `systemd-analyze` reads directives and ignores comments, so it was silent while both
# units pointed at a `config.example.yml` under `/usr/share/doc/` that had moved to `/etc/` --
# an operator following the unit's own instructions would have found nothing there. A path in a
# comment is documentation the package ships, so it is checked like the rest of the package.
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

  # Every absolute path, not a list of prefixes: a unit naming `/opt/...` or `/run/...` would
  # otherwise be scanned and reported complete without that path having been looked at.
  # `set -euo pipefail` so a half-finished install -- unit written, postinstall failed -- cannot
  # reach the `CHECKED` receipt and read as a pass.
  #
  # Three exclusions, each because the path is not a file the package ships: systemd's own
  # `WantedBy=` targets (`multi-user.target` is a unit name, and appears without a directory),
  # anything under `/proc` or `/sys`, and `/etc/yorishiro/config.yml` -- the optional config the
  # unit points `YORISHIRO_CONFIG_PATH` at. That last one is deliberately absent after install:
  # a deployment configured entirely through the env file never creates it, and shipping an
  # empty one would make `deny_unknown_fields` parse a file nobody wrote. The example beside it
  # (`config.example.yml`) is shipped, and is checked.
  out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
    set -euo pipefail
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y -qq /pkg/'"$pkg"' >/dev/null 2>&1
    unit=/lib/systemd/system/'"$unit"'
    [ -f "$unit" ] || { echo "NO_UNIT"; exit 1; }
    grep -oE "(^|[[:space:]=\"])/[A-Za-z0-9._/-]+" "$unit" \
      | sed -e "s/^[[:space:]=\"]//" -e "s/[.,]$//" \
      | grep -vE "^/(proc|sys)(/|$)|^/etc/yorishiro/config\.yml$" \
      | sort -u | while read -r p; do
        [ -e "$p" ] || echo "MISSING:$p"
      done
    echo CHECKED' 2>&1)
  if ! grep -q CHECKED <<<"$out"; then
    bad "could not check the paths $unit names: $(echo "$out" | tr '\n' ' ')"
  elif grep -q MISSING: <<<"$out"; then
    bad "$unit names paths the package does not install: $(grep -o 'MISSING:[^ ]*' <<<"$out" | tr '\n' ' ')"
  else
    ok "every path $unit names exists after install"
  fi
}
verify_unit "$(basename "$(deb)")" yorishiro.service
verify_unit "$(basename "$(deb_ce)")" yorishiro.service

# --------------------------------------------------------------------------------------------
note "switching editions keeps the machine working"
# --------------------------------------------------------------------------------------------
# The two packages conflict, so moving between editions is a remove and an install in one
# transaction -- a path an operator takes deliberately (evaluating the paid features, or
# dropping back) and one nothing else here covers.
#
# Two things have to survive it. The configuration and state, because they belong to the
# deployment rather than to the edition; and the systemd wiring, because the outgoing unit's
# `enable` symlink outlives the unit file it points at. Measured before `preremove.sh` existed:
# switching left a dangling multi-user.target.wants entry, the new edition not enabled, and
# nothing starting at boot.
switch_editions() {
  from="$1" to="$2" label="$3"
  out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
    set -uo pipefail
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y -qq systemd /pkg/'"$from"' >/dev/null 2>&1 || { echo "FIRST_INSTALL_FAILED"; exit 1; }

    # What a configured deployment looks like, in the two places that must outlive the switch.
    echo "database_url: postgres://kept:secret@db:5432/yorishiro" >> /etc/yorishiro/config.yml
    mkdir -p /var/lib/yorishiro && echo kept > /var/lib/yorishiro/marker

    # `enable`, by hand: there is no running systemd in this container to do it.
    mkdir -p /etc/systemd/system/multi-user.target.wants
    for u in /lib/systemd/system/yorishiro*.service; do
      ln -sf "$u" "/etc/systemd/system/multi-user.target.wants/$(basename "$u")"
    done

    apt-get install -y -qq /pkg/'"$to"' >/dev/null 2>&1 || { echo "SWITCH_FAILED"; exit 1; }

    grep -q "kept:secret" /etc/yorishiro/config.yml && echo CONFIG_KEPT
    [ "$(cat /var/lib/yorishiro/marker 2>/dev/null)" = kept ] && echo STATE_KEPT
    [ "$(dpkg -l | grep -c "^ii  yorishiro")" = 1 ] && echo ONE_PACKAGE
    # Both editions ship the same unit name, so a switch replaces the file the symlink points
    # at rather than removing it. A dangling link here would mean that stopped being true.
    for l in /etc/systemd/system/multi-user.target.wants/yorishiro*.service; do
      [ -e "$l" ] || echo "DANGLING:$(basename "$l")"
    done
    [ -L /etc/systemd/system/multi-user.target.wants/yorishiro.service ] && echo STILL_ENABLED
    echo SWITCHED' 2>&1)

  if ! grep -q SWITCHED <<<"$out"; then
    bad "$label: the switch did not complete: $(echo "$out" | tr '\n' ' ')"
    return
  fi
  for want in CONFIG_KEPT STATE_KEPT ONE_PACKAGE; do
    case "$out" in
      *"$want"*) ok "$label: $want" ;;
      *) bad "$label: $want (output: $(echo "$out" | tr '\n' ' '))" ;;
    esac
  done
  if grep -q DANGLING: <<<"$out"; then
    bad "$label: a dead enable symlink survived: $(grep -o 'DANGLING:[^ ]*' <<<"$out" | tr '\n' ' ')"
  elif ! grep -q STILL_ENABLED <<<"$out"; then
    bad "$label: the service is no longer enabled after the switch"
  else
    ok "$label: still enabled, no dead symlink"
  fi
}
switch_editions "$(basename "$(deb_ce)")" "$(basename "$(deb)")" "ce -> ee"
switch_editions "$(basename "$(deb)")" "$(basename "$(deb_ce)")" "ee -> ce"

# --------------------------------------------------------------------------------------------
note "upgrading from a release that configured through the environment"
# --------------------------------------------------------------------------------------------
# The path every existing deployment takes on its next upgrade, and the one where getting it
# wrong is silent: the unit stops reading the environment file, so a machine configured that
# way comes back up unconfigured and stops at 78 with nothing pointing at the cause.
#
# `dpkg -i` rather than apt, because the locally built package is version 0.0.0~rc0 and so
# sorts *below* the released one -- apt would decline the "downgrade". The install scripts run
# either way, which is what this exercises.
#
# Skipped rather than failed when the old package is not present: it comes from a GitHub
# release, and a test that needs the network is not one to fail an offline run on.
PREV_DEB="$(ls "$PKG_DIR"/yorishiro-ee_0.4*.deb 2>/dev/null | grep -v '0\.0\.0' | head -1)"
if [ -z "$PREV_DEB" ]; then
  echo "  SKIP no previous release in $PKG_DIR (fetch one with: gh release download vX.Y.Z -p 'yorishiro-ee_*amd64.deb')"
else
  out=$(docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -c '
    set -uo pipefail
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y -qq /pkg/'"$(basename "$PREV_DEB")"' >/dev/null 2>&1 || { echo "OLD_INSTALL_FAILED"; exit 1; }

    # Configured the way that release documented.
    echo "DATABASE_URL=postgres://kept:secret@db:5432/yorishiro" >> /etc/yorishiro/yorishiro.env

    dpkg -i /pkg/'"$(basename "$(deb)")"' >/dev/null 2>&1 || { echo "UPGRADE_FAILED"; exit 1; }

    [ -f /etc/yorishiro/config.yml ] && echo CONFIG_CREATED
    grep -q "yorishiro.env" /etc/yorishiro/config.yml && echo NOTE_ADDED
    # The old file is left alone: it is the only copy of what the deployment was running.
    grep -q "kept:secret" /etc/yorishiro/yorishiro.env && echo OLD_FILE_KEPT
    grep -q EnvironmentFile /lib/systemd/system/yorishiro.service || echo UNIT_CLEAN
    echo UPGRADED' 2>&1)

  if ! grep -q UPGRADED <<<"$out"; then
    bad "the upgrade did not complete: $(echo "$out" | tr '\n' ' ')"
  else
    for want in CONFIG_CREATED NOTE_ADDED OLD_FILE_KEPT UNIT_CLEAN; do
      case "$out" in
        *"$want"*) ok "upgrade: $want" ;;
        *) bad "upgrade: $want (output: $(echo "$out" | tr '\n' ' '))" ;;
      esac
    done
  fi
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
