#!/usr/bin/env bash
set -euo pipefail

: "${YORISHIRO_REDIS_TEST_URL:?set YORISHIRO_REDIS_TEST_URL to an accessible redis:// or rediss:// endpoint}"
case "$YORISHIRO_REDIS_TEST_URL" in
  redis://*|rediss://*) ;;
  *) echo "YORISHIRO_REDIS_TEST_URL must use redis:// or rediss://" >&2; exit 2 ;;
esac

export LOCO_ENV="${LOCO_ENV:-test_sqlite}"
cargo test --locked --features test-support --test mod \
  requests::queue::redis_bounded_scan_is_observable_at_the_queue_boundary -- --exact --nocapture
