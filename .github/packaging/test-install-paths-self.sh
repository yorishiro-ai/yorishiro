#!/usr/bin/env bash
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALL_TEST="$REPO/.github/packaging/test-install.sh"

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
for path in /etc/yorishiro/production.yaml.example /usr/bin/yorishiro /lib/systemd/system/yorishiro.service; do
  if path_is_exempt "$path"; then
    echo "required package path was incorrectly exempted: $path" >&2
    exit 1
  fi
done
path_is_exempt /proc/1/status
path_is_exempt /sys/kernel/osrelease

printf '%s\n' 'install path policy: optional override excluded; required package paths retained'
