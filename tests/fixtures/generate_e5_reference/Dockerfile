# syntax=docker/dockerfile:1.29.0

#   docker build -t yorishiro-e5-reference tests/fixtures/generate_e5_reference
#   docker run --rm yorishiro-e5-reference > tests/fixtures/multilingual-e5-base_reference_embeddings.json
#
# Transitive dependencies are unpinned, so the parity test checks cosine similarity, not exact values.
FROM python:3.14-slim

WORKDIR /app

RUN pip install --no-cache-dir \
    torch==2.15.0+cpu --index-url https://download.pytorch.org/whl/cpu \
    && pip install --no-cache-dir \
    sentence-transformers==6.2.0 \
    transformers==5.22.1 \
    && pip check

COPY generate.py .

ENTRYPOINT ["python", "generate.py"]
