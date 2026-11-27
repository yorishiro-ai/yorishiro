#!/usr/bin/env bash
# Exercise current package installation, conflicts, removal, and version ordering.
set -euo pipefail

PKG_DIR="${1:?usage: test-package-lifecycle.sh <package directory> <version>}"
VERSION="${2:?usage: test-package-lifecycle.sh <package directory> <version>}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"
for edition in ce ee; do
  for arch in amd64 arm64; do
    for ext in deb rpm; do
      file="$PKG_DIR/yorishiro-${edition}-${VERSION}-${arch}.${ext}"
      test -f "$file" || { echo "missing exact artifact: $file" >&2; exit 1; }
    done
  done
done
test "$(find "$PKG_DIR" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) | wc -l)" -eq 8

check_prerelease_version_order() {
  local current earlier later stable
  current="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$REPO/Cargo.toml")"
  earlier="$(cd "$REPO" && python3 scripts/release_version.py pre-20261104-2)"
  later="$(cd "$REPO" && python3 scripts/release_version.py pre-20261104-10)"
  stable="${earlier%%~*}"
  dpkg --compare-versions "$current" lt "$earlier"
  dpkg --compare-versions "$earlier" lt "$later"
  dpkg --compare-versions "$later" lt "$stable"
  docker run --rm -e CURRENT="$current" -e EARLIER="$earlier" -e LATER="$later" -e STABLE="$stable" almalinux:10 bash -eu -c '
    for pair in "$CURRENT $EARLIER" "$EARLIER $LATER" "$LATER $STABLE"; do
      read -r first second <<< "$pair"
      test "$(rpm --eval "%{lua:print(rpm.vercmp(\"$first\",\"$second\"))}")" = -1
    done
  '
}

run_deb_colocation_refusal() {
  docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:26.04 bash -eu -c "
    apt-get -o Acquire::Retries=5 update -qq
    apt-get -o Acquire::Retries=5 install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
    if dpkg --unpack /pkg/yorishiro-ee-${VERSION}-amd64.deb; then
      echo 'conflicting package unexpectedly installed' >&2
      exit 1
    fi
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-ce)\" = 'install ok installed'
  "
}

run_deb_current_remove_preserves_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:26.04 bash -eu -c "
    apt-get -o Acquire::Retries=5 update -qq
    apt-get -o Acquire::Retries=5 install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
    test -f /etc/yorishiro/production.yaml.example
    cp /etc/yorishiro/production.yaml.example /etc/yorishiro/production.yaml
    printf '\n# administrator-owned\n' >> /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    apt-get remove -y -qq yorishiro-ce
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test ! -e /etc/yorishiro/production.yaml.example
  "
}

run_deb_fresh_install_without_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$REPO/config/production.yaml":/canonical-production.yaml:ro ubuntu:26.04 bash -eu -c "
    apt-get -o Acquire::Retries=5 update -qq
    apt-get -o Acquire::Retries=5 install -y -qq systemd-sysv /pkg/yorishiro-ce-${VERSION}-amd64.deb
    test ! -e /etc/yorishiro/production.yaml
    cmp /etc/yorishiro/production.yaml.example /canonical-production.yaml
  "
}

run_rpm_colocation_refusal() {
  docker run --rm -v "$PKG_DIR":/pkg:ro almalinux:10 bash -eu -c "
    dnf install -y -q /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    if rpm -i /pkg/yorishiro-ee-${VERSION}-amd64.rpm; then
      echo 'conflicting package unexpectedly installed' >&2
      exit 1
    fi
    rpm -q yorishiro-ce
    if rpm -q yorishiro-ee; then
      exit 1
    fi
  "
}

run_rpm_current_remove_preserves_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro almalinux:10 bash -eu -c "
    dnf install -y -q diffutils /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    test -f /etc/yorishiro/production.yaml.example
    cp /etc/yorishiro/production.yaml.example /etc/yorishiro/production.yaml
    printf '\n# administrator-owned\n' >> /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    dnf remove -y -q yorishiro-ce
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test ! -e /etc/yorishiro/production.yaml.example
  "
}

run_rpm_fresh_install_without_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$REPO/config/production.yaml":/canonical-production.yaml:ro almalinux:10 bash -eu -c "
    dnf install -y -q diffutils /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    test ! -e /etc/yorishiro/production.yaml
    cmp /etc/yorishiro/production.yaml.example /canonical-production.yaml
  "
}

check_prerelease_version_order
run_deb_colocation_refusal
run_deb_fresh_install_without_admin_config
run_deb_current_remove_preserves_admin_config
run_rpm_colocation_refusal
run_rpm_fresh_install_without_admin_config
run_rpm_current_remove_preserves_admin_config
