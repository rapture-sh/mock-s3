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

# musl, so the binary links nothing at runtime and the final stage can be `scratch`.
# Against glibc it needs libc, libm and libgcc, and supplying those means shipping a
# Debian userland: 88 packages this binary never calls, all of them accumulating CVEs
# that are then reported against the image. Each architecture is built on a machine of
# its own architecture, so `uname -m` names the target and nothing is cross-compiled.
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY xtask ./xtask
COPY --from=console /web/dist ./web/dist

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    target="$(uname -m)-unknown-linux-musl"; \
    rustup target add "$target" \
    && cargo build --release --locked --target "$target" --package mock-s3 \
    && cp "target/$target/release/mock-s3" /usr/local/bin/mock-s3

# The three things `scratch` does not come with. The passwd and group entries give the
# uid a name, and the data directory has to be created here because a volume mounted
# over a path that does not exist is created owned by root, which the server, running
# unprivileged, then cannot write to.
RUN mkdir -p /rootfs/data \
    && echo 'mock-s3:x:10001:10001::/data:/sbin/nologin' > /rootfs/passwd \
    && echo 'mock-s3:x:10001:' > /rootfs/group

# Runtime stage. The binary and nothing else: no package manager, no shell, no shared
# libraries, and so nothing for a scanner to find. The cost is that `docker exec` into a
# running container is not possible; inspect the volume from the host instead.
FROM scratch

COPY --from=build /rootfs/passwd /etc/passwd
COPY --from=build /rootfs/group /etc/group
COPY --from=build --chown=10001:10001 /rootfs/data /data
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
