#!/usr/bin/env bash
# Static and effective checks for the image's configuration ownership.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
grep -q 'COPY config/example.yaml /app/config/example.yaml' "$repo/Dockerfile"
! grep -q 'COPY config/production.yaml' "$repo/Dockerfile"
grep -q 'BINDING: 0.0.0.0' "$repo/docker-compose.yml"
grep -q '^logger:' "$repo/config/example.yaml"
grep -q '^server:' "$repo/config/example.yaml"
grep -q '^database:' "$repo/config/example.yaml"
grep -q '^queue:' "$repo/config/example.yaml"
grep -q '^workers:' "$repo/config/example.yaml"

if command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
  docker compose -f "$repo/docker-compose.yml" config >/dev/null
fi

if [[ $# -gt 0 ]]; then
  image=$1
  docker run --rm \
    -v "$repo/config/example.yaml:/canonical-example.yaml:ro" \
    --entrypoint sh "$image" -ec '
      test -f /app/config/example.yaml
      cmp /app/config/example.yaml /canonical-example.yaml
      test ! -e /app/config/production.yaml
      test ! -e /app/config/production.local.yaml
    '
fi
