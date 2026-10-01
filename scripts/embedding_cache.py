#!/usr/bin/env -S uv run --script
"""Prepare and verify the immutable embedding model cache."""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import sys
import tempfile
import time
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
REQUIRED = (
    "CACHE_DIR",
    "MODEL_URL",
    "TOKENIZER_URL",
    "EMBEDDING_MODEL_ID",
    "EMBEDDING_MODEL_SHORT_ID",
    "EMBEDDING_MODEL_REMOTE_PATH",
    "EMBEDDING_MODEL_LOCAL_NAME",
    "EMBEDDING_TOKENIZER_REMOTE_PATH",
    "EMBEDDING_TOKENIZER_LOCAL_NAME",
    "EMBEDDING_MODEL_REVISION",
    "EMBEDDING_MODEL_SHA256",
    "EMBEDDING_MODEL_SIZE",
    "EMBEDDING_TOKENIZER_SHA256",
    "EMBEDDING_TOKENIZER_SIZE",
)


def env(name: str) -> str:
    value = os.environ.get(name)
    if value is None:
        raise SystemExit(f"{name} is required")
    return value


def verify(path: Path, size: str, digest: str) -> bool:
    if not path.is_file() or path.is_symlink() or path.stat().st_size != int(size):
        return False
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest() == digest


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.is_dir():
        shutil.rmtree(path)


def download(path: Path, url: str, size: str, digest: str) -> None:
    remove_path(path)
    for attempt in range(5):
        temporary: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                dir=path.parent, prefix=f".{path.name}.", suffix=".partial", delete=False
            ) as stream:
                temporary = Path(stream.name)
                with urllib.request.urlopen(url) as response:
                    shutil.copyfileobj(response, stream)
            if not verify(temporary, size, digest):
                raise RuntimeError(f"downloaded artifact failed size or SHA-256 verification: {url}")
            temporary.replace(path)
            return
        except Exception:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
            if attempt == 4:
                raise
            time.sleep(1 << attempt)


def validate_source(values: dict[str, str]) -> None:
    source = (ROOT / "src/services/embedding/model_fetch.rs").read_text()
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
        if actual != values[name].replace("_", ""):
            raise SystemExit(f"{name} does not match model_fetch.rs")
    if not re.search(
        r"pub\(super\) const DEFAULT_MODEL: &LocalModelDef = &MULTILINGUAL_E5_BASE;", source
    ):
        raise SystemExit("DEFAULT_MODEL does not alias MULTILINGUAL_E5_BASE")


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in {"prepare", "verify"}:
        print("usage: uv run --script scripts/embedding_cache.py <prepare|verify>", file=sys.stderr)
        return 2
    values = {name: env(name) for name in REQUIRED}
    validate_source(values)
    cache_dir = Path(values["CACHE_DIR"]).expanduser()
    model_path = cache_dir / values["EMBEDDING_MODEL_LOCAL_NAME"]
    tokenizer_path = cache_dir / values["EMBEDDING_TOKENIZER_LOCAL_NAME"]
    valid = lambda: verify(model_path, values["EMBEDDING_MODEL_SIZE"], values["EMBEDDING_MODEL_SHA256"]) and verify(
        tokenizer_path, values["EMBEDDING_TOKENIZER_SIZE"], values["EMBEDDING_TOKENIZER_SHA256"]
    )

    if sys.argv[1] == "verify":
        if values.get("CACHE_HIT", os.environ.get("CACHE_HIT", "")) != "true" or not valid():
            raise SystemExit("embedding cache is absent or invalid")
        return 0
    if os.environ.get("CACHE_HIT") == "true":
        if not valid():
            raise SystemExit("immutable embedding cache hit is invalid")
    else:
        cache_dir.mkdir(parents=True, exist_ok=True)
        download(model_path, values["MODEL_URL"], values["EMBEDDING_MODEL_SIZE"], values["EMBEDDING_MODEL_SHA256"])
        download(tokenizer_path, values["TOKENIZER_URL"], values["EMBEDDING_TOKENIZER_SIZE"], values["EMBEDDING_TOKENIZER_SHA256"])
        if not valid():
            raise SystemExit("embedding cache is absent or invalid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
