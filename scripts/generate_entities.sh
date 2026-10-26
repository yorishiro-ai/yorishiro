#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
container=
cleanup() {
    if [[ -n "$container" ]]; then
        docker rm --force --volumes "$container" >/dev/null
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

container=$(docker run --detach \
    --env POSTGRES_USER=yorishiro \
    --env POSTGRES_PASSWORD=yorishiro \
    --env POSTGRES_DB=yorishiro \
    --mount "type=bind,source=$PWD/init-extensions.sql,target=/docker-entrypoint-initdb.d/init-extensions.sql,readonly" \
    --publish 127.0.0.1::5432 \
    --health-cmd 'pg_isready -U yorishiro -h 127.0.0.1' \
    --health-interval 1s --health-timeout 3s --health-retries 60 \
    pgvector/pgvector:pg18)

ready=false
for ((attempt=0; attempt<90; attempt++)); do
    status=$(docker inspect --format '{{.State.Health.Status}}' "$container")
    case "$status" in
        healthy) ready=true; break ;;
        unhealthy) break ;;
    esac
    sleep 1
done
if [[ "$ready" != true ]]; then
    docker logs "$container" >&2
    exit 1
fi

port=$(docker inspect --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "$container")
export DATABASE_URL="postgres://yorishiro:yorishiro@127.0.0.1:$port/yorishiro"
export QUEUE_URL="$DATABASE_URL"
export YORISHIRO_EMBEDDING_PROVIDER=none
export LOCO_ENV=test_postgres

./target/debug/yorishiro db migrate
rm -f src/models/_entities/*.rs
./target/debug/yorishiro db entities
python3 scripts/harden_generated_entities.py
