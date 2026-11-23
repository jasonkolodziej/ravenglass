# syntax=docker/dockerfile:1.7
#
# Ravenglass Relay: a static musl binary in a distroless image.
#
#   docker build -f packaging/relay/Dockerfile -t ravenglass-relay .          # from the repo root
#   docker buildx build -f packaging/relay/Dockerfile --platform linux/amd64,linux/arm64 .
#
# Configuration is the environment, passed through at run time (see packaging/relay/README in
# packaging/README.md and crates/ravenglass-relay/src/main.rs); nothing is baked in except the
# listen address, which must not be loopback inside a container. State, when the Relay keeps any,
# belongs in the /data volume.
#
# No HEALTHCHECK: the binary has no health endpoint or self-check flag, every route needs the
# bearer token, and the final image has no shell or curl. Orchestrators can probe the TCP port.

ARG RUST_IMAGE=rust:1-slim-bookworm

# Build on the runner's own architecture and cross-compile with cargo-zigbuild (zig as the musl
# linker), so a multi-platform build does not run rustc under emulation. zig >= 0.15 is needed for
# aarch64: rustc passes --fix-cortex-a53-843419, which zig 0.14 rejects.
FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS build
ARG TARGETARCH
WORKDIR /src
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-pip ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && pip3 install --break-system-packages --no-cache-dir ziglang==0.15.2 cargo-zigbuild==0.24.1
COPY . .
# rust-toolchain.toml pins the compiler; rustup installs it on first use.
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl > /target ;; \
      arm64) echo aarch64-unknown-linux-musl > /target ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && rustup target add "$(cat /target)"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target,id=relay-target-${TARGETARCH} \
    cargo zigbuild --release --locked -p ravenglass-relay --target "$(cat /target)" \
 && mkdir -p /out /out/data \
 && cp "target/$(cat /target)/release/ravenglass-relay" /out/ravenglass-relay

# distroless/static: CA certificates, tzdata and a `nonroot` user (65532), no shell.
FROM gcr.io/distroless/static-debian12:nonroot
LABEL org.opencontainers.image.title="Ravenglass Relay" \
      org.opencontainers.image.description="Bridges the Ravenglass web client to IMAP, SMTP and JMAP." \
      org.opencontainers.image.source="https://github.com/jasonkolodziej/ravenglass" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
COPY --from=build /out/ravenglass-relay /usr/local/bin/ravenglass-relay
COPY --from=build --chown=65532:65532 /out/data /data
USER 65532:65532
WORKDIR /data
VOLUME ["/data"]
ENV RAVENGLASS_RELAY_LISTEN=0.0.0.0:8743
EXPOSE 8743
ENTRYPOINT ["/usr/local/bin/ravenglass-relay"]
