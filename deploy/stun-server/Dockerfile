# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder

WORKDIR /src
COPY . .

# Cache Cargo downloads and compiled dependencies across BuildKit builds. The
# finished binary is copied outside the cached target directory so the runtime
# stage can consume it.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --release -p stun-server --bin swarm-stun-server && \
    install -Dm755 target/release/swarm-stun-server /out/swarm-stun-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install --no-install-recommends -y ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 swarm && \
    useradd --uid 10001 --gid swarm --home-dir /data --shell /usr/sbin/nologin swarm && \
    install -d -o swarm -g swarm /data /opt/swarm/static

COPY --from=builder /out/swarm-stun-server /usr/local/bin/swarm-stun-server
COPY --from=builder --chown=swarm:swarm /src/apps/stun-server/static/ /opt/swarm/static/

USER swarm
WORKDIR /data

ENV SWARM_DATABASE_PATH=/data/swarm.sqlite \
    SWARM_HTTP_BIND=0.0.0.0:8080 \
    SWARM_REFLECTOR_PORTS=9443,3478 \
    SWARM_STATIC_DIR=/opt/swarm/static

VOLUME ["/data"]
EXPOSE 8080/tcp 9443/udp 3478/udp

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "--output", "/dev/null", "http://127.0.0.1:8080/health"]

STOPSIGNAL SIGTERM
ENTRYPOINT ["swarm-stun-server"]
