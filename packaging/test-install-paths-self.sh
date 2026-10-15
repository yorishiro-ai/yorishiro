#!/usr/bin/env bash
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_TEST="$REPO/packaging/test-install.sh"

path_is_exempt() {
  local path="$1"
  grep -qE "^/(proc|sys)(/|$)|^/etc/yorishiro/production\.yaml$" <<<"$path"
}

grep -Fq '^/etc/yorishiro/production\.yaml$' "$INSTALL_TEST"
if ! path_is_exempt /etc/yorishiro/production.yaml; then
  echo 'optional admin override was treated as required' >&2
  exit 1
fi
if path_is_exempt /etc/yorishiro/production.yaml.missing; then
  echo 'missing required path was incorrectly exempted' >&2
  exit 1
fi
! path_is_exempt /etc/yorishiro/production.yaml.example
! path_is_exempt /usr/bin/yorishiro
! path_is_exempt /lib/systemd/system/yorishiro.service
path_is_exempt /proc/1/status
path_is_exempt /sys/kernel/osrelease

printf '%s\n' 'install path policy: optional override excluded; required package paths retained'
