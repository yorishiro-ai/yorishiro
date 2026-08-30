#!/usr/bin/env python3
"""Generates tests/fixtures/e5_reference_embeddings.json from a real sentence-transformers run of intfloat/multilingual-e5-base.

Unlike tests/fixtures/nomic_reference_embeddings.json (generated once from the now-removed ort implementation and irreplaceable), this fixture is regenerable: rerun this script inside the Docker image built from this directory's Dockerfile whenever the pinned revision changes.

sentence-transformers does not add multilingual-e5-base's query:/passage: prefixes on its own (see the model's own card), so this script prepends them by hand before encoding, exactly the convention src/services/embedding/model_fetch.rs's MULTILINGUAL_E5_BASE definition expects the candle provider to apply on its own via embed_as. The fixture stores raw (unprefixed) text; the Rust-side parity test applies the prefix itself and checks the result against these reference vectors, so the test exercises the prefix plumbing, not just the model's numeric output.
"""

import json
import sys

from sentence_transformers import SentenceTransformer

REVISION = "d128750597153bb5987e10b1c3493a34e5a4502a"
REPO = "intfloat/multilingual-e5-base"

# Japanese-heavy (multilingual is the point of this model), semantically distinct pairs and
# singletons, matching the shape of the nomic fixture this one sits alongside.
SENTENCES = [
    "猫がソファで眠っている",
    "子猫が毛糸で遊んでいる",
    "犬が公園を走り回っている",
    "自動車のエンジンを整備する",
    "四半期の売上高はアナリストの予想を上回った",
    "発表後に株価が急落した",
    "東京の夜景はとても美しい",
    "京都で紅葉を見に行った",
    "今日の天気は晴れのち曇りです",
    "パスワードをリセットする方法を教えてください",
]


def main() -> None:
    model = SentenceTransformer(REPO, revision=REVISION)

    entries = []
    for text in SENTENCES:
        query_vector = model.encode(f"query: {text}", normalize_embeddings=True).tolist()
        document_vector = model.encode(f"passage: {text}", normalize_embeddings=True).tolist()
        entries.append(
            {
                "text": text,
                "query_vector": query_vector,
                "document_vector": document_vector,
            }
        )

    fixture = {
        "entries": entries,
        "generated_by": f"sentence-transformers, {REPO} at {REVISION}",
        "repo": REPO,
        "revision": REVISION,
    }

    json.dump(fixture, sys.stdout, ensure_ascii=False)


if __name__ == "__main__":
    main()
