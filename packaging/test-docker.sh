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
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  docker run --rm \
    -v "$repo/config/example.yaml:/canonical-example.yaml:ro" \
    --entrypoint sh "$image" -ec '
      test -f /app/config/example.yaml
      cmp /app/config/example.yaml /canonical-example.yaml
      test ! -e /app/config/production.yaml
      test ! -e /app/config/production.local.yaml
    '

  # Keep runtime probes short and use the disabled provider so no model download is involved.
  run_config_case() {
    local name=$1 expected=$2
    shift 2
    rm -rf "$tmp/config" "$tmp/state"
    mkdir -p "$tmp/config" "$tmp/state"
    chmod 0777 "$tmp/state"
    cp "$repo/config/example.yaml" "$tmp/config/production.yaml"
    "$@"
    set +e
    timeout 20 docker run --rm \
      -e DATABASE_URL='sqlite:///var/lib/yorishiro/runtime.sqlite3?mode=rwc' \
      -e QUEUE_URL='sqlite:///var/lib/yorishiro/runtime-queue.sqlite3?mode=rwc' \
      -e YORISHIRO_EMBEDDING_PROVIDER=none \
      -e BINDING=127.0.0.1 \
      -v "$tmp/config:/app/config" \
      -v "$tmp/state:/var/lib/yorishiro" \
      --entrypoint yorishiro "$image" start >/tmp/yorishiro-docker-$name.log 2>&1 &
    pid=$!
    sleep 3
    kill "$pid" 2>/dev/null || true
    wait "$pid"
    actual=$?
    set -e
    if [[ "$expected" = success && "$actual" -ne 0 && "$actual" -ne 143 ]]; then
      cat "/tmp/yorishiro-docker-$name.log" >&2
      return 1
    fi
    if [[ "$expected" = failure && "$actual" -eq 0 ]]; then
      cat "/tmp/yorishiro-docker-$name.log" >&2
      return 1
    fi
  }

  run_config_case embedded success rm "$tmp/config/production.yaml"
  run_config_case external success true
  run_config_case merged success sh -c "printf 'settings:\\n  max_tenants: 1\\n' > '$tmp/config/production.local.yaml'"
  run_config_case malformed failure sh -c "printf 'not: [valid' > '$tmp/config/production.yaml'"
  rm -f "$tmp/config/production.local.yaml"
  run_config_case local-only failure sh -c "rm '$tmp/config/production.yaml'; printf 'settings:\\n  max_tenants: 1\\n' > '$tmp/config/production.local.yaml'"
fi
