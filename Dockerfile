FROM rust:1-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY ordo-brain ./ordo-brain
COPY ordo-bus ./ordo-bus
COPY ordo-classify ./ordo-classify
COPY ordo-cli ./ordo-cli
COPY ordo-control ./ordo-control
COPY ordo-discovery ./ordo-discovery
COPY ordo-heal ./ordo-heal
COPY ordo-handshake ./ordo-handshake
COPY ordo-mcp-host ./ordo-mcp-host
COPY ordo-memory ./ordo-memory
COPY ordo-models ./ordo-models
COPY ordo-planner ./ordo-planner
COPY ordo-protocol ./ordo-protocol
COPY ordo-rag ./ordo-rag
COPY ordo-router ./ordo-router
COPY ordo-runtime ./ordo-runtime
COPY ordo-store ./ordo-store
COPY ordo-transport ./ordo-transport
COPY docs ./docs
COPY README.md ./

RUN cargo build --release -p ordo-cli

FROM debian:bookworm-slim
RUN useradd --create-home --shell /bin/bash ordo
WORKDIR /workspace
COPY --from=builder /workspace/target/release/ordo /usr/local/bin/ordo

ENV ORDO_DATABASE_PATH=/workspace/data/ordo.db
ENV ORDO_CONTROL_API_BIND=0.0.0.0:4141
ENV ORDO_LEGACY_MEMORY_PATH=/workspace/data/memory.jsonl
ENV ORDO_LEGACY_RAG_INDEX_PATH=/workspace/data/rag-index.jsonl
ENV ORDO_RUNTIME_PROFILE=standard
ENV ORDO_USER_FILES_PATH=/workspace/user-files
ENV ORDO_SELF_HEAL_HISTORY_BUDGET_BYTES=536870912

RUN mkdir -p /workspace/data /workspace/user-files && chown -R ordo:ordo /workspace
VOLUME ["/workspace/data", "/workspace/user-files"]
USER ordo

CMD ["ordo"]
