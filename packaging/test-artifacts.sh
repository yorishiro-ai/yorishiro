#!/usr/bin/env bash
# Verify public package filenames and embedded metadata.
set -euo pipefail

DIST="${1:?usage: test-artifacts.sh <distribution directory> <version>}"
VERSION="${2:?usage: test-artifacts.sh <distribution directory> <version>}"

for edition in ce ee; do
  for arch in amd64 arm64; do
    case "$arch" in amd64) deb_arch=amd64; rpm_arch=x86_64 ;; arm64) deb_arch=arm64; rpm_arch=aarch64 ;; esac
    package="yorishiro-${edition}"
    deb="$DIST/${package}-${VERSION}-${arch}.deb"
    rpm="$DIST/${package}-${VERSION}-${arch}.rpm"
    for ext in deb rpm; do
      test -f "$DIST/${package}-${VERSION}-${arch}.${ext}"
    done
    test "$(dpkg-deb -f "$deb" Package)" = "$package"
    test "$(dpkg-deb -f "$deb" Version)" = "$VERSION"
    test "$(dpkg-deb -f "$deb" Architecture)" = "$deb_arch"
    test "$(rpm -qp --qf '%{NAME}' "$rpm")" = "$package"
    test "$(rpm -qp --qf '%{VERSION}' "$rpm")" = "$VERSION"
    test "$(rpm -qp --qf '%{RELEASE}' "$rpm")" = 1
    test "$(rpm -qp --qf '%{ARCH}' "$rpm")" = "$rpm_arch"
  done
done
test "$(find "$DIST" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) | wc -l)" -eq 8
for f in "$DIST"/*.deb; do
  dpkg-deb -I "$f" | grep -E 'Conflicts:.*yorishiro|Replaces:.*yorishiro'
done
for f in "$DIST"/*.rpm; do
  rpm -qp --conflicts "$f" | grep -Eq '^(yorishiro|yorishiro-ce|yorishiro-ee)$'
  rpm -qp --obsoletes "$f" | grep -Eq '^(yorishiro|yorishiro-ce|yorishiro-ee)$'
done
