# syntax=docker/dockerfile:1

# Console stage. The assets are embedded into the binary by `rust-embed`, so they have to
# exist before the Rust build runs; `web/dist` is in .dockerignore precisely so the
# image builds its own rather than shipping whatever happened to be on the host.
FROM node:22-bookworm-slim AS console

WORKDIR /web

RUN corepack enable

# Manifests first: dependencies only re-resolve when they actually change, not on every
# edit to a component.
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile

COPY web ./
RUN pnpm build

# Build stage. Cache mounts keep the crates.io registry and the target directory out of
# the image layers, so a code-only change rebuilds in seconds instead of re-fetching and
# re-compiling the whole dependency tree.
FROM rust:1.100-slim-bookworm AS build

WORKDIR /src

COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY xtask ./xtask
COPY --from=console /web/dist ./web/dist

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --package mock-s3 \
    && cp target/release/mock-s3 /usr/local/bin/mock-s3

# Runtime stage.
FROM debian:bookworm-slim

# Objects land in a mounted volume, so the data directory has to be writable by the
# unprivileged user the server runs as.
RUN useradd --system --uid 10001 --user-group mock-s3 \
    && mkdir -p /data \
    && chown -R mock-s3:mock-s3 /data

COPY --from=build /usr/local/bin/mock-s3 /usr/local/bin/mock-s3

USER mock-s3
WORKDIR /data

ENV MOCK_S3_DATA=/data \
    MOCK_S3_BIND=0.0.0.0 \
    MOCK_S3_PORT=9000 \
    MOCK_S3_CONSOLE_PORT=9001 \
    MOCK_S3_REGION=us-east-1 \
    MOCK_S3_LOG=info

# 9000 is the S3 API, 9001 the console and admin API.
EXPOSE 9000 9001
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/mock-s3"]
