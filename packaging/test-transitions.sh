#!/usr/bin/env bash
# Exercise package identity, replacement, and shared-file behavior in Debian and RPM containers.
set -euo pipefail

PKG_DIR="${1:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
UNIFIED_DIR="${2:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
VERSION="${3:?usage: test-transitions.sh <package directory> <unified package directory> <version>}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG_DIR="$(cd "$PKG_DIR" && pwd)"
UNIFIED_DIR="$(cd "$UNIFIED_DIR" && pwd)"
LEGACY_DIR="$(mktemp -d)"
LEGACY_CFG="$REPO/packaging/nfpm-yorishiro-unified-legacy.yaml"
trap 'rm -rf "$LEGACY_DIR"' EXIT

# Build a real artifact from the previous unified manifest shape. This exercises the package
# manager's ownership transition instead of creating production.yaml after installing the new
# layout. The binary and service files are the same; only the old managed config entry differs.
command -v nfpm >/dev/null || { echo "nfpm is required to build the legacy fixture" >&2; exit 2; }
nfpm package --config "$LEGACY_CFG" --packager deb --target "$LEGACY_DIR"
nfpm package --config "$LEGACY_CFG" --packager rpm --target "$LEGACY_DIR"
mv "$LEGACY_DIR/yorishiro_${VERSION}_amd64.deb" "$LEGACY_DIR/yorishiro-${VERSION}-amd64.deb"
mv "$LEGACY_DIR/yorishiro-${VERSION}-1.x86_64.rpm" "$LEGACY_DIR/yorishiro-${VERSION}-amd64.rpm"

for edition in ce ee; do
  for arch in amd64 arm64; do
    for ext in deb rpm; do
      file="$PKG_DIR/yorishiro-${edition}-${VERSION}-${arch}.${ext}"
      test -f "$file" || { echo "missing exact artifact: $file" >&2; exit 1; }
    done
  done
done
test "$(find "$PKG_DIR" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) | wc -l)" -eq 8

legacy_config_metadata='test -f /etc/yorishiro/production.yaml && test "$(stat -c "%U:%G" /etc/yorishiro/production.yaml)" = root:yorishiro && test "$(stat -c "%a" /etc/yorishiro/production.yaml)" = 640'

run_deb_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.deb"
  if [ "$from" = yorishiro ]; then base="/legacy/yorishiro-${VERSION}-amd64.deb"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq systemd-sysv $base
    $legacy_config_metadata
    # The unified artifact is the prior package identity/layout in this transition matrix.
    # Seed the legacy package-owned path before installing the new edition package.
    printf 'edited-by-transition' > /etc/yorishiro/production.yaml
    chown root:yorishiro /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    apt-get install -y -qq /pkg/yorishiro-${to}-${VERSION}-amd64.deb
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-${to})\" = 'install ok installed'
    test \"\$(dpkg-query -W -f='\${Status}' yorishiro-${from} 2>/dev/null || true)\" != 'install ok installed'
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test -f /etc/yorishiro/example.yaml
    test \"\$(cat /var/lib/yorishiro/transition-state)\" = state
    test \"\$(stat -c '%U:%G' /var/lib/yorishiro)\" = root:yorishiro
    test \"\$(stat -c '%a' /var/lib/yorishiro)\" = 1770
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-worker.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-scheduler.service
    test -x /usr/bin/yorishiro
  "
}

run_deb_untouched_transition() {
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq systemd-sysv /legacy/yorishiro-${VERSION}-amd64.deb
    $legacy_config_metadata
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    apt-get install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
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

run_deb_absent_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.deb"
  if [ "$from" = yorishiro ]; then base="/legacy/yorishiro-${VERSION}-amd64.deb"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq $base
    $legacy_config_metadata
    rm -f /etc/yorishiro/production.yaml
    apt-get install -y -qq /pkg/yorishiro-${to}-${VERSION}-amd64.deb
    test ! -e /etc/yorishiro/production.yaml
    test -f /etc/yorishiro/example.yaml
  "
}

run_deb_current_remove_preserves_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
    test -f /etc/yorishiro/example.yaml
    cp /etc/yorishiro/example.yaml /etc/yorishiro/production.yaml
    printf '\n# administrator-owned\n' >> /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    apt-get remove -y -qq yorishiro-ce
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test ! -e /etc/yorishiro/example.yaml
  "
}

run_rpm_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.rpm"
  if [ "$from" = yorishiro ]; then base="/legacy/yorishiro-${VERSION}-amd64.rpm"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro almalinux:10 bash -eu -c "
    dnf install -y -q $base
    $legacy_config_metadata
    # Seed the legacy package-owned path before installing the new edition package.
    printf 'edited-by-transition' > /etc/yorishiro/production.yaml
    chown root:yorishiro /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    dnf install -y -q /pkg/yorishiro-${to}-${VERSION}-amd64.rpm
    rpm -q yorishiro-${to}
    ! rpm -q yorishiro-${from}
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test -f /etc/yorishiro/example.yaml
    test \"\$(cat /var/lib/yorishiro/transition-state)\" = state
    test \"\$(stat -c '%U:%G' /var/lib/yorishiro)\" = root:yorishiro
    test \"\$(stat -c '%a' /var/lib/yorishiro)\" = 1770
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-worker.service
    test -L /etc/systemd/system/multi-user.target.wants/yorishiro-scheduler.service
    test -x /usr/bin/yorishiro
  "
}

run_rpm_untouched_transition() {
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro almalinux:10 bash -eu -c "
    dnf install -y -q /legacy/yorishiro-${VERSION}-amd64.rpm
    $legacy_config_metadata
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    dnf install -y -q /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
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

run_rpm_absent_transition() {
  local from="$1" to="$2"
  local base="/pkg/yorishiro-${from}-${VERSION}-amd64.rpm"
  if [ "$from" = yorishiro ]; then base="/legacy/yorishiro-${VERSION}-amd64.rpm"; fi
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$LEGACY_DIR":/legacy:ro almalinux:10 bash -eu -c "
    dnf install -y -q $base
    $legacy_config_metadata
    rm -f /etc/yorishiro/production.yaml
    dnf install -y -q /pkg/yorishiro-${to}-${VERSION}-amd64.rpm
    test ! -e /etc/yorishiro/production.yaml
    test -f /etc/yorishiro/example.yaml
  "
}

run_rpm_current_remove_preserves_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro almalinux:10 bash -eu -c "
    dnf install -y -q /pkg/yorishiro-ce-${VERSION}-amd64.rpm
    test -f /etc/yorishiro/example.yaml
    cp /etc/yorishiro/example.yaml /etc/yorishiro/production.yaml
    printf '\n# administrator-owned\n' >> /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    dnf remove -y -q yorishiro-ce
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    test ! -e /etc/yorishiro/example.yaml
  "
}

run_deb_transition yorishiro ce
run_deb_untouched_transition
run_deb_transition ce ee
run_deb_transition ee ce
run_deb_colocation_refusal
run_deb_absent_transition yorishiro ce
run_deb_current_remove_preserves_admin_config
run_rpm_transition yorishiro ce
run_rpm_transition yorishiro ee
run_rpm_transition ce ee
run_rpm_transition ee ce
run_rpm_colocation_refusal
run_rpm_absent_transition yorishiro ce
run_rpm_untouched_transition
run_rpm_current_remove_preserves_admin_config
