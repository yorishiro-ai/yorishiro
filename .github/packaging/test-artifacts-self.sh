#!/usr/bin/env bash
# Check that artifact metadata validation rejects common naming and header regressions.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST="${1:?usage: test-artifacts-self.sh <distribution directory> <version>}"
VERSION="${2:?usage: test-artifacts-self.sh <distribution directory> <version>}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

for case_name in renamed missing wrong-version; do
  mkdir -p "$WORK/$case_name"
  cp "$DIST"/*.deb "$DIST"/*.rpm "$WORK/$case_name/"
done

mv "$WORK/renamed/yorishiro-ce-${VERSION}-amd64.deb" "$WORK/renamed/renamed.deb"
rm "$WORK/missing/yorishiro-ee-${VERSION}-arm64.rpm"
dpkg-deb -R "$WORK/wrong-version/yorishiro-ce-${VERSION}-amd64.deb" "$WORK/control"
sed -i "s/^Version: .*/Version: ${VERSION}.wrong-version/" "$WORK/control/DEBIAN/control"
dpkg-deb -b "$WORK/control" "$WORK/wrong-version/yorishiro-ce-${VERSION}-amd64.deb" >/dev/null

for case_name in renamed missing wrong-version; do
  if "$REPO/.github/packaging/test-artifacts.sh" "$WORK/$case_name" "$VERSION" >/dev/null 2>&1; then
    echo "artifact verification unexpectedly accepted $case_name" >&2
    exit 1
  fi
done

echo "artifact negative cases: rejected renamed, missing, and wrong-version packages"
