# Builds and runs generate.py, producing tests/fixtures/multilingual-e5-base_reference_embeddings.json on stdout.
#
# Usage (from the repository root):
#   docker build -t yorishiro-e5-reference tests/fixtures/generate_e5_reference
#   docker run --rm yorishiro-e5-reference > tests/fixtures/multilingual-e5-base_reference_embeddings.json
#
# torch/sentence-transformers/transformers are pinned directly; their own transitive dependencies
# (numpy, tokenizers, safetensors, and so on) are not pinned here and resolve to whatever is
# current at build time, so a rerun is not guaranteed byte-for-byte identical to this run.
# The parity test's own >0.999 cosine threshold is what actually absorbs that drift, the same way
# it already absorbs floating-point op-ordering differences between candle and any reference
# implementation; it is not claiming exact reproducibility across every dependency in this image.
FROM python:3.12.7-slim

WORKDIR /app

RUN pip install --no-cache-dir \
    torch==2.5.1 --index-url https://download.pytorch.org/whl/cpu \
    && pip install --no-cache-dir \
    sentence-transformers==3.3.1 \
    transformers==4.46.3

COPY generate.py .

ENTRYPOINT ["python", "generate.py"]
