#!/usr/bin/env bash
set -euo pipefail

mode=${1:?usage: embedding-cache.sh <prepare|verify>}
: "${CACHE_DIR:?}"
: "${CACHE_HIT:?}"
: "${MODEL_URL:?}"
: "${TOKENIZER_URL:?}"
: "${EMBEDDING_MODEL_ID:?}"
: "${EMBEDDING_MODEL_SHORT_ID:?}"
: "${EMBEDDING_MODEL_REMOTE_PATH:?}"
: "${EMBEDDING_MODEL_LOCAL_NAME:?}"
: "${EMBEDDING_TOKENIZER_REMOTE_PATH:?}"
: "${EMBEDDING_TOKENIZER_LOCAL_NAME:?}"
: "${EMBEDDING_MODEL_REVISION:?}"
: "${EMBEDDING_MODEL_SHA256:?}"
: "${EMBEDDING_MODEL_SIZE:?}"
: "${EMBEDDING_TOKENIZER_SHA256:?}"
: "${EMBEDDING_TOKENIZER_SIZE:?}"

CACHE_DIR="${CACHE_DIR/#\~/$HOME}"

python3 - <<'PY'
import os
import re
from pathlib import Path

source = Path("src/services/embedding/model_fetch.rs").read_text()
definition = source.split("pub(super) static MULTILINGUAL_E5_BASE", 1)[1].split(
    "pub(super) const DEFAULT_MODEL", 1
)[0]
model, tokenizer = definition.split("tokenizer: Artifact", 1)

expected = {
    "EMBEDDING_MODEL_ID": (definition, r'\bid: "([^"]+)"'),
    "EMBEDDING_MODEL_SHORT_ID": (definition, r'\bshort_id: "([^"]+)"'),
    "EMBEDDING_MODEL_REVISION": (definition, r'\brevision: "([^"]+)"'),
    "EMBEDDING_MODEL_REMOTE_PATH": (model, r'\bremote_path: "([^"]+)"'),
    "EMBEDDING_MODEL_LOCAL_NAME": (model, r'\blocal_name: "([^"]+)"'),
    "EMBEDDING_MODEL_SHA256": (model, r'\bsha256: "([^"]+)"'),
    "EMBEDDING_MODEL_SIZE": (model, r'\bsize: ([0-9_]+)'),
    "EMBEDDING_TOKENIZER_REMOTE_PATH": (tokenizer, r'\bremote_path: "([^"]+)"'),
    "EMBEDDING_TOKENIZER_LOCAL_NAME": (tokenizer, r'\blocal_name: "([^"]+)"'),
    "EMBEDDING_TOKENIZER_SHA256": (tokenizer, r'\bsha256: "([^"]+)"'),
    "EMBEDDING_TOKENIZER_SIZE": (tokenizer, r'\bsize: ([0-9_]+)'),
}
for name, (haystack, pattern) in expected.items():
    match = re.search(pattern, haystack)
    actual = match.group(1).replace("_", "") if match else None
    if actual != os.environ[name].replace("_", ""):
        raise SystemExit(f"{name} does not match model_fetch.rs")

if not re.search(
    r"pub\(super\) const DEFAULT_MODEL: &LocalModelDef = &MULTILINGUAL_E5_BASE;",
    source,
):
    raise SystemExit("DEFAULT_MODEL does not alias MULTILINGUAL_E5_BASE")
PY

model_path="$CACHE_DIR/$EMBEDDING_MODEL_LOCAL_NAME"
tokenizer_path="$CACHE_DIR/$EMBEDDING_TOKENIZER_LOCAL_NAME"

verify() {
  local path=$1 expected_size=$2 expected_sha=$3
  [[ -f "$path" && ! -L "$path" ]] || return 1
  [[ "$(stat -c '%s' "$path")" == "$expected_size" ]] || return 1
  [[ "$(sha256sum "$path" | cut -d' ' -f1)" == "$expected_sha" ]]
}

verify_all() {
  verify "$model_path" "$EMBEDDING_MODEL_SIZE" "$EMBEDDING_MODEL_SHA256"
  verify "$tokenizer_path" "$EMBEDDING_TOKENIZER_SIZE" "$EMBEDDING_TOKENIZER_SHA256"
}

remove_path() {
  local path=$1
  if [[ -L "$path" || -d "$path" ]]; then
    rm -rf -- "$path"
  else
    rm -f -- "$path"
  fi
}

download() {
  local path=$1 url=$2 expected_size=$3 expected_sha=$4
  remove_path "$path"
  local temp
  temp=$(mktemp "$CACHE_DIR/.$(basename "$path").XXXXXX.partial")
  if ! curl --fail --location --retry 5 --retry-all-errors \
    --output "$temp" "$url"; then
    rm -f -- "$temp"
    return 1
  fi
  if ! verify "$temp" "$expected_size" "$expected_sha"; then
    echo "ERROR: downloaded artifact failed size or SHA-256 verification: $url" >&2
    rm -f -- "$temp"
    return 1
  fi
  mv -f -- "$temp" "$path"
}

case "$mode" in
  verify)
    verify_all || {
      echo "ERROR: embedding cache is absent or invalid" >&2
      exit 1
    }
    ;;
  prepare)
    if [[ "$CACHE_HIT" == "true" ]]; then
      verify_all || {
        echo "ERROR: immutable embedding cache hit is invalid" >&2
        exit 1
      }
    else
      mkdir -p "$CACHE_DIR"
      download "$model_path" "$MODEL_URL" \
        "$EMBEDDING_MODEL_SIZE" "$EMBEDDING_MODEL_SHA256"
      download "$tokenizer_path" "$TOKENIZER_URL" \
        "$EMBEDDING_TOKENIZER_SIZE" "$EMBEDDING_TOKENIZER_SHA256"
      verify_all
    fi
    ;;
  *)
    echo "usage: embedding-cache.sh <prepare|verify>" >&2
    exit 2
    ;;
esac
