#!/usr/bin/env bash
# Static and effective checks for the image's configuration ownership.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
if grep -q 'COPY config/production.yaml /app/config/' "$repo/Dockerfile"; then
  echo 'Runtime defaults must come from the application, not a second configuration copy' >&2
  exit 1
fi
grep -q '^logger:' "$repo/config/production.yaml"
grep -q '^server:' "$repo/config/production.yaml"
grep -q '^database:' "$repo/config/production.yaml"
grep -q '^queue:' "$repo/config/production.yaml"
grep -q '^workers:' "$repo/config/production.yaml"

if command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
  TAG=v0.0.0 EDITION=ce DATABASE_URL=postgres://example.invalid/yorishiro \
    docker compose -f "$repo/docker-compose.yaml" config --format json |
    python3 -c '
import json
import sys

services = json.load(sys.stdin)["services"]
assert set(services) == {"app", "worker", "jobs", "scheduler", "redis"}
for role in {"app", "worker", "jobs", "scheduler"}:
    assert services[role]["image"] == "ghcr.io/yorishiro-ai/yorishiro:v0.0.0-ce"
    assert "build" not in services[role]
    assert services[role]["environment"]["BINDING"] == "0.0.0.0"
'
  docker compose -f "$repo/docker-compose.test.yaml" config --format json |
    python3 -c '
import json
import sys

config = json.load(sys.stdin)
assert set(config["services"]) == {"testdb", "redis"}
assert all("build" not in service for service in config["services"].values())
assert "/var/lib/postgresql" in config["services"]["testdb"]["tmpfs"]
'
  docker compose -f "$repo/docker-compose.dev.yaml" config --format json |
    python3 -c '
import json
import sys

services = json.load(sys.stdin)["services"]
assert set(services) == {"app", "worker", "jobs", "scheduler", "db", "redis"}
for role in {"app", "worker", "jobs", "scheduler"}:
    assert services[role]["build"]["context"]
assert set(services["app"]["depends_on"]) == {"db", "redis"}
for role in {"worker", "jobs"}:
    assert services[role]["healthcheck"]["disable"]
    assert set(services[role]["depends_on"]) == {"app", "redis"}
    assert all(dependency["condition"] == "service_healthy" for dependency in services[role]["depends_on"].values())
assert set(services["scheduler"]["depends_on"]) == {"app"}
assert any(mount["source"] == "pgdata" for mount in services["db"]["volumes"])
'
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
    --entrypoint sh "$image" -ec '
      test ! -e /app/config/production.yaml
      test ! -e /app/config/production.local.yaml
      test -z "$(find /app -type f)"
      test "$(find /usr/local/bin -type f)" = /usr/local/bin/yorishiro
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
      --entrypoint yorishiro "$image" start >/dev/null 2>&1
    host_port=""
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container")" = true ]]; then
      host_port=$(docker port "$container" 5150/tcp 2>/dev/null | sed 's/.*://' | head -1 || true)
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
        local-only) [[ "$log" == *"production.local.yaml"* ]] || { echo "$log" >&2; return 1; };;
        incomplete) [[ "$log" == *"production.yaml"* ]] || { echo "$log" >&2; return 1; };;
        unreadable) [[ "$log" == *"PermissionDenied"* ]] || { echo "$log" >&2; return 1; };;
      esac
    fi
    after_base=$(sha256sum "$tmp/config/production.yaml" 2>/dev/null || true)
    after_local=$(sha256sum "$tmp/config/production.local.yaml" 2>/dev/null || true)
    if [[ "$before_base" != "$after_base" || "$before_local" != "$after_local" ]]; then
      echo "$name: the container changed its mounted configuration" >&2
      return 1
    fi
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
  run_config_case missing success rm "$tmp/config/production.yaml"
  run_config_case local-only failure sh -c "rm '$tmp/config/production.yaml'; printf 'settings:\\n  max_tenants: 1\\n' > '$tmp/config/production.local.yaml'"
  run_config_case incomplete failure sh -c "printf 'logger: {}\\n' > '$tmp/config/production.yaml'"
  run_config_case unreadable failure sh -c "chmod 000 '$tmp/config/production.yaml'"
fi
