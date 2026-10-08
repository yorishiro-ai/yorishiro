#!/usr/bin/env bash
# Exercise package identity, replacement, and shared-file behavior in Debian and RPM containers.
set -euo pipefail

PKG_DIR="${1:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
UNIFIED_DIR="${2:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
VERSION="${3:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"
UNIFIED_DIR="$(cd "$UNIFIED_DIR" && pwd)"

for edition in ce ee; do
  for arch in amd64 arm64; do
    for ext in deb rpm; do
      file="$PKG_DIR/yorishiro-${edition}-${VERSION}-${arch}.${ext}"
      test -f "$file" || { echo "missing exact artifact: $file" >&2; exit 1; }
    done
  done
done
test "$(find "$PKG_DIR" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) | wc -l)" -eq 8

run_deb_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.deb"
  if [ "$from" = yorishiro ]; then base="/unified/yorishiro-${VERSION}-amd64.deb"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$UNIFIED_DIR":/unified:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq systemd-sysv $base
    printf 'edited-by-transition' > /etc/yorishiro/production.yaml
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    apt-get install -y -qq /pkg/yorishiro-${to}-${VERSION}-amd64.deb
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-${to})\" = 'install ok installed'
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-${from} 2>/dev/null || true)\" != 'install ok installed'
    grep -qx edited-by-transition /etc/yorishiro/production.yaml
    test \"\$(cat /var/lib/yorishiro/transition-state)\" = state
    test \"\$(stat -c '%U:%G' /var/lib/yorishiro)\" = root:yorishiro
    test \"\$(stat -c '%a' /var/lib/yorishiro)\" = 1770
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-worker.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-scheduler.service
    test -x /usr/bin/yorishiro
  "
}

run_deb_colocation_refusal() {
  docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
    dpkg --unpack /pkg/yorishiro-ee-${VERSION}-amd64.deb || true
    dpkg --configure -a
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-ce 2>/dev/null || true)\" != 'install ok installed'
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-ee 2>/dev/null || true)\" = 'install ok installed'
  "
}

run_rpm_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.rpm"
  if [ "$from" = yorishiro ]; then base="/unified/yorishiro-${VERSION}-amd64.rpm"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$UNIFIED_DIR":/unified:ro almalinux:10 bash -eu -c "
    dnf install -y -q $base
    printf 'edited-by-transition' > /etc/yorishiro/production.yaml
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    dnf install -y -q /pkg/yorishiro-${to}-${VERSION}-amd64.rpm
    rpm -q yorishiro-${to}
    ! rpm -q yorishiro-${from}
    grep -qx edited-by-transition /etc/yorishiro/production.yaml
    test \"\$(cat /var/lib/yorishiro/transition-state)\" = state
    test \"\$(stat -c '%U:%G' /var/lib/yorishiro)\" = root:yorishiro
    test \"\$(stat -c '%a' /var/lib/yorishiro)\" = 1770
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-worker.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-scheduler.service
    test -x /usr/bin/yorishiro
  "
}

run_rpm_colocation_refusal() {
  docker run --rm -v "$PKG_DIR":/pkg:ro almalinux:10 bash -eu -c "
    dnf install -y -q /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    rpm -i /pkg/yorishiro-ee-${VERSION}-amd64.rpm || true
    test \"\$(rpm -q yorishiro-ce 2>/dev/null || true)\" = yorishiro-ce-${VERSION}-1.x86_64
    test \"\$(rpm -q yorishiro-ee 2>/dev/null || true)\" != yorishiro-ee-${VERSION}-1.x86_64
  "
}

run_deb_transition yorishiro ce
run_deb_transition ce ee
run_deb_transition ee ce
run_deb_colocation_refusal
run_rpm_transition yorishiro ce
run_rpm_transition yorishiro ee
run_rpm_transition ce ee
run_rpm_transition ee ce
run_rpm_colocation_refusal
