#!/usr/bin/env bash
# Verify public package filenames and embedded metadata.
set -euo pipefail

DIST="${1:?usage: test-artifacts.sh <distribution directory> <version>}"
VERSION="${2:?usage: test-artifacts.sh <distribution directory> <version>}"
FILE_VERSION="${3:-$VERSION}"

for edition in ce ee; do
  for arch in amd64 arm64; do
    case "$arch" in amd64) deb_arch=amd64; rpm_arch=x86_64 ;; arm64) deb_arch=arm64; rpm_arch=aarch64 ;; esac
    package="yorishiro-${edition}"
    stem="$DIST/${package}-${FILE_VERSION}-${arch}"
    deb="$stem.deb"
    rpm="$stem.rpm"
    members=$'yorishiro\nLICENSE'
    if [ "$edition" = ee ]; then
      members+=$'\nee/LICENSE'
    fi
    test "$(tar -tzf "$stem.tar.gz")" = "$members"
    test "$(dpkg-deb -f "$deb" Package)" = "$package"
    test "$(dpkg-deb -f "$deb" Version)" = "$VERSION"
    test "$(dpkg-deb -f "$deb" Architecture)" = "$deb_arch"
    test "$(rpm -qp --qf '%{NAME}' "$rpm")" = "$package"
    test "$(rpm -qp --qf '%{VERSION}' "$rpm")" = "$VERSION"
    test "$(rpm -qp --qf '%{RELEASE}' "$rpm")" = 1
    test "$(rpm -qp --qf '%{ARCH}' "$rpm")" = "$rpm_arch"
  done
done
test "$(find "$DIST" -maxdepth 1 -type f \( -name '*.tar.gz' -o -name '*.deb' -o -name '*.rpm' \) | wc -l)" -eq 12
for f in "$DIST"/*.deb; do
  case "$(dpkg-deb -f "$f" Package)" in
    yorishiro-ce) peer=yorishiro-ee ;;
    yorishiro-ee) peer=yorishiro-ce ;;
  esac
  test "$(dpkg-deb -f "$f" Conflicts)" = "$peer"
  test -z "$(dpkg-deb -f "$f" Replaces)"
done
for f in "$DIST"/*.rpm; do
  case "$(rpm -qp --qf '%{NAME}' "$f")" in
    yorishiro-ce) peer=yorishiro-ee ;;
    yorishiro-ee) peer=yorishiro-ce ;;
  esac
  test "$(rpm -qp --conflicts "$f")" = "$peer"
  test -z "$(rpm -qp --obsoletes "$f")"
done
