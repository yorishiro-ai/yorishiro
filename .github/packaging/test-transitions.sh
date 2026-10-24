#!/usr/bin/env bash
# Exercise package identity, replacement, and shared-file behavior in Debian and RPM containers.
set -euo pipefail

PKG_DIR="${1:?usage: test-transitions.sh <package directory> <version>}"
VERSION="${2:?usage: test-transitions.sh <package directory> <version>}"
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

current_package_id() {
  case "$1" in
    ce|ee) printf 'yorishiro-%s\n' "$1" ;;
    *) echo "unknown current package edition: $1" >&2; return 2 ;;
  esac
}

run_deb_transition() {
  local from="$1" to="$2"
  local from_package to_package
  from_package="$(current_package_id "$from")"
  to_package="$(current_package_id "$to")"
  local old_package="$from_package"
  local base="/pkg/${from_package}-${VERSION}-amd64.deb"
  docker run --rm -e OLD_PACKAGE="$old_package" -v "$PKG_DIR":/pkg:ro -v "$REPO/config/production.yaml":/canonical-production.yaml:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq systemd-sysv $base
    test ! -e /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml.example /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    apt-get install -y -qq /pkg/${to_package}-${VERSION}-amd64.deb
    test \"\$(dpkg-query -W -f='\${Status}' ${to_package})\" = 'install ok installed'
    set +e
    old_status=\"\$(dpkg-query -W -f='\${Status}' \"\$OLD_PACKAGE\" 2>/dev/null)\"
    old_query_status=\$?
    set -e
    if [ \"\$old_query_status\" -eq 0 ]; then
      test \"\$old_status\" != 'install ok installed'
    else
      test \"\$old_query_status\" -eq 1
      test -z \"\$old_status\"
    fi
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    cmp /etc/yorishiro/production.yaml.example /canonical-production.yaml
    test -f /etc/yorishiro/production.yaml.example
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

run_deb_current_remove_preserves_admin_config() {
  docker run --rm -v "$PKG_DIR":/pkg:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq /pkg/yorishiro-ce-${VERSION}-amd64.deb
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
  docker run --rm -v "$PKG_DIR":/pkg:ro -v "$REPO/config/production.yaml":/canonical-production.yaml:ro ubuntu:24.04 bash -eu -c "
    apt-get update -qq
    apt-get install -y -qq systemd-sysv /pkg/yorishiro-ce-${VERSION}-amd64.deb
    test ! -e /etc/yorishiro/production.yaml
    cmp /etc/yorishiro/production.yaml.example /canonical-production.yaml
  "
}

run_rpm_transition() {
  local from="$1" to="$2"
  local from_package to_package
  from_package="$(current_package_id "$from")"
  to_package="$(current_package_id "$to")"
  local old_package="$from_package"
  local base="/pkg/${from_package}-${VERSION}-amd64.rpm"
  docker run --rm -e OLD_PACKAGE="$old_package" -v "$PKG_DIR":/pkg:ro -v "$REPO/config/production.yaml":/canonical-production.yaml:ro almalinux:10 bash -eu -c "
    dnf install -y -q diffutils $base
    test ! -e /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml.example /etc/yorishiro/production.yaml
    chown root:root /etc/yorishiro/production.yaml
    chmod 0600 /etc/yorishiro/production.yaml
    cp /etc/yorishiro/production.yaml /tmp/production.before
    stat -c '%U:%G %a' /etc/yorishiro/production.yaml > /tmp/production.meta
    mkdir -p /var/lib/yorishiro
    printf 'state' > /var/lib/yorishiro/transition-state
    chown -R yorishiro:yorishiro /var/lib/yorishiro
    systemctl --root=/ enable yorishiro.service
    systemctl --root=/ enable yorishiro-worker.service
    systemctl --root=/ enable yorishiro-scheduler.service
    set +e
    dnf install -y -q /pkg/${to_package}-${VERSION}-amd64.rpm
    switch_install_status=\$?
    set -e
    test "\$switch_install_status" -eq 0
    rpm -q ${to_package}
    target_query_status=\$?
    test "\$target_query_status" -eq 0
    set +e
    rpm -q \"\$OLD_PACKAGE\"
    old_query_status=\$?
    set -e
    if [ \"\$old_query_status\" -eq 0 ]; then
      echo \"old package still installed: \$OLD_PACKAGE\" >&2
      exit 1
    fi
    test \"\$old_query_status\" -eq 1
    echo \"RPM_SWITCH_RESULT from=${from_package} to=${to_package} dnf_install=\$switch_install_status target_query=\$target_query_status source_query=\$old_query_status\"
    cmp /tmp/production.before /etc/yorishiro/production.yaml
    test \"\$(stat -c '%U:%G %a' /etc/yorishiro/production.yaml)\" = \"\$(cat /tmp/production.meta)\"
    cmp /etc/yorishiro/production.yaml.example /canonical-production.yaml
    test -f /etc/yorishiro/production.yaml.example
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

run_deb_transition ce ee
run_deb_transition ee ce
run_deb_colocation_refusal
run_deb_fresh_install_without_admin_config
run_deb_current_remove_preserves_admin_config
run_rpm_transition ce ee
run_rpm_transition ee ce
run_rpm_colocation_refusal
run_rpm_fresh_install_without_admin_config
run_rpm_current_remove_preserves_admin_config
