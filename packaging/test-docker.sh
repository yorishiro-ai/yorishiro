#!/usr/bin/env bash
# Static and effective checks for the image's configuration ownership.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
grep -q 'COPY config/production.yaml /app/config/production.yaml' "$repo/Dockerfile"
grep -q 'BINDING: 0.0.0.0' "$repo/docker-compose.yml"
grep -q '^logger:' "$repo/config/production.yaml"
grep -q '^server:' "$repo/config/production.yaml"
grep -q '^database:' "$repo/config/production.yaml"
grep -q '^queue:' "$repo/config/production.yaml"
grep -q '^workers:' "$repo/config/production.yaml"

if command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
  docker compose -f "$repo/docker-compose.yml" config >/dev/null
fi

if [[ $# -gt 0 ]]; then
  image=$1
  tmp=$(mktemp -d)
  active_container=""
  cleanup() {
    if [[ -n "$active_container" ]]; then docker rm -f "$active_container" >/dev/null 2>&1 || true; fi
    rm -rf "$tmp"
  }
  trap cleanup EXIT
  docker run --rm \
    -v "$repo/config/production.yaml:/canonical-example.yaml:ro" \
    --entrypoint sh "$image" -ec '
      test -f /app/config/production.yaml
      cmp /app/config/production.yaml /canonical-example.yaml
      test ! -e /app/config/production.local.yaml
    '

  # Keep runtime probes short and use the disabled provider so no model download is involved.
  run_config_case() {
    local name=$1 expected=$2
    shift 2
    rm -rf "$tmp/config" "$tmp/state"
    mkdir -p "$tmp/config" "$tmp/state"
    chmod 0777 "$tmp/state"
    cp "$repo/config/production.yaml" "$tmp/config/production.yaml"
    "$@"
    before_base=$(sha256sum "$tmp/config/production.yaml" 2>/dev/null || true)
    before_local=$(sha256sum "$tmp/config/production.local.yaml" 2>/dev/null || true)
    container="yorishiro-docker-$name-$$"
    active_container="$container"
    cleanup_case() {
      docker rm -f "$container" >/dev/null 2>&1 || true
      rm -rf "$tmp/config" "$tmp/state"
    }
    docker run -d --name "$container" \
      -e DATABASE_URL='sqlite:///var/lib/yorishiro/runtime.sqlite3?mode=rwc' \
      -e QUEUE_URL='sqlite:///var/lib/yorishiro/runtime-queue.sqlite3?mode=rwc' \
      -e YORISHIRO_EMBEDDING_PROVIDER=none \
      -e BINDING=0.0.0.0 \
      -v "$tmp/config:/app/config" \
      -v "$tmp/state:/var/lib/yorishiro" \
      -p 0:5150 \
      --entrypoint yorishiro "$image" start >/dev/null 2>&1 >/dev/null
    host_port=""
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container")" = true ]]; then
      host_port=$(docker port "$container" 5150/tcp | sed 's/.*://' | head -1)
    fi
    ready=false
    for _ in $(seq 1 20); do
      if [[ -n "$host_port" ]] && curl -fsS "http://127.0.0.1:$host_port/_ping" >/dev/null 2>&1; then ready=true; break; fi
      if [[ "$(docker inspect -f '{{.State.Running}}' "$container")" != true ]]; then break; fi
      sleep 1
    done
    if [[ "$expected" = success && "$ready" = true ]]; then
      docker stop "$container" >/dev/null
    fi
    actual=$(docker inspect -f '{{.State.ExitCode}}' "$container")
    docker logs "$container" >/tmp/yorishiro-docker-$name.log 2>&1 || true
    if [[ "$expected" = failure && "$ready" != true ]]; then
      log=$(cat "/tmp/yorishiro-docker-$name.log")
      case "$name" in
        malformed) [[ "$log" == *"YAMLFile"*"production.yaml"* ]] || { echo "$log" >&2; return 1; };;
        missing|local-only|incomplete|unreadable) [[ "$log" == *"production.yaml"* ]] || { echo "$log" >&2; return 1; };;
      esac
    fi
    after_base=$(sha256sum "$tmp/config/production.yaml" 2>/dev/null || true)
    after_local=$(sha256sum "$tmp/config/production.local.yaml" 2>/dev/null || true)
    test "$before_base" = "$after_base" && test "$before_local" = "$after_local"
    if [[ "$expected" = success && "$ready" != true ]]; then
      cat "/tmp/yorishiro-docker-$name.log" >&2
      return 1
    fi
    if [[ "$expected" = failure && "$ready" = true ]]; then
      cat "/tmp/yorishiro-docker-$name.log" >&2
      return 1
    fi
    if [[ "$expected" = failure && "$actual" -eq 0 ]]; then
      cat "/tmp/yorishiro-docker-$name.log" >&2
      return 1
    fi
    docker rm -f "$container" >/dev/null 2>&1 || true
    active_container=""
    cleanup_case
  }

  run_config_case external success true
  run_config_case merged success sh -c "printf 'settings:\\n  max_tenants: 1\\n' > '$tmp/config/production.local.yaml'"
  run_config_case malformed failure sh -c "printf 'not: [valid' > '$tmp/config/production.yaml'"
  rm -f "$tmp/config/production.local.yaml"
  run_config_case missing failure rm "$tmp/config/production.yaml"
  run_config_case local-only failure sh -c "rm '$tmp/config/production.yaml'; printf 'settings:\\n  max_tenants: 1\\n' > '$tmp/config/production.local.yaml'"
  run_config_case incomplete failure sh -c "printf 'logger: {}\\n' > '$tmp/config/production.yaml'"
  run_config_case unreadable failure sh -c "chmod 000 '$tmp/config/production.yaml'"
fi
